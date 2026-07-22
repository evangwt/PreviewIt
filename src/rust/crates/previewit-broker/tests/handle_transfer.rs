use std::path::{Path, PathBuf};
use std::time::Duration;

use previewit_broker::{
    Deadlines, ShutdownOutcome, WorkerMode, WorkerSupervisor, current_process_handle_count,
};
use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

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
        cancel: Duration::from_millis(250),
    }
}

fn launch_handles_worker() -> WorkerSupervisor {
    WorkerSupervisor::launch(worker_exe(), WorkerMode::Handles, deadlines())
        .expect("launch handles worker")
}

fn assert_file_unchanged(path: &Path, expected: &[u8]) {
    assert_eq!(std::fs::read(path).expect("read fixture"), expected);
}

#[test]
fn duplicated_handle_reads_but_cannot_write() {
    let path = fixture();
    let expected = std::fs::read(&path).expect("read fixture");
    let mut supervisor = launch_handles_worker();

    let outcome = supervisor
        .open_document(&path, "handle-1")
        .expect("read through duplicated handle");

    assert_eq!(outcome.status, "read-ok");
    assert_eq!(outcome.payload, expected);
    assert_eq!(outcome.write_error, ERROR_ACCESS_DENIED);
    assert_file_unchanged(&path, &expected);

    let shutdown = supervisor.shutdown("shutdown-1").expect("shutdown worker");
    assert!(matches!(shutdown, ShutdownOutcome::Exited(status) if status.success()));
}

#[test]
fn one_hundred_transfers_do_not_leak_broker_handles() {
    let path = fixture();
    let expected = std::fs::read(&path).expect("read fixture");
    let mut supervisor = launch_handles_worker();
    let starting_handles = current_process_handle_count().expect("starting handle count");

    for index in 0..100 {
        let outcome = supervisor
            .open_document(&path, &format!("repeat-{index}"))
            .expect("repeated handle transfer");
        assert_eq!(outcome.status, "read-ok");
        assert_eq!(outcome.payload, expected);
        assert_eq!(outcome.write_error, ERROR_ACCESS_DENIED);
    }

    let ending_handles = current_process_handle_count().expect("ending handle count");
    println!("BROKER_HANDLE_COUNT={starting_handles}->{ending_handles}");
    assert!(
        ending_handles <= starting_handles + 2,
        "broker handle count grew from {starting_handles} to {ending_handles}"
    );
    assert_file_unchanged(&path, &expected);

    let shutdown = supervisor
        .shutdown("shutdown-100")
        .expect("shutdown worker");
    assert!(matches!(shutdown, ShutdownOutcome::Exited(status) if status.success()));
}
