use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use previewit_broker::{InstanceLease, InstanceRole};
use uuid::Uuid;

const PRODUCT_SUFFIX_ENV: &str = "PREVIEWIT_TEST_PRODUCT_SUFFIX";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);

struct ManagedChild {
    child: Option<Child>,
}

impl ManagedChild {
    fn spawn(suffix: &str, arguments: &[&OsStr]) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_previewit-broker"))
            .args(arguments)
            .env(PRODUCT_SUFFIX_ENV, suffix)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch Broker process");
        Self { child: Some(child) }
    }

    fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .as_mut()
            .expect("child")
            .try_wait()
            .expect("poll Broker process")
    }

    fn text_after_exit(&mut self) -> String {
        let child = self.child.as_mut().expect("child");
        let mut text = String::new();
        child
            .stdout
            .as_mut()
            .expect("Broker stdout")
            .read_to_string(&mut text)
            .expect("read Broker stdout");
        child
            .stderr
            .as_mut()
            .expect("Broker stderr")
            .read_to_string(&mut text)
            .expect("read Broker stderr");
        text
    }

    fn kill_and_collect(&mut self) -> String {
        let child = self.child.as_mut().expect("child");
        let _ = child.kill();
        child.wait().expect("wait for killed Broker");
        self.text_after_exit()
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repository root")
        .to_path_buf()
}

fn fixture() -> PathBuf {
    repository_root()
        .join("tests")
        .join("fixtures")
        .join("handles")
        .join("read-only.txt")
}

fn unique_suffix(label: &str) -> String {
    format!("{label}.{}", Uuid::new_v4().simple())
}

fn product_id(suffix: &str) -> String {
    format!("PreviewIt.Broker.Test.{suffix}")
}

fn wait_for_exit(child: &mut ManagedChild, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait() {
            return status;
        }
        assert!(Instant::now() < deadline, "Broker process did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn ten_processes_elect_one_primary_and_crash_takeover_replaces_it() {
    let suffix = unique_suffix("election");
    let path = fixture();
    let arguments = [OsStr::new("--open"), path.as_os_str()];
    let mut processes: Vec<_> = (0..10)
        .map(|_| ManagedChild::spawn(&suffix, &arguments))
        .collect();
    let mut statuses = vec![None; processes.len()];
    let deadline = Instant::now() + PROCESS_TIMEOUT;

    loop {
        for (index, process) in processes.iter_mut().enumerate() {
            if statuses[index].is_none() {
                statuses[index] = process.try_wait();
            }
        }
        let exited = statuses.iter().flatten().count();
        if exited >= 9 || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let running: Vec<_> = statuses
        .iter()
        .enumerate()
        .filter_map(|(index, status)| status.is_none().then_some(index))
        .collect();
    assert_eq!(running.len(), 1, "exactly one Broker must remain primary");
    for (index, status) in statuses.iter().enumerate() {
        if let Some(status) = status {
            assert!(status.success(), "secondary {index} exited with {status}");
            let text = processes[index].text_after_exit();
            assert!(text.contains("role=secondary"));
            assert!(text.contains("accepted=true"));
            assert!(!text.contains(&path.display().to_string()));
        }
    }

    let primary_index = running[0];
    let primary_text = processes[primary_index].kill_and_collect();
    assert!(primary_text.contains("role=primary"));
    assert!(primary_text.contains("event=instance-elected"));
    assert!(primary_text.contains("event=endpoint-started"));
    assert!(primary_text.contains("event=command-accepted"));

    let close_arguments = [OsStr::new("--close")];
    let mut replacement = ManagedChild::spawn(&suffix, &close_arguments);
    let takeover_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(
            replacement.try_wait().is_none(),
            "replacement exited instead of becoming primary"
        );
        if Instant::now() >= takeover_deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let replacement_text = replacement.kill_and_collect();
    assert!(replacement_text.contains("role=primary"));
    assert!(replacement_text.contains("accepted=true"));
    assert!(replacement_text.contains("event=instance-elected"));
    assert!(replacement_text.contains("event=endpoint-started"));
}

#[test]
fn held_lease_without_pipe_returns_primary_not_ready_within_deadline() {
    let suffix = unique_suffix("not-ready");
    let InstanceRole::Primary(_lease) =
        InstanceLease::elect(&product_id(&suffix)).expect("hold test lease")
    else {
        panic!("unique test lease must elect primary");
    };
    let started = Instant::now();
    let mut contender = ManagedChild::spawn(&suffix, &[OsStr::new("--close")]);

    let status = wait_for_exit(&mut contender, Duration::from_secs(2));
    let text = contender.text_after_exit();

    assert!(!status.success());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(text.contains("primary-not-ready"));
}

#[test]
fn invalid_arguments_fail_before_instance_election() {
    let suffix = unique_suffix("invalid-arguments");
    let InstanceRole::Primary(_lease) =
        InstanceLease::elect(&product_id(&suffix)).expect("hold test lease")
    else {
        panic!("unique test lease must elect primary");
    };
    let path = fixture();
    let arguments = [
        OsStr::new("--open"),
        path.as_os_str(),
        OsStr::new("--close"),
    ];
    let started = Instant::now();
    let mut invalid = ManagedChild::spawn(&suffix, &arguments);

    let status = wait_for_exit(&mut invalid, Duration::from_secs(1));
    let text = invalid.text_after_exit();

    assert!(!status.success());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(text.contains("usage:"));
    assert!(!text.contains("primary-not-ready"));
}

#[test]
fn broker_executable_is_x64() {
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)").expect("ProgramFiles(x86)");
    let vswhere = Path::new(&program_files_x86)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    let located = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-find",
            r"VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe",
        ])
        .output()
        .expect("locate dumpbin");
    assert!(located.status.success());
    let dumpbin = String::from_utf8_lossy(&located.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("x64 dumpbin path")
        .trim()
        .to_owned();

    let headers = Command::new(dumpbin)
        .arg("/headers")
        .arg(env!("CARGO_BIN_EXE_previewit-broker"))
        .output()
        .expect("inspect Broker executable");
    assert!(headers.status.success());
    let stdout = String::from_utf8_lossy(&headers.stdout);
    assert!(
        stdout.contains("8664 machine (x64)"),
        "Broker PE header was not x64:\n{stdout}"
    );
}
