use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr::null_mut;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use previewit_broker::{
    BrokerCommand, BrokerCommandClient, BrokerCommandError, BrokerCommandServer, BrokerEvent,
    CommandAck, CommandRejectionCode, EventSink,
};
use previewit_protocol::v0::{
    BrokerControlRequest, BrokerControlResponse, ClosePreview, OpenPath, broker_control_request,
};
use previewit_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR, encode_frame};
use prost::Message;
use uuid::Uuid;
use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};

const TIMEOUT: Duration = Duration::from_secs(2);
const QUEUE_CAPACITY: usize = 8;
const QUEUE_FILL_PACING: Duration = Duration::from_millis(25);

type ClientResult = Result<CommandAck, BrokerCommandError>;

struct QueueFullSignal(Mutex<Sender<()>>);

impl EventSink for QueueFullSignal {
    fn emit(&self, event: &BrokerEvent) {
        if matches!(event, BrokerEvent::CommandQueueFull { .. }) {
            let _ = self.0.lock().unwrap().send(());
        }
    }
}

fn pipe_name(label: &str) -> String {
    format!("PreviewIt.Test.{label}.{}", Uuid::new_v4().simple())
}

fn open_path_request() -> BrokerControlRequest {
    request_with_path(
        "command-1",
        r"C:\fixtures\preview.txt"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect(),
    )
}

fn close_request(command_id: &str) -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: command_id.into(),
        command: Some(broker_control_request::Command::ClosePreview(
            ClosePreview {},
        )),
    }
}

fn accepted(command_id: &str, request_id: &str) -> BrokerControlResponse {
    BrokerControlResponse {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: command_id.into(),
        accepted: true,
        request_id: request_id.into(),
        error_code: String::new(),
    }
}

fn request_with_path(command_id: &str, path_utf16le: Vec<u8>) -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: command_id.into(),
        command: Some(broker_control_request::Command::OpenPath(OpenPath {
            path_utf16le,
        })),
    }
}

fn hold_partial_frame(
    name: String,
    ready: Sender<()>,
    release: Receiver<()>,
    connect_timeout: Duration,
) -> bool {
    let Some(mut pipe) = connect_raw_client(&name, connect_timeout) else {
        return false;
    };
    pipe.write_all(&[10, 0, 0, 0, 1]).unwrap();
    pipe.flush().unwrap();
    ready.send(()).unwrap();
    let _ = release.recv_timeout(TIMEOUT);
    true
}

fn connect_raw_client(name: &str, connect_timeout: Duration) -> Option<File> {
    let pipe_path = format!(r"\\.\pipe\{name}");
    let deadline = Instant::now() + connect_timeout;
    loop {
        match OpenOptions::new().read(true).write(true).open(&pipe_path) {
            Ok(pipe) => return Some(pipe),
            Err(_) if Instant::now() < deadline => std::thread::yield_now(),
            Err(_) => return None,
        }
    }
}

fn spawn_client(
    name: &str,
    request: BrokerControlRequest,
) -> std::thread::JoinHandle<ClientResult> {
    let name = name.to_owned();
    std::thread::spawn(move || BrokerCommandClient::send(&name, &request, TIMEOUT, TIMEOUT))
}

fn fill_pending_queue(
    name: &str,
    label: &str,
    count: usize,
) -> Vec<std::thread::JoinHandle<ClientResult>> {
    (0..count)
        .map(|index| {
            let client = spawn_client(name, close_request(&format!("command-{label}-{index}")));
            // Pacing keeps this setup below decoder saturation. The typed
            // CommandQueueFull event, not the sleep, is the readiness proof.
            std::thread::sleep(QUEUE_FILL_PACING);
            client
        })
        .collect()
}

fn raw_round_trip(name: &str, request: &BrokerControlRequest) -> BrokerControlResponse {
    let mut pipe = connect_raw_client(name, TIMEOUT).expect("raw client could not connect");
    pipe.write_all(&encode_frame(&request.encode_to_vec()).unwrap())
        .unwrap();
    pipe.flush().unwrap();
    read_response(&mut pipe)
}

fn read_response(pipe: &mut File) -> BrokerControlResponse {
    let mut length = [0_u8; 4];
    pipe.read_exact(&mut length).unwrap();
    let mut payload = vec![0_u8; u32::from_le_bytes(length) as usize];
    pipe.read_exact(&mut payload).unwrap();
    BrokerControlResponse::decode(payload.as_slice()).unwrap()
}

#[test]
fn one_slow_client_does_not_block_a_normal_command() {
    let name = pipe_name("slow-client");
    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let (state_done_tx, state_done_rx) = mpsc::channel();
    let state = std::thread::spawn(move || {
        let pending = server.receive()?;
        let command_id = pending.command().command_id().clone();
        assert!(matches!(pending.command().command(), BrokerCommand::Close));
        pending.respond(CommandAck::CloseAccepted { command_id })?;
        state_done_rx
            .recv_timeout(TIMEOUT)
            .map_err(|_| BrokerCommandError::ListenerStopped)?;
        Ok::<_, BrokerCommandError>(())
    });

    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let slow_name = name.clone();
    let slow =
        std::thread::spawn(move || hold_partial_frame(slow_name, ready_tx, release_rx, TIMEOUT));
    ready_rx.recv_timeout(TIMEOUT).unwrap();

    let started = Instant::now();
    let response = BrokerCommandClient::send(
        &name,
        &close_request("command-normal"),
        Duration::from_millis(500),
        Duration::from_millis(500),
    );
    let elapsed = started.elapsed();

    state_done_tx.send(()).unwrap();
    release_tx.send(()).unwrap();
    assert!(slow.join().unwrap());
    state.join().unwrap().unwrap();

    assert!(elapsed < Duration::from_millis(500));
    assert!(matches!(
        response.unwrap(),
        CommandAck::CloseAccepted { .. }
    ));
}

#[test]
fn exhausted_decode_slots_report_primary_busy() {
    const DECODE_SLOTS: usize = 4;

    let name = pipe_name("decode-busy");
    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let mut releases = Vec::new();
    let mut clients = Vec::new();
    for _ in 0..DECODE_SLOTS {
        let (release_tx, release_rx) = mpsc::channel();
        releases.push(release_tx);
        let client_name = name.clone();
        let client_ready = ready_tx.clone();
        clients.push(std::thread::spawn(move || {
            hold_partial_frame(
                client_name,
                client_ready,
                release_rx,
                Duration::from_millis(750),
            )
        }));
    }
    drop(ready_tx);

    let readiness_deadline = Instant::now() + Duration::from_millis(750);
    let mut ready_count = 0;
    while ready_count < DECODE_SLOTS {
        let Some(remaining) = readiness_deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if ready_rx.recv_timeout(remaining).is_err() {
            break;
        }
        ready_count += 1;
    }

    let busy = if ready_count == DECODE_SLOTS {
        BrokerCommandClient::send(
            &name,
            &close_request("command-busy"),
            Duration::from_millis(250),
            Duration::from_millis(250),
        )
        .expect_err("an exhausted endpoint must reject before routing")
    } else {
        BrokerCommandError::PrimaryNotReady
    };

    for release in releases {
        let _ = release.send(());
    }
    for client in clients {
        let _ = client.join();
    }

    assert_eq!(ready_count, DECODE_SLOTS);
    assert!(
        matches!(busy, BrokerCommandError::PrimaryBusy),
        "unexpected error: {busy:?}"
    );
    assert_eq!(busy.code(), "primary-busy");

    let (state_done_tx, state_done_rx) = mpsc::channel();
    let state = std::thread::spawn(move || {
        let pending = server.receive().unwrap();
        let command_id = pending.command().command_id().clone();
        pending
            .respond(CommandAck::CloseAccepted { command_id })
            .unwrap();
        state_done_rx.recv_timeout(TIMEOUT).unwrap();
    });
    let recovery_deadline = Instant::now() + TIMEOUT;
    let mut attempt = 0;
    let recovered = loop {
        let request = close_request(&format!("command-recovered-{attempt}"));
        match BrokerCommandClient::send(&name, &request, TIMEOUT, TIMEOUT) {
            Ok(ack) => break ack,
            Err(BrokerCommandError::PrimaryBusy) if Instant::now() < recovery_deadline => {
                attempt += 1;
                std::thread::yield_now();
            }
            Err(error) => panic!("decoder capacity did not recover: {error:?}"),
        }
    };
    state_done_tx.send(()).unwrap();
    state.join().unwrap();
    assert!(matches!(recovered, CommandAck::CloseAccepted { .. }));
}

#[test]
fn dropping_server_allows_immediate_same_name_recreation() {
    let name = pipe_name("drop-recreate");
    drop(BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap());

    BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
}

#[test]
fn client_rejects_mismatched_response_id() {
    let name = pipe_name("wrong-response-id");
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let mut pipe = create_raw_server(&server_name);
        ready_tx.send(()).unwrap();
        connect_raw_server(&pipe);
        let mut length = [0_u8; 4];
        pipe.read_exact(&mut length).unwrap();
        let mut request = vec![0_u8; u32::from_le_bytes(length) as usize];
        pipe.read_exact(&mut request).unwrap();
        let response = accepted("different-command", "request-1");
        pipe.write_all(&encode_frame(&response.encode_to_vec()).unwrap())
            .unwrap();
        pipe.flush().unwrap();
    });
    ready_rx.recv().unwrap();

    let error = BrokerCommandClient::send(&name, &open_path_request(), TIMEOUT, TIMEOUT)
        .expect_err("a response for another command must be rejected");

    assert_eq!(error.code(), "response-command-mismatch");
    server.join().unwrap();
}

#[test]
fn broker_control_round_trips_open_path() {
    let name = pipe_name("open");
    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let client_name = name.clone();
    let client = std::thread::spawn(move || {
        BrokerCommandClient::send(&client_name, &open_path_request(), TIMEOUT, TIMEOUT)
    });
    let pending = server.receive().unwrap();
    assert!(matches!(
        pending.command().command(),
        BrokerCommand::Open(_)
    ));
    let command_id = pending.command().command_id().clone();
    pending
        .respond(CommandAck::OpenAccepted {
            command_id,
            request_id: "request-1".into(),
        })
        .unwrap();

    let response = client.join().unwrap().unwrap();

    assert!(matches!(
        response,
        CommandAck::OpenAccepted { ref request_id, .. } if request_id == "request-1"
    ));
}

#[test]
fn broker_control_round_trips_close_preview() {
    let name = pipe_name("close");
    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let client_name = name.clone();
    let client = std::thread::spawn(move || {
        BrokerCommandClient::send(
            &client_name,
            &close_request("command-close"),
            TIMEOUT,
            TIMEOUT,
        )
    });
    let pending = server.receive().unwrap();
    assert!(matches!(pending.command().command(), BrokerCommand::Close));
    let command_id = pending.command().command_id().clone();
    pending
        .respond(CommandAck::CloseAccepted { command_id })
        .unwrap();

    let response = client.join().unwrap().unwrap();

    assert!(matches!(response, CommandAck::CloseAccepted { .. }));
}

#[test]
fn wrong_protocol_major_returns_stable_rejection() {
    let name = pipe_name("wrong-major");
    let _server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let mut request = open_path_request();
    request.protocol_major = 1;

    let response = BrokerCommandClient::send(&name, &request, TIMEOUT, TIMEOUT).unwrap();

    assert!(matches!(
        response,
        CommandAck::Rejected {
            reason: CommandRejectionCode::ProtocolMajorMismatch,
            ..
        }
    ));
}

#[test]
fn invalid_utf16_path_returns_stable_rejection() {
    let name = pipe_name("invalid-utf16");
    let _server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();

    let response = BrokerCommandClient::send(
        &name,
        &request_with_path("command-invalid", vec![0x00]),
        TIMEOUT,
        TIMEOUT,
    )
    .unwrap();

    assert!(matches!(
        response,
        CommandAck::Rejected {
            reason: CommandRejectionCode::InvalidUtf16,
            ..
        }
    ));
}

#[test]
fn embedded_nul_path_returns_stable_rejection() {
    let name = pipe_name("embedded-nul");
    let _server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let path = "C:\\fixtures\\bad\0.txt";

    let response = BrokerCommandClient::send(
        &name,
        &request_with_path(
            "command-nul",
            path.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ),
        TIMEOUT,
        TIMEOUT,
    )
    .unwrap();

    assert!(matches!(
        response,
        CommandAck::Rejected {
            reason: CommandRejectionCode::EmbeddedNul,
            ..
        }
    ));
}

#[test]
fn overlong_path_returns_stable_rejection_without_echoing_path() {
    let name = pipe_name("overlong-path");
    let _server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let path = "C:\\".to_owned() + &"x".repeat(32_768);

    let response = BrokerCommandClient::send(
        &name,
        &request_with_path(
            "command-long",
            path.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ),
        TIMEOUT,
        TIMEOUT,
    )
    .unwrap();

    assert!(matches!(
        response,
        CommandAck::Rejected {
            reason: CommandRejectionCode::PathTooLong,
            ..
        }
    ));
}

#[test]
fn missing_command_id_returns_stable_rejection() {
    let name = pipe_name("missing-command-id");
    let _server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();

    let response = raw_round_trip(
        &name,
        &request_with_path(
            "",
            r"C:\fixtures\preview.txt"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
    );

    assert!(!response.accepted);
    assert_eq!(response.error_code, "invalid-command-id");
    assert!(response.command_id.is_empty());
}

#[test]
fn connect_deadline_returns_primary_not_ready() {
    let error = BrokerCommandClient::send(
        &pipe_name("no-primary"),
        &open_path_request(),
        Duration::from_millis(100),
        TIMEOUT,
    )
    .expect_err("missing primary must hit startup deadline");

    assert!(matches!(error, BrokerCommandError::PrimaryNotReady));
    assert_eq!(error.code(), "primary-not-ready");
}

#[test]
fn response_deadline_is_bounded() {
    let name = pipe_name("response-deadline");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        let _pending = server.receive().unwrap();
        std::thread::sleep(Duration::from_millis(250));
    });

    let error = BrokerCommandClient::send(
        &name,
        &open_path_request(),
        TIMEOUT,
        Duration::from_millis(100),
    )
    .expect_err("missing response must hit the IO deadline");

    assert!(matches!(error, BrokerCommandError::Transport(_)));
    assert_eq!(error.code(), "read-timeout");
    server.join().unwrap();
}

#[test]
fn full_pending_queue_returns_stable_rejection() {
    let name = pipe_name("queue-full");
    let (queue_full_tx, queue_full_rx) = mpsc::channel();
    let events: Arc<dyn EventSink> = Arc::new(QueueFullSignal(Mutex::new(queue_full_tx)));
    let server =
        BrokerCommandServer::create_with_event_sink(&name, TIMEOUT, TIMEOUT, events).unwrap();
    let clients = fill_pending_queue(&name, "queued", QUEUE_CAPACITY + 1);

    queue_full_rx.recv_timeout(TIMEOUT).unwrap();
    for _ in 0..QUEUE_CAPACITY {
        let pending = server.receive().unwrap();
        let command_id = pending.command().command_id().clone();
        pending
            .respond(CommandAck::CloseAccepted { command_id })
            .unwrap();
    }

    let responses: Vec<_> = clients
        .into_iter()
        .map(|client| client.join().unwrap().unwrap())
        .collect();
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(response, CommandAck::CloseAccepted { .. }))
            .count(),
        QUEUE_CAPACITY
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(
                response,
                CommandAck::Rejected {
                    reason: CommandRejectionCode::QueueFull,
                    ..
                }
            ))
            .count(),
        1
    );

    let client_name = name.clone();
    let recovered = std::thread::spawn(move || {
        BrokerCommandClient::send(
            &client_name,
            &close_request("command-queue-recovered"),
            TIMEOUT,
            TIMEOUT,
        )
    });
    let pending = server.receive().unwrap();
    let command_id = pending.command().command_id().clone();
    pending
        .respond(CommandAck::CloseAccepted { command_id })
        .unwrap();
    assert!(matches!(
        recovered.join().unwrap().unwrap(),
        CommandAck::CloseAccepted { .. }
    ));
}

#[test]
fn combined_capacity_rejects_without_stopping_endpoint() {
    const DECODER_SLOTS: usize = 4;

    let name = pipe_name("combined-capacity");
    let (queue_full_tx, queue_full_rx) = mpsc::channel();
    let events: Arc<dyn EventSink> = Arc::new(QueueFullSignal(Mutex::new(queue_full_tx)));
    let server =
        BrokerCommandServer::create_with_event_sink(&name, TIMEOUT, TIMEOUT, events).unwrap();

    let active_client = spawn_client(&name, close_request("command-active"));
    let active = server.receive().unwrap();

    let queued_clients = fill_pending_queue(&name, "combined", QUEUE_CAPACITY + 1);
    queue_full_rx.recv_timeout(TIMEOUT).unwrap();

    let (decoder_ready_tx, decoder_ready_rx) = mpsc::channel();
    let mut decoder_releases = Vec::with_capacity(DECODER_SLOTS);
    let mut decoder_clients = Vec::with_capacity(DECODER_SLOTS);
    for _ in 0..DECODER_SLOTS {
        let (release_tx, release_rx) = mpsc::channel();
        decoder_releases.push(release_tx);
        let client_name = name.clone();
        let ready = decoder_ready_tx.clone();
        decoder_clients.push(std::thread::spawn(move || {
            hold_partial_frame(client_name, ready, release_rx, TIMEOUT)
        }));
    }
    drop(decoder_ready_tx);
    for _ in 0..DECODER_SLOTS {
        decoder_ready_rx.recv_timeout(TIMEOUT).unwrap();
    }

    let saturated =
        BrokerCommandClient::send(&name, &close_request("command-saturated"), TIMEOUT, TIMEOUT);

    for release in decoder_releases {
        let _ = release.send(());
    }
    for client in decoder_clients {
        assert!(client.join().unwrap());
    }

    let active_id = active.command().command_id().clone();
    let _ = active.respond(CommandAck::CloseAccepted {
        command_id: active_id,
    });
    for _ in 0..QUEUE_CAPACITY {
        let pending = server.receive().unwrap();
        let command_id = pending.command().command_id().clone();
        let _ = pending.respond(CommandAck::CloseAccepted { command_id });
    }

    let active_result = active_client.join().unwrap();
    let queued_results: Vec<_> = queued_clients
        .into_iter()
        .map(|client| client.join().unwrap())
        .collect();

    assert!(
        matches!(saturated, Err(BrokerCommandError::PrimaryBusy)),
        "combined saturation returned {saturated:?}"
    );
    assert!(matches!(
        active_result.unwrap(),
        CommandAck::CloseAccepted { .. }
    ));
    assert_eq!(
        queued_results
            .iter()
            .filter(|result| matches!(result, Ok(CommandAck::CloseAccepted { .. })))
            .count(),
        QUEUE_CAPACITY
    );
    assert_eq!(
        queued_results
            .iter()
            .filter(|result| matches!(
                result,
                Ok(CommandAck::Rejected {
                    reason: CommandRejectionCode::QueueFull,
                    ..
                })
            ))
            .count(),
        1
    );

    let recovered = spawn_client(&name, close_request("command-combined-recovered"));
    let pending = server.receive().unwrap();
    let command_id = pending.command().command_id().clone();
    pending
        .respond(CommandAck::CloseAccepted { command_id })
        .unwrap();
    assert!(matches!(
        recovered.join().unwrap().unwrap(),
        CommandAck::CloseAccepted { .. }
    ));
}

fn create_raw_server(name: &str) -> File {
    let full_name = wide(&format!(r"\\.\pipe\{name}"));
    let handle = unsafe {
        CreateNamedPipeW(
            full_name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            0,
            null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    unsafe { File::from_raw_handle(handle) }
}

fn connect_raw_server(pipe: &File) {
    let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle().cast(), null_mut()) };
    assert!(connected != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED);
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
