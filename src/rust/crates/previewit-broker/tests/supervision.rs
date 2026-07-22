use std::path::PathBuf;
use std::time::{Duration, Instant};

use previewit_broker::{Deadlines, ShutdownOutcome, SupervisorError, WorkerMode, WorkerSupervisor};

fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("source root")
        .to_path_buf()
}

fn repository_root() -> PathBuf {
    source_root()
        .parent()
        .expect("repository root")
        .to_path_buf()
}

fn worker_exe() -> PathBuf {
    source_root()
        .join("dotnet")
        .join("PreviewIt.WorkerProbe")
        .join("bin")
        .join("Release")
        .join("net462")
        .join("PreviewIt.WorkerProbe.exe")
}

fn fixture() -> PathBuf {
    repository_root()
        .join("tests")
        .join("fixtures")
        .join("handles")
        .join("read-only.txt")
}

fn deadlines() -> Deadlines {
    Deadlines {
        startup: Duration::from_secs(5),
        io: Duration::from_secs(5),
        cancel: Duration::from_millis(150),
    }
}

#[test]
fn crashing_worker_does_not_terminate_broker() {
    let broker_pid = std::process::id();
    let mut supervisor = WorkerSupervisor::launch(worker_exe(), WorkerMode::Crash, deadlines())
        .expect("launch crash worker");

    let status = supervisor
        .wait_for_exit(Duration::from_secs(2))
        .expect("wait for crash")
        .expect("crash worker must exit");

    assert!(!status.success());
    assert_eq!(std::process::id(), broker_pid);
}

#[test]
fn hung_worker_is_terminated_by_closing_its_job() {
    let broker_pid = std::process::id();
    let mut supervisor = WorkerSupervisor::launch(worker_exe(), WorkerMode::Hang, deadlines())
        .expect("launch hang worker");

    let started = Instant::now();
    let outcome = supervisor
        .shutdown("cancel-hang")
        .expect("cancel hung worker");
    let elapsed = started.elapsed();

    assert_eq!(outcome, ShutdownOutcome::Terminated);
    assert!(!supervisor.is_worker_running().expect("worker state"));
    assert_eq!(std::process::id(), broker_pid);
    assert!(elapsed >= deadlines().cancel);
    assert!(elapsed < Duration::from_secs(2));
    println!("HANG_CANCEL_ELAPSED_MS={}", elapsed.as_millis());
}

#[test]
fn stale_result_is_never_accepted_for_current_request() {
    let mut supervisor = WorkerSupervisor::launch(worker_exe(), WorkerMode::Stale, deadlines())
        .expect("launch stale worker");

    supervisor
        .open_document(&fixture(), "request-1")
        .expect("first request");
    let error = supervisor
        .open_document(&fixture(), "request-2")
        .expect_err("stale response must be rejected");

    assert!(matches!(
        error,
        SupervisorError::StaleResponse { expected, actual }
            if expected == "request-2" && actual == "request-1"
    ));
}
