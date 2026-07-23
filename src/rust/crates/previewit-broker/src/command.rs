use std::io;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use previewit_protocol::MAX_CONTROL_FRAME;
use previewit_protocol::v0::{BrokerControlRequest, BrokerControlResponse, broker_control_request};
use prost::Message;
use thiserror::Error;
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, GetLastError,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::pipe::{BrokerError, connect_pipe, read_frame, write_frame};
use crate::windows_security::{CurrentUserSecurity, OwnedHandle, WindowsSecurityError};

const PROTOCOL_MAJOR: u32 = 0;
const PROTOCOL_MINOR: u32 = 1;
const MAX_COMMAND_ID_BYTES: usize = 128;
const MAX_PATH_UTF16_UNITS: usize = 32_767;
const MAX_PENDING_COMMANDS: u32 = 8;
const PIPE_BUFFER_SIZE: u32 = (MAX_CONTROL_FRAME + 4) as u32;
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

struct PipeHandle(OwnedHandle);

// Overlapped Named Pipe handles may move between listener and consumer threads. Keeping the
// marker on this wrapper avoids making thread-affine Mutex ownership transferable.
unsafe impl Send for PipeHandle {}

impl PipeHandle {
    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0.raw()
    }
}

#[derive(Debug, Error)]
pub enum BrokerCommandError {
    #[error("the primary Broker did not become ready before the startup deadline")]
    PrimaryNotReady,
    #[error("unsupported Broker protocol major {major}")]
    ProtocolMajorMismatch { major: u32 },
    #[error("unsupported Broker protocol minor {minor}")]
    ProtocolMinorMismatch { minor: u32 },
    #[error("the Broker command id is invalid")]
    InvalidCommandId,
    #[error("the Broker control request has no command")]
    InvalidCommand,
    #[error("the preview path is not valid UTF-16LE")]
    InvalidUtf16,
    #[error("the preview path contains an embedded NUL")]
    EmbeddedNul,
    #[error("the preview path exceeds the Windows long-path limit")]
    PathTooLong,
    #[error("invalid Broker control request: {0}")]
    InvalidRequest(#[source] prost::DecodeError),
    #[error("invalid Broker control response: {0}")]
    InvalidResponse(#[source] prost::DecodeError),
    #[error("the Broker command listener stopped")]
    ListenerStopped,
    #[error("there is no pending Broker command response")]
    NoPendingResponse,
    #[error(transparent)]
    Transport(#[from] BrokerError),
    #[error("{operation} failed: {source}")]
    Windows {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

impl BrokerCommandError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PrimaryNotReady => "primary-not-ready",
            Self::ProtocolMajorMismatch { .. } => "protocol-major-mismatch",
            Self::ProtocolMinorMismatch { .. } => "protocol-minor-mismatch",
            Self::InvalidCommandId => "invalid-command-id",
            Self::InvalidCommand => "invalid-command",
            Self::InvalidUtf16 => "invalid-utf16",
            Self::EmbeddedNul => "embedded-nul",
            Self::PathTooLong => "path-too-long",
            Self::InvalidRequest(_) => "invalid-protobuf",
            Self::InvalidResponse(_) => "invalid-response",
            Self::ListenerStopped => "listener-stopped",
            Self::NoPendingResponse => "no-pending-response",
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
            Self::Windows { .. } => "windows-error",
        }
    }
}

impl From<WindowsSecurityError> for BrokerCommandError {
    fn from(error: WindowsSecurityError) -> Self {
        let WindowsSecurityError::Windows { operation, source } = error;
        Self::Windows { operation, source }
    }
}

struct PendingCommand {
    request: BrokerControlRequest,
    handle: PipeHandle,
}

enum Incoming {
    Request(PendingCommand),
    Rejected(BrokerCommandError),
}

enum DecodedRequest {
    Valid(BrokerControlRequest),
    Invalid {
        command_id: String,
        error: BrokerCommandError,
    },
}

pub struct BrokerCommandServer {
    receiver: Mutex<Receiver<Incoming>>,
    pending_response: Mutex<Option<PipeHandle>>,
    startup_timeout: Duration,
    io_timeout: Duration,
    stop: Arc<AtomicBool>,
}

impl BrokerCommandServer {
    pub fn create(
        name: &str,
        startup_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<Self, BrokerCommandError> {
        let initial = create_pipe_instance(name, true)?;
        let (sender, receiver) = mpsc::sync_channel(MAX_PENDING_COMMANDS as usize);
        let stop = Arc::new(AtomicBool::new(false));
        let listener_stop = Arc::clone(&stop);
        let listener_name = name.to_owned();

        thread::spawn(move || {
            listen(
                listener_name,
                initial,
                startup_timeout,
                io_timeout,
                sender,
                listener_stop,
            );
        });

        Ok(Self {
            receiver: Mutex::new(receiver),
            pending_response: Mutex::new(None),
            startup_timeout,
            io_timeout,
            stop,
        })
    }

    pub fn receive_request(&self) -> Result<BrokerControlRequest, BrokerCommandError> {
        let incoming = self
            .receiver
            .lock()
            .map_err(|_| BrokerCommandError::ListenerStopped)?
            .recv_timeout(self.startup_timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => BrokerError::StartupTimeout.into(),
                RecvTimeoutError::Disconnected => BrokerCommandError::ListenerStopped,
            })?;

        match incoming {
            Incoming::Request(pending) => {
                let mut active = self
                    .pending_response
                    .lock()
                    .map_err(|_| BrokerCommandError::ListenerStopped)?;
                *active = Some(pending.handle);
                Ok(pending.request)
            }
            Incoming::Rejected(error) => Err(error),
        }
    }

    pub fn send_response(
        &self,
        response: &BrokerControlResponse,
    ) -> Result<(), BrokerCommandError> {
        let handle = self
            .pending_response
            .lock()
            .map_err(|_| BrokerCommandError::ListenerStopped)?
            .take()
            .ok_or(BrokerCommandError::NoPendingResponse)?;
        send_response(&handle, response, self.io_timeout)
    }
}

impl Drop for BrokerCommandServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub struct BrokerCommandClient;

impl BrokerCommandClient {
    pub fn send(
        name: &str,
        request: &BrokerControlRequest,
        startup_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<BrokerControlResponse, BrokerCommandError> {
        let handle = connect(name, startup_timeout)?;
        write_frame(handle.raw(), &request.encode_to_vec(), io_timeout)?;
        let payload = read_frame(handle.raw(), io_timeout)?;
        BrokerControlResponse::decode(payload.as_slice())
            .map_err(BrokerCommandError::InvalidResponse)
    }
}

fn listen(
    name: String,
    mut listener: PipeHandle,
    startup_timeout: Duration,
    io_timeout: Duration,
    sender: SyncSender<Incoming>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        match connect_pipe(listener.raw(), startup_timeout) {
            Ok(()) => {}
            Err(BrokerError::StartupTimeout) => {
                if let Ok(next) = create_pipe_instance(&name, false) {
                    listener = next;
                    continue;
                }
                break;
            }
            Err(error) => {
                let _ = sender.send(Incoming::Rejected(error.into()));
                break;
            }
        }

        let incoming = decode_request(&listener, io_timeout);
        let connected = listener;

        match incoming {
            DecodedRequest::Valid(request) => {
                let pending = Incoming::Request(PendingCommand {
                    request,
                    handle: connected,
                });
                match sender.try_send(pending) {
                    Ok(()) => {}
                    Err(TrySendError::Full(Incoming::Request(pending))) => {
                        let response = rejected_response(&pending.request.command_id, "queue-full");
                        let _ = send_response(&pending.handle, &response, io_timeout);
                        drop(pending);
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                    Err(TrySendError::Full(Incoming::Rejected(_))) => unreachable!(),
                }
            }
            DecodedRequest::Invalid { command_id, error } => {
                let response = rejected_response(&command_id, rejection_code(&error));
                let _ = send_response(&connected, &response, io_timeout);
                drop(connected);
                match sender.try_send(Incoming::Rejected(error)) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
        }

        match create_pipe_instance(&name, false) {
            Ok(next) => listener = next,
            Err(error) => {
                let _ = sender.try_send(Incoming::Rejected(error));
                break;
            }
        }
    }
}

fn decode_request(handle: &PipeHandle, io_timeout: Duration) -> DecodedRequest {
    let payload = match read_frame(handle.raw(), io_timeout) {
        Ok(payload) => payload,
        Err(error) => {
            return DecodedRequest::Invalid {
                command_id: String::new(),
                error: error.into(),
            };
        }
    };
    let request = match BrokerControlRequest::decode(payload.as_slice()) {
        Ok(request) => request,
        Err(error) => {
            return DecodedRequest::Invalid {
                command_id: String::new(),
                error: BrokerCommandError::InvalidRequest(error),
            };
        }
    };
    match validate_request(&request) {
        Ok(()) => DecodedRequest::Valid(request),
        Err(error) => {
            let command_id = if matches!(error, BrokerCommandError::InvalidCommandId) {
                String::new()
            } else {
                request.command_id.clone()
            };
            DecodedRequest::Invalid { command_id, error }
        }
    }
}

fn validate_request(request: &BrokerControlRequest) -> Result<(), BrokerCommandError> {
    if request.protocol_major != PROTOCOL_MAJOR {
        return Err(BrokerCommandError::ProtocolMajorMismatch {
            major: request.protocol_major,
        });
    }
    if request.protocol_minor != PROTOCOL_MINOR {
        return Err(BrokerCommandError::ProtocolMinorMismatch {
            minor: request.protocol_minor,
        });
    }
    if request.command_id.is_empty()
        || request.command_id.len() > MAX_COMMAND_ID_BYTES
        || !request
            .command_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(BrokerCommandError::InvalidCommandId);
    }

    match request.command.as_ref() {
        Some(broker_control_request::Command::OpenPath(open)) => validate_path(&open.path_utf16le),
        Some(broker_control_request::Command::ClosePreview(_)) => Ok(()),
        None => Err(BrokerCommandError::InvalidCommand),
    }
}

fn validate_path(bytes: &[u8]) -> Result<(), BrokerCommandError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(BrokerCommandError::InvalidUtf16);
    }

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    if units.len() > MAX_PATH_UTF16_UNITS {
        return Err(BrokerCommandError::PathTooLong);
    }
    if units.contains(&0) {
        return Err(BrokerCommandError::EmbeddedNul);
    }
    if char::decode_utf16(units).any(|character| character.is_err()) {
        return Err(BrokerCommandError::InvalidUtf16);
    }
    Ok(())
}

fn create_pipe_instance(name: &str, first: bool) -> Result<PipeHandle, BrokerCommandError> {
    let full_name = wide(&format!(r"\\.\pipe\{name}"));
    let security = CurrentUserSecurity::new()?;
    let first_flag = if first {
        FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        0
    };
    let handle = unsafe {
        CreateNamedPipeW(
            full_name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | first_flag,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            MAX_PENDING_COMMANDS + 1,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            security.attributes(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(last_error("CreateNamedPipeW(command)"))
    } else {
        Ok(PipeHandle(OwnedHandle::new(handle)))
    }
}

fn send_response(
    handle: &PipeHandle,
    response: &BrokerControlResponse,
    io_timeout: Duration,
) -> Result<(), BrokerCommandError> {
    write_frame(handle.raw(), &response.encode_to_vec(), io_timeout)?;
    Ok(())
}

fn rejected_response(command_id: &str, error_code: &str) -> BrokerControlResponse {
    BrokerControlResponse {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: command_id.to_owned(),
        accepted: false,
        request_id: String::new(),
        error_code: error_code.to_owned(),
    }
}

fn rejection_code(error: &BrokerCommandError) -> &'static str {
    error.code()
}

fn connect(name: &str, startup_timeout: Duration) -> Result<OwnedHandle, BrokerCommandError> {
    let full_name = wide(&format!(r"\\.\pipe\{name}"));
    let deadline = Instant::now() + startup_timeout;

    loop {
        let handle = unsafe {
            CreateFileW(
                full_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(OwnedHandle::new(handle));
        }

        let error = unsafe { GetLastError() };
        if error != ERROR_FILE_NOT_FOUND && error != ERROR_PIPE_BUSY {
            return Err(error_code("CreateFileW(command)", error));
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(BrokerCommandError::PrimaryNotReady);
        };
        thread::sleep(remaining.min(RETRY_INTERVAL));
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn last_error(operation: &'static str) -> BrokerCommandError {
    BrokerCommandError::Windows {
        operation,
        source: io::Error::last_os_error(),
    }
}

fn error_code(operation: &'static str, code: u32) -> BrokerCommandError {
    BrokerCommandError::Windows {
        operation,
        source: io::Error::from_raw_os_error(code as i32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::MaybeUninit;
    use std::ptr::null_mut;
    use std::thread;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile};

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
            thread::sleep(RETRY_INTERVAL);
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

    #[test]
    fn oversized_declared_frame_returns_stable_rejection() {
        let name = test_name("oversized");
        let server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_secs(2))
                .unwrap();
        let client_name = name.clone();
        let client = thread::spawn(move || {
            raw_round_trip(
                &client_name,
                &((MAX_CONTROL_FRAME as u32 + 1).to_le_bytes()),
            )
        });

        assert!(matches!(
            server.receive_request(),
            Err(BrokerCommandError::Transport(
                BrokerError::ControlFrameTooLarge { .. }
            ))
        ));
        let response = BrokerControlResponse::decode(client.join().unwrap().as_slice()).unwrap();
        assert_eq!(response.error_code, "frame-too-large");
    }

    #[test]
    fn malformed_protobuf_returns_stable_rejection() {
        let name = test_name("malformed");
        let server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_secs(2))
                .unwrap();
        let client_name = name.clone();
        let client = thread::spawn(move || raw_round_trip(&client_name, &frame(&[0xff])));

        assert!(matches!(
            server.receive_request(),
            Err(BrokerCommandError::InvalidRequest(_))
        ));
        let response = BrokerControlResponse::decode(client.join().unwrap().as_slice()).unwrap();
        assert_eq!(response.error_code, "invalid-protobuf");
    }

    #[test]
    fn oversized_command_id_returns_stable_rejection() {
        let name = test_name("command-id");
        let server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_secs(2))
                .unwrap();
        let request = BrokerControlRequest {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            command_id: "x".repeat(MAX_COMMAND_ID_BYTES + 1),
            command: Some(broker_control_request::Command::ClosePreview(
                previewit_protocol::v0::ClosePreview {},
            )),
        };
        let client_name = name.clone();
        let client =
            thread::spawn(move || raw_round_trip(&client_name, &frame(&request.encode_to_vec())));

        assert!(matches!(
            server.receive_request(),
            Err(BrokerCommandError::InvalidCommandId)
        ));
        let response = BrokerControlResponse::decode(client.join().unwrap().as_slice()).unwrap();
        assert_eq!(response.error_code, "invalid-command-id");
    }

    #[test]
    fn truncated_frame_is_reported_without_waiting_forever() {
        let name = test_name("truncated");
        let server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_millis(200))
                .unwrap();
        let client_name = name.clone();
        let client = thread::spawn(move || {
            raw_write_and_close(&client_name, &[10, 0, 0, 0, 1, 2]);
        });

        let error = server.receive_request().unwrap_err();
        assert!(matches!(
            error,
            BrokerCommandError::Transport(BrokerError::TruncatedControlFrame)
        ));
        assert_eq!(rejection_code(&error), "truncated-frame");
        client.join().unwrap();
    }

    #[test]
    fn response_write_does_not_wait_for_client_to_read() {
        let name = test_name("write-deadline");
        let server =
            BrokerCommandServer::create(&name, Duration::from_secs(2), Duration::from_millis(100))
                .unwrap();
        let request = BrokerControlRequest {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            command_id: "command-hold".into(),
            command: Some(broker_control_request::Command::ClosePreview(
                previewit_protocol::v0::ClosePreview {},
            )),
        };
        let client_name = name.clone();
        let client = thread::spawn(move || {
            raw_write_and_hold(
                &client_name,
                &frame(&request.encode_to_vec()),
                Duration::from_millis(500),
            );
        });

        let request = server.receive_request().unwrap();
        let started = Instant::now();
        server
            .send_response(&rejected_response(&request.command_id, "test-response"))
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(250));
        client.join().unwrap();
    }

    #[test]
    fn write_timeout_exposes_stable_code() {
        let error = BrokerCommandError::Transport(BrokerError::WriteTimeout);
        assert_eq!(error.code(), "write-timeout");
    }
}
