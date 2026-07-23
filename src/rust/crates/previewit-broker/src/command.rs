use std::io;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use previewit_protocol::MAX_CONTROL_FRAME;
use previewit_protocol::v0::{BrokerControlRequest, BrokerControlResponse, broker_control_request};
use prost::Message;
use thiserror::Error;
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use tokio::runtime::Builder;
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY};

use crate::control::InvalidCommandResponse;
use crate::pipe::{BrokerError, read_frame, write_frame};
use crate::windows_security::{CurrentUserSecurity, WindowsSecurityError};
use crate::{
    BrokerCommand, BrokerControlContract, CommandAck, CommandId, CommandRejectionCode, ExpectedAck,
    ValidatedCommand,
};

const MAX_QUEUED_COMMANDS: usize = 8;
const MAX_ACTIVE_COMMANDS: usize = 1;
const MAX_DECODING_CONNECTIONS: usize = 4;
const LISTENER_RESERVE: usize = 1;
const MAX_PIPE_INSTANCES: usize =
    MAX_QUEUED_COMMANDS + MAX_ACTIVE_COMMANDS + MAX_DECODING_CONNECTIONS + LISTENER_RESERVE;
const PIPE_BUFFER_SIZE: u32 = (MAX_CONTROL_FRAME + 4) as u32;
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Error)]
pub enum BrokerCommandError {
    #[error("the primary Broker did not become ready before the startup deadline")]
    PrimaryNotReady,
    #[error("the primary Broker has no free connection decoder")]
    PrimaryBusy,
    #[error("the Broker command was rejected before it could be sent: {0:?}")]
    RequestRejected(CommandRejectionCode),
    #[error("invalid Broker control response: {0}")]
    InvalidResponse(#[source] prost::DecodeError),
    #[error(transparent)]
    ResponseContract(#[from] InvalidCommandResponse),
    #[error("the Broker command listener stopped")]
    ListenerStopped,
    #[error("there is no pending Broker command response")]
    NoPendingResponse,
    #[error("the command client stopped before receiving its response")]
    ResponseReceiverDropped,
    #[error(transparent)]
    Transport(#[from] BrokerError),
}

impl BrokerCommandError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PrimaryNotReady => "primary-not-ready",
            Self::PrimaryBusy => "primary-busy",
            Self::RequestRejected(reason) => reason.as_str(),
            Self::InvalidResponse(_) => "invalid-response",
            Self::ResponseContract(error) => error.code(),
            Self::ListenerStopped => "listener-stopped",
            Self::NoPendingResponse => "no-pending-response",
            Self::ResponseReceiverDropped => "response-receiver-dropped",
            Self::Transport(error) => match error {
                BrokerError::StartupTimeout => "startup-timeout",
                BrokerError::ReadTimeout => "read-timeout",
                BrokerError::WriteTimeout => "write-timeout",
                BrokerError::ControlFrameTooLarge { .. } => "frame-too-large",
                BrokerError::TruncatedControlFrame => "truncated-frame",
                BrokerError::ProtocolMajorMismatch { .. } => "protocol-major-mismatch",
                BrokerError::ProtocolMinorMismatch { .. } => "protocol-minor-mismatch",
                BrokerError::InvalidEnvelope(_) => "invalid-protobuf",
                BrokerError::UnexpectedClientProcess { .. }
                | BrokerError::ExpectedHello
                | BrokerError::MissingReadHandleCapability
                | BrokerError::Frame(_)
                | BrokerError::Windows { .. } => "transport-error",
            },
        }
    }
}

impl From<WindowsSecurityError> for BrokerCommandError {
    fn from(error: WindowsSecurityError) -> Self {
        let WindowsSecurityError::Windows { operation, source } = error;
        BrokerError::Windows { operation, source }.into()
    }
}

pub struct PendingCommand {
    command: ValidatedCommand,
    wire_request: BrokerControlRequest,
    reply: oneshot::Sender<CommandAck>,
}

impl PendingCommand {
    pub fn command(&self) -> &ValidatedCommand {
        &self.command
    }

    pub fn respond(self, ack: CommandAck) -> Result<(), BrokerCommandError> {
        let expected = expected_for_command(&self.command);
        let response = BrokerControlContract::encode_response(&ack);
        let validated = BrokerControlContract::decode_response(&expected, response)?;
        self.reply
            .send(validated)
            .map_err(|_| BrokerCommandError::ResponseReceiverDropped)
    }
}

pub struct BrokerCommandServer {
    receiver: Receiver<PendingCommand>,
    shutdown: Option<oneshot::Sender<()>>,
    endpoint_thread: Option<JoinHandle<()>>,
    receive_timeout: Duration,
    legacy_pending: Mutex<Option<PendingCommand>>,
}

impl BrokerCommandServer {
    pub fn create(
        name: &str,
        startup_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<Self, BrokerCommandError> {
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUED_COMMANDS);
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let endpoint_name = name.to_owned();
        let endpoint_thread = thread::Builder::new()
            .name("previewit-command-endpoint".into())
            .spawn(move || {
                endpoint_thread_main(
                    endpoint_name,
                    io_timeout,
                    sender,
                    shutdown_receiver,
                    ready_sender,
                );
            })
            .map_err(|source| transport_error("spawn command endpoint", source))?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                receiver,
                shutdown: Some(shutdown),
                endpoint_thread: Some(endpoint_thread),
                receive_timeout: startup_timeout,
                legacy_pending: Mutex::new(None),
            }),
            Ok(Err(error)) => {
                let _ = endpoint_thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = endpoint_thread.join();
                Err(BrokerCommandError::ListenerStopped)
            }
        }
    }

    pub fn receive(&self) -> Result<PendingCommand, BrokerCommandError> {
        self.receiver
            .recv_timeout(self.receive_timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => BrokerError::StartupTimeout.into(),
                RecvTimeoutError::Disconnected => BrokerCommandError::ListenerStopped,
            })
    }

    // Transitional adapter for the raw router. Task 4 removes it when routing becomes domain-only.
    pub fn receive_request(&self) -> Result<BrokerControlRequest, BrokerCommandError> {
        let pending = self.receive()?;
        let request = pending.wire_request.clone();
        let mut active = self
            .legacy_pending
            .lock()
            .map_err(|_| BrokerCommandError::ListenerStopped)?;
        if active.is_some() {
            return Err(BrokerCommandError::NoPendingResponse);
        }
        *active = Some(pending);
        Ok(request)
    }

    // Transitional adapter for the raw router. Task 4 removes it when routing becomes domain-only.
    pub fn send_response(
        &self,
        response: &BrokerControlResponse,
    ) -> Result<(), BrokerCommandError> {
        let pending = self
            .legacy_pending
            .lock()
            .map_err(|_| BrokerCommandError::ListenerStopped)?
            .take()
            .ok_or(BrokerCommandError::NoPendingResponse)?;
        let expected = expected_for_command(pending.command());
        let ack = BrokerControlContract::decode_response(&expected, response.clone())?;
        pending.respond(ack)
    }

    pub fn shutdown(&mut self) -> Result<(), BrokerCommandError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(endpoint_thread) = self.endpoint_thread.take() {
            endpoint_thread
                .join()
                .map_err(|_| BrokerCommandError::ListenerStopped)?;
        }
        Ok(())
    }
}

impl Drop for BrokerCommandServer {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub struct BrokerCommandClient;

impl BrokerCommandClient {
    pub fn send(
        name: &str,
        request: &BrokerControlRequest,
        startup_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<CommandAck, BrokerCommandError> {
        let expected = expected_for_request(request)?;
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|source| transport_error("create Tokio command runtime", source))?;
        runtime.block_on(send_command(
            name,
            request,
            &expected,
            startup_timeout,
            io_timeout,
        ))
    }
}

fn endpoint_thread_main(
    name: String,
    io_timeout: Duration,
    sender: SyncSender<PendingCommand>,
    shutdown: oneshot::Receiver<()>,
    ready: SyncSender<Result<(), BrokerCommandError>>,
) {
    let runtime = match Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(source) => {
            let _ = ready.send(Err(transport_error(
                "create Tokio command endpoint runtime",
                source,
            )));
            return;
        }
    };

    runtime.block_on(async move {
        let security = match CurrentUserSecurity::new().map_err(BrokerCommandError::from) {
            Ok(security) => security,
            Err(error) => {
                let _ = ready.send(Err(error));
                return;
            }
        };
        let listener = match create_pipe_instance(&name, true, &security) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = ready.send(Err(error));
                return;
            }
        };
        if ready.send(Ok(())).is_err() {
            return;
        }

        run_endpoint(name, listener, security, io_timeout, sender, shutdown).await;
    });
}

async fn run_endpoint(
    name: String,
    mut listener: NamedPipeServer,
    security: CurrentUserSecurity,
    io_timeout: Duration,
    sender: SyncSender<PendingCommand>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let decoders = std::sync::Arc::new(Semaphore::new(MAX_DECODING_CONNECTIONS));
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            result = listener.connect() => {
                match result {
                    Ok(()) => {}
                    Err(_) => {
                        match create_pipe_instance(&name, false, &security) {
                            Ok(next) => listener = next,
                            Err(_) => break,
                        }
                        continue;
                    }
                }

                let next = match create_pipe_instance(&name, false, &security) {
                    Ok(next) => next,
                    Err(_) => break,
                };
                let connected = std::mem::replace(&mut listener, next);
                match decoders.clone().try_acquire_owned() {
                    Ok(permit) => {
                        let connection_sender = sender.clone();
                        connections.spawn(async move {
                            handle_connection(connected, permit, connection_sender, io_timeout).await;
                        });
                    }
                    Err(_) => {
                        let mut connected = connected;
                        let ack = CommandAck::Rejected {
                            command_id: None,
                            reason: CommandRejectionCode::PrimaryBusy,
                        };
                        write_ack(&mut connected, &ack, io_timeout).await;
                    }
                }
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }

    connections.abort_all();
    while connections.join_next().await.is_some() {}
}

async fn handle_connection(
    mut pipe: NamedPipeServer,
    decoder: tokio::sync::OwnedSemaphorePermit,
    sender: SyncSender<PendingCommand>,
    io_timeout: Duration,
) {
    let payload = match read_frame(&mut pipe, io_timeout).await {
        Ok(payload) => payload,
        Err(BrokerError::ControlFrameTooLarge { .. }) => {
            let ack = CommandAck::Rejected {
                command_id: None,
                reason: CommandRejectionCode::FrameTooLarge,
            };
            let _ = write_ack(&mut pipe, &ack, io_timeout).await;
            return;
        }
        Err(_) => return,
    };
    let wire_request = match BrokerControlRequest::decode(payload.as_slice()) {
        Ok(request) => request,
        Err(_) => {
            let ack = CommandAck::Rejected {
                command_id: None,
                reason: CommandRejectionCode::InvalidProtobuf,
            };
            let _ = write_ack(&mut pipe, &ack, io_timeout).await;
            return;
        }
    };
    let command = match BrokerControlContract::decode_request(wire_request.clone()) {
        Ok(command) => command,
        Err(rejection) => {
            let _ = write_ack(&mut pipe, &rejection.into_ack(), io_timeout).await;
            return;
        }
    };
    drop(decoder);

    let command_id = command.command_id().clone();
    let (reply, response) = oneshot::channel();
    let pending = PendingCommand {
        command,
        wire_request,
        reply,
    };
    match sender.try_send(pending) {
        Ok(()) => {
            if let Ok(Ok(ack)) = timeout(io_timeout, response).await {
                let _ = write_ack(&mut pipe, &ack, io_timeout).await;
            }
        }
        Err(TrySendError::Full(_)) => {
            let ack = CommandAck::Rejected {
                command_id: Some(command_id),
                reason: CommandRejectionCode::QueueFull,
            };
            let _ = write_ack(&mut pipe, &ack, io_timeout).await;
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

async fn write_ack(pipe: &mut NamedPipeServer, ack: &CommandAck, io_timeout: Duration) {
    let response = BrokerControlContract::encode_response(ack);
    let _ = write_frame(pipe, &response.encode_to_vec(), io_timeout).await;
}

async fn send_command(
    name: &str,
    request: &BrokerControlRequest,
    expected: &ExpectedAck,
    startup_timeout: Duration,
    io_timeout: Duration,
) -> Result<CommandAck, BrokerCommandError> {
    let pipe = connect(name, startup_timeout).await?;
    let (mut reader, mut writer) = tokio::io::split(pipe);
    let request_payload = request.encode_to_vec();
    let (write_result, read_result) = tokio::join!(
        write_frame(&mut writer, &request_payload, io_timeout),
        read_frame(&mut reader, io_timeout),
    );
    let payload = match read_result {
        Ok(payload) => payload,
        Err(read_error) => {
            write_result?;
            return Err(read_error.into());
        }
    };
    let response = BrokerControlResponse::decode(payload.as_slice())
        .map_err(BrokerCommandError::InvalidResponse)?;
    let ack = BrokerControlContract::decode_response(expected, response)?;
    if matches!(
        ack,
        CommandAck::Rejected {
            reason: CommandRejectionCode::PrimaryBusy,
            ..
        }
    ) {
        Err(BrokerCommandError::PrimaryBusy)
    } else {
        write_result?;
        Ok(ack)
    }
}

async fn connect(
    name: &str,
    startup_timeout: Duration,
) -> Result<NamedPipeClient, BrokerCommandError> {
    let full_name = format!(r"\\.\pipe\{name}");
    let deadline = Instant::now() + startup_timeout;
    let mut observed_busy = false;

    loop {
        match ClientOptions::new().open(&full_name) {
            Ok(pipe) => return Ok(pipe),
            Err(source) => match source.raw_os_error().map(|code| code as u32) {
                Some(ERROR_FILE_NOT_FOUND) => {}
                Some(ERROR_PIPE_BUSY) => observed_busy = true,
                _ => return Err(transport_error("open command pipe", source)),
            },
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return if observed_busy {
                Err(BrokerCommandError::PrimaryBusy)
            } else {
                Err(BrokerCommandError::PrimaryNotReady)
            };
        };
        sleep(remaining.min(RETRY_INTERVAL)).await;
    }
}

fn expected_for_request(request: &BrokerControlRequest) -> Result<ExpectedAck, BrokerCommandError> {
    let command_id = CommandId::parse(request.command_id.clone())
        .map_err(BrokerCommandError::RequestRejected)?;
    match request.command {
        Some(broker_control_request::Command::OpenPath(_)) => Ok(ExpectedAck::open(command_id)),
        Some(broker_control_request::Command::ClosePreview(_)) => {
            Ok(ExpectedAck::close(command_id))
        }
        None => Err(BrokerCommandError::RequestRejected(
            CommandRejectionCode::InvalidCommand,
        )),
    }
}

fn expected_for_command(command: &ValidatedCommand) -> ExpectedAck {
    match command.command() {
        BrokerCommand::Open(_) => ExpectedAck::open(command.command_id().clone()),
        BrokerCommand::Close => ExpectedAck::close(command.command_id().clone()),
    }
}

fn create_pipe_instance(
    name: &str,
    first: bool,
    security: &CurrentUserSecurity,
) -> Result<NamedPipeServer, BrokerCommandError> {
    let full_name = format!(r"\\.\pipe\{name}");
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .max_instances(MAX_PIPE_INSTANCES)
        .in_buffer_size(PIPE_BUFFER_SIZE)
        .out_buffer_size(PIPE_BUFFER_SIZE);
    unsafe {
        options.create_with_security_attributes_raw(
            &full_name,
            security.attributes().cast_mut().cast(),
        )
    }
    .map_err(|source| transport_error("create command pipe", source))
}

fn transport_error(operation: &'static str, source: io::Error) -> BrokerCommandError {
    BrokerError::Windows { operation, source }.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::MaybeUninit;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, OPEN_EXISTING, ReadFile, WriteFile,
    };

    fn test_name(label: &str) -> String {
        format!(
            "PreviewIt.Test.Unit.{label}.{}",
            uuid::Uuid::new_v4().simple()
        )
    }

    fn raw_round_trip(name: &str, bytes: &[u8]) -> Vec<u8> {
        let handle = raw_connect(name);
        write_all(handle, bytes);
        let mut length = [0_u8; 4];
        read_all(handle, &mut length);
        let mut payload = vec![0_u8; u32::from_le_bytes(length) as usize];
        read_all(handle, &mut payload);
        unsafe {
            CloseHandle(handle);
        }
        payload
    }

    fn raw_write_and_close(name: &str, bytes: &[u8]) {
        let handle = raw_connect(name);
        write_all(handle, bytes);
        unsafe {
            CloseHandle(handle);
        }
    }

    fn raw_write_and_hold(name: &str, bytes: &[u8], hold: Duration) {
        let handle = raw_connect(name);
        write_all(handle, bytes);
        thread::sleep(hold);
        unsafe {
            CloseHandle(handle);
        }
    }

    fn raw_connect(name: &str) -> HANDLE {
        let full_name = wide(&format!(r"\\.\pipe\{name}"));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let handle = unsafe {
                CreateFileW(
                    full_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return handle;
            }
            if Instant::now() >= deadline {
                panic!("raw client could not connect");
            }
            thread::yield_now();
        }
    }

    fn write_all(handle: HANDLE, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let mut written = MaybeUninit::uninit();
            let result = unsafe {
                WriteFile(
                    handle,
                    bytes.as_ptr().cast(),
                    bytes.len() as u32,
                    written.as_mut_ptr(),
                    null_mut(),
                )
            };
            assert_ne!(result, 0);
            let written = unsafe { written.assume_init() } as usize;
            assert!(written > 0);
            bytes = &bytes[written..];
        }
    }

    fn read_all(handle: HANDLE, mut bytes: &mut [u8]) {
        while !bytes.is_empty() {
            let mut read = MaybeUninit::uninit();
            let result = unsafe {
                ReadFile(
                    handle,
                    bytes.as_mut_ptr().cast(),
                    bytes.len() as u32,
                    read.as_mut_ptr(),
                    null_mut(),
                )
            };
            assert_ne!(result, 0);
            let read = unsafe { read.assume_init() } as usize;
            assert!(read > 0);
            bytes = &mut bytes[read..];
        }
    }

    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

    fn close_request(command_id: &str) -> BrokerControlRequest {
        BrokerControlRequest {
            protocol_major: previewit_protocol::PROTOCOL_MAJOR,
            protocol_minor: previewit_protocol::PROTOCOL_MINOR,
            command_id: command_id.into(),
            command: Some(broker_control_request::Command::ClosePreview(
                previewit_protocol::v0::ClosePreview {},
            )),
        }
    }

    #[test]
    fn oversized_declared_frame_returns_stable_rejection() {
        let name = test_name("oversized");
        let _server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_secs(2))
                .unwrap();
        let response = raw_round_trip(&name, &((MAX_CONTROL_FRAME as u32 + 1).to_le_bytes()));

        let response = BrokerControlResponse::decode(response.as_slice()).unwrap();
        assert_eq!(response.error_code, "frame-too-large");
    }

    #[test]
    fn malformed_protobuf_returns_stable_rejection() {
        let name = test_name("malformed");
        let _server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_secs(2))
                .unwrap();
        let response = raw_round_trip(&name, &frame(&[0xff]));

        let response = BrokerControlResponse::decode(response.as_slice()).unwrap();
        assert_eq!(response.error_code, "invalid-protobuf");
    }

    #[test]
    fn oversized_command_id_returns_stable_rejection() {
        let name = test_name("command-id");
        let _server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_secs(2))
                .unwrap();
        let request = close_request(&"x".repeat(129));
        let response = raw_round_trip(&name, &frame(&request.encode_to_vec()));

        let response = BrokerControlResponse::decode(response.as_slice()).unwrap();
        assert_eq!(response.error_code, "invalid-command-id");
        assert!(response.command_id.is_empty());
    }

    #[test]
    fn truncated_frame_does_not_stop_the_endpoint() {
        let name = test_name("truncated");
        let server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_millis(200))
                .unwrap();
        raw_write_and_close(&name, &[10, 0, 0, 0, 1, 2]);
        let client_name = name.clone();
        let client = thread::spawn(move || {
            BrokerCommandClient::send(
                &client_name,
                &close_request("command-after-truncated"),
                Duration::from_secs(2),
                Duration::from_secs(2),
            )
        });

        let pending = server.receive().unwrap();
        let command_id = pending.command().command_id().clone();
        pending
            .respond(CommandAck::CloseAccepted { command_id })
            .unwrap();
        assert!(matches!(
            client.join().unwrap().unwrap(),
            CommandAck::CloseAccepted { .. }
        ));
    }

    #[test]
    fn response_write_does_not_wait_for_client_to_read() {
        let name = test_name("write-deadline");
        let server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_millis(100))
                .unwrap();
        let request = close_request("command-hold");
        let client_name = name.clone();
        let client = thread::spawn(move || {
            raw_write_and_hold(
                &client_name,
                &frame(&request.encode_to_vec()),
                Duration::from_millis(500),
            );
        });

        let pending = server.receive().unwrap();
        let command_id = pending.command().command_id().clone();
        let started = Instant::now();
        pending
            .respond(CommandAck::CloseAccepted { command_id })
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(250));
        client.join().unwrap();
    }

    #[test]
    fn write_timeout_exposes_stable_code() {
        let error = BrokerCommandError::Transport(BrokerError::WriteTimeout);
        assert_eq!(error.code(), "write-timeout");
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
