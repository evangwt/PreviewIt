use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use previewit_broker::{BrokerError, PipeServer};
use previewit_protocol::v0::{Envelope, HelloAck, envelope};

fn worker_exe() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("source root")
        .to_path_buf();
    root.join("dotnet")
        .join("PreviewIt.WorkerProbe")
        .join("bin")
        .join("Release")
        .join("net462")
        .join("PreviewIt.WorkerProbe.exe")
}

fn launch_worker(server: &PipeServer, extra: &[&str]) -> Child {
    let mut command = Command::new(worker_exe());
    command
        .arg("--pipe")
        .arg(server.name())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for argument in extra {
        command.arg(argument);
    }
    command
        .spawn()
        .expect("worker probe must be built before this test")
}

fn assert_hello(hello: &Envelope) {
    assert_eq!(hello.protocol_major, 0);
    assert_eq!(hello.protocol_minor, 1);
    assert_eq!(hello.request_id, "handshake-1");
    let Some(envelope::Payload::Hello(payload)) = hello.payload.as_ref() else {
        panic!("expected Hello payload");
    };
    assert_eq!(payload.component_id, "dotnet-worker-probe");
    assert_eq!(payload.capabilities, ["read-handle-v0"]);
}

#[test]
fn dotnet_worker_completes_authenticated_handshake() {
    let mut server = PipeServer::create(Duration::from_secs(5)).expect("create pipe");
    assert!(server.name().starts_with("PreviewIt-"));
    assert!(!server.name().contains(['\\', '/']));
    let mut worker = launch_worker(&server, &[]);

    let hello = server
        .accept_hello(worker.id())
        .expect("authenticated hello");
    assert_hello(&hello);

    server
        .send_hello_ack(HelloAck {
            accepted_capabilities: vec!["read-handle-v0".to_owned()],
        })
        .expect("send HelloAck");

    let status = worker.wait().expect("wait for worker");
    assert!(status.success(), "worker exited with {status}");
}

#[test]
fn non_expected_client_pid_is_rejected_before_protocol_read() {
    let mut server = PipeServer::create(Duration::from_secs(5)).expect("create pipe");
    let mut worker = launch_worker(&server, &[]);

    let error = server
        .accept_hello(worker.id().saturating_add(1))
        .expect_err("wrong client pid must be rejected");
    assert!(matches!(error, BrokerError::UnexpectedClientProcess { .. }));

    let _ = worker.kill();
    let _ = worker.wait();
}

#[test]
fn protocol_major_mismatch_is_rejected() {
    let mut server = PipeServer::create(Duration::from_secs(5)).expect("create pipe");
    let mut worker = launch_worker(&server, &["--protocol-major", "1"]);

    let error = server
        .accept_hello(worker.id())
        .expect_err("protocol major mismatch must be rejected");
    assert!(matches!(
        error,
        BrokerError::ProtocolMajorMismatch { major: 1 }
    ));

    drop(server);
    let status = worker.wait().expect("wait for worker");
    assert!(!status.success(), "mismatched worker must fail");
}

#[test]
fn oversized_control_frame_is_rejected() {
    let mut server = PipeServer::create(Duration::from_secs(5)).expect("create pipe");
    let mut worker = launch_worker(&server, &["--oversized"]);

    let error = server
        .accept_hello(worker.id())
        .expect_err("oversized control frame must be rejected");
    assert!(matches!(error, BrokerError::ControlFrameTooLarge { .. }));

    let status = worker.wait().expect("wait for worker");
    assert!(!status.success(), "oversized worker must fail");
}

#[test]
fn startup_deadline_expires_without_client() {
    let mut server = PipeServer::create(Duration::from_millis(100)).expect("create pipe");

    let error = server
        .accept_hello(1)
        .expect_err("missing client must hit startup deadline");
    assert!(matches!(error, BrokerError::StartupTimeout));
}
