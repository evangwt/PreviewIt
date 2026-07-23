use std::time::Duration;

use previewit_broker::{BrokerCommandClient, BrokerCommandError, BrokerCommandServer};
use previewit_protocol::v0::{
    BrokerControlRequest, BrokerControlResponse, ClosePreview, OpenPath, broker_control_request,
};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(2);

fn pipe_name(label: &str) -> String {
    format!("PreviewIt.Test.{label}.{}", Uuid::new_v4().simple())
}

fn open_path_request() -> BrokerControlRequest {
    let path = r"C:\fixtures\preview.txt";
    BrokerControlRequest {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: "command-1".into(),
        command: Some(broker_control_request::Command::OpenPath(OpenPath {
            path_utf16le: path.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        })),
    }
}

fn accepted(command_id: &str, request_id: &str) -> BrokerControlResponse {
    BrokerControlResponse {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: command_id.into(),
        accepted: true,
        request_id: request_id.into(),
        error_code: String::new(),
    }
}

fn request_with_path(command_id: &str, path_utf16le: Vec<u8>) -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: command_id.into(),
        command: Some(broker_control_request::Command::OpenPath(OpenPath {
            path_utf16le,
        })),
    }
}

#[test]
fn broker_control_round_trips_open_path() {
    let name = pipe_name("open");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        let request = server.receive_request().unwrap();
        server
            .send_response(&accepted(&request.command_id, "request-1"))
            .unwrap();
        request
    });

    let request = open_path_request();
    let response = BrokerCommandClient::send(&name, &request, TIMEOUT, TIMEOUT).unwrap();

    assert!(response.accepted);
    assert_eq!(response.command_id, "command-1");
    assert_eq!(response.request_id, "request-1");
    assert_eq!(server.join().unwrap(), request);
}

#[test]
fn broker_control_round_trips_close_preview() {
    let name = pipe_name("close");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        let request = server.receive_request().unwrap();
        server
            .send_response(&accepted(&request.command_id, ""))
            .unwrap();
        request
    });

    let request = BrokerControlRequest {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: "command-close".into(),
        command: Some(broker_control_request::Command::ClosePreview(
            ClosePreview {},
        )),
    };
    let response = BrokerCommandClient::send(&name, &request, TIMEOUT, TIMEOUT).unwrap();

    assert!(response.accepted);
    assert!(response.request_id.is_empty());
    assert_eq!(server.join().unwrap(), request);
}

#[test]
fn wrong_protocol_major_returns_stable_rejection() {
    let name = pipe_name("wrong-major");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        server.receive_request()
    });

    let mut request = open_path_request();
    request.protocol_major = 1;
    let response = BrokerCommandClient::send(&name, &request, TIMEOUT, TIMEOUT).unwrap();

    assert!(!response.accepted);
    assert_eq!(response.command_id, "command-1");
    assert_eq!(response.error_code, "protocol-major-mismatch");
    assert!(matches!(
        server.join().unwrap(),
        Err(BrokerCommandError::ProtocolMajorMismatch { .. })
    ));
}

#[test]
fn invalid_utf16_path_returns_stable_rejection() {
    let name = pipe_name("invalid-utf16");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        server.receive_request()
    });

    let response = BrokerCommandClient::send(
        &name,
        &request_with_path("command-invalid", vec![0x00]),
        TIMEOUT,
        TIMEOUT,
    )
    .unwrap();

    assert!(!response.accepted);
    assert_eq!(response.error_code, "invalid-utf16");
    assert!(matches!(
        server.join().unwrap(),
        Err(BrokerCommandError::InvalidUtf16)
    ));
}

#[test]
fn embedded_nul_path_returns_stable_rejection() {
    let name = pipe_name("embedded-nul");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        server.receive_request()
    });

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

    assert!(!response.accepted);
    assert_eq!(response.error_code, "embedded-nul");
    assert!(matches!(
        server.join().unwrap(),
        Err(BrokerCommandError::EmbeddedNul)
    ));
}

#[test]
fn overlong_path_returns_stable_rejection_without_echoing_path() {
    let name = pipe_name("overlong-path");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        server.receive_request()
    });

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

    assert!(!response.accepted);
    assert_eq!(response.error_code, "path-too-long");
    assert!(response.request_id.is_empty());
    assert!(matches!(
        server.join().unwrap(),
        Err(BrokerCommandError::PathTooLong)
    ));
}

#[test]
fn missing_command_id_returns_stable_rejection() {
    let name = pipe_name("missing-command-id");
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        let server = BrokerCommandServer::create(&server_name, TIMEOUT, TIMEOUT).unwrap();
        server.receive_request()
    });

    let response = BrokerCommandClient::send(
        &name,
        &request_with_path(
            "",
            r"C:\fixtures\preview.txt"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
        TIMEOUT,
        TIMEOUT,
    )
    .unwrap();

    assert!(!response.accepted);
    assert_eq!(response.error_code, "invalid-command-id");
    assert!(matches!(
        server.join().unwrap(),
        Err(BrokerCommandError::InvalidCommandId)
    ));
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
        let _request = server.receive_request().unwrap();
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
    const QUEUE_CAPACITY: usize = 8;

    let name = pipe_name("queue-full");
    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let clients: Vec<_> = (0..=QUEUE_CAPACITY)
        .map(|index| {
            let client_name = name.clone();
            std::thread::spawn(move || {
                let mut request = open_path_request();
                request.command_id = format!("command-{index}");
                BrokerCommandClient::send(&client_name, &request, TIMEOUT, TIMEOUT)
            })
        })
        .collect();

    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..QUEUE_CAPACITY {
        let request = server.receive_request().unwrap();
        server
            .send_response(&accepted(&request.command_id, "request-queued"))
            .unwrap();
    }

    let responses: Vec<_> = clients
        .into_iter()
        .map(|client| client.join().unwrap().unwrap())
        .collect();
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.accepted)
            .count(),
        QUEUE_CAPACITY
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.error_code == "queue-full")
            .count(),
        1
    );
}
