use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::ptr::null;
use std::time::Duration;

use previewit_protocol::v0::{Envelope, OpenDocument, envelope};
use thiserror::Error;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use crate::handles::{HandleError, ReadOnlyDocument};
use crate::pipe::{BrokerError, PipeServer};

#[derive(Clone, Copy, Debug)]
pub struct Deadlines {
    pub startup: Duration,
    pub io: Duration,
    pub cancel: Duration,
}

#[derive(Clone, Copy, Debug)]
pub enum WorkerMode {
    Handles,
    Crash,
    Hang,
    Stale,
}

impl WorkerMode {
    fn argument(self) -> &'static str {
        match self {
            Self::Handles => "handles",
            Self::Crash => "crash",
            Self::Hang => "hang",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug)]
pub struct DocumentOutcome {
    pub status: String,
    pub payload: Vec<u8>,
    pub write_error: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Exited(ExitStatus),
    Terminated,
}

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error(transparent)]
    Broker(#[from] BrokerError),
    #[error(transparent)]
    Handle(#[from] HandleError),
    #[error("{operation} failed: {source}")]
    Windows {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("worker exited before the request completed")]
    WorkerExited,
    #[error("response for request {actual} arrived while {expected} was current")]
    StaleResponse { expected: String, actual: String },
    #[error("worker returned an unexpected response envelope")]
    UnexpectedResponse,
    #[error("worker returned a non-read result status: {0}")]
    UnexpectedStatus(String),
    #[error("worker did not report a numeric write error: {0}")]
    InvalidWriteError(String),
}

pub struct WorkerSupervisor {
    pipe: PipeServer,
    child: Child,
    job: Option<Job>,
    deadlines: Deadlines,
    current_request_id: Option<String>,
}

impl WorkerSupervisor {
    pub fn launch(
        worker_exe: PathBuf,
        mode: WorkerMode,
        deadlines: Deadlines,
    ) -> Result<Self, SupervisorError> {
        let pipe = PipeServer::create_with_timeouts(deadlines.startup, deadlines.io)?;
        let mut command = Command::new(worker_exe);
        command
            .arg("--pipe")
            .arg(pipe.name())
            .arg("--mode")
            .arg(mode.argument())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn().map_err(|source| SupervisorError::Windows {
            operation: "spawn worker probe",
            source,
        })?;
        let job = match Job::assign(child.as_raw_handle()) {
            Ok(job) => job,
            Err(error) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };

        let mut supervisor = Self {
            pipe,
            child,
            job: Some(job),
            deadlines,
            current_request_id: None,
        };

        let child_pid = supervisor.child.id();
        if let Err(error) = supervisor.pipe.accept_hello(child_pid) {
            supervisor.terminate_job();
            return Err(error.into());
        }
        if let Err(error) = supervisor
            .pipe
            .send_hello_ack(previewit_protocol::v0::HelloAck {
                accepted_capabilities: vec!["read-handle-v0".to_owned()],
            })
        {
            supervisor.terminate_job();
            return Err(error.into());
        }

        Ok(supervisor)
    }

    pub fn open_document(
        &mut self,
        path: &Path,
        request_id: &str,
    ) -> Result<DocumentOutcome, SupervisorError> {
        self.current_request_id = Some(request_id.to_owned());
        let document = ReadOnlyDocument::open(path)?;
        let remote = unsafe { document.duplicate_into(self.child.as_raw_handle())? };
        let envelope = Envelope {
            protocol_major: 0,
            protocol_minor: 1,
            request_id: request_id.to_owned(),
            payload: Some(envelope::Payload::OpenDocument(OpenDocument {
                duplicated_handle: remote.value(),
                size: document.size(),
                display_name: document.display_name().to_owned(),
            })),
        };
        self.pipe.send_envelope(&envelope)?;
        let _transferred_handle = remote.into_transferred_value();

        let result = self.receive_current()?;
        let Some(envelope::Payload::Result(result)) = result.payload else {
            return Err(SupervisorError::UnexpectedResponse);
        };
        if result.status != "read-ok" {
            return Err(SupervisorError::UnexpectedStatus(result.status));
        }

        let write_error = self.receive_current()?;
        let Some(envelope::Payload::Error(error)) = write_error.payload else {
            return Err(SupervisorError::UnexpectedResponse);
        };
        if error.code != "write-denied" {
            return Err(SupervisorError::UnexpectedResponse);
        }
        let write_error = error
            .message
            .parse::<u32>()
            .map_err(|_| SupervisorError::InvalidWriteError(error.message.clone()))?;

        Ok(DocumentOutcome {
            status: result.status,
            payload: result.payload,
            write_error,
        })
    }

    pub fn wait_for_exit(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<ExitStatus>, SupervisorError> {
        let wait = unsafe { WaitForSingleObject(self.child.as_raw_handle(), timeout_ms(timeout)) };
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        if wait != WAIT_OBJECT_0 {
            return Err(last_error("WaitForSingleObject(worker)"));
        }
        self.child
            .wait()
            .map(Some)
            .map_err(|source| SupervisorError::Windows {
                operation: "wait for worker",
                source,
            })
    }

    pub fn is_worker_running(&mut self) -> Result<bool, SupervisorError> {
        self.child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|source| SupervisorError::Windows {
                operation: "query worker state",
                source,
            })
    }

    pub fn shutdown(&mut self, request_id: &str) -> Result<ShutdownOutcome, SupervisorError> {
        if !self.is_worker_running()? {
            return self
                .child
                .wait()
                .map(ShutdownOutcome::Exited)
                .map_err(|source| SupervisorError::Windows {
                    operation: "collect exited worker",
                    source,
                });
        }

        let cancel = Envelope {
            protocol_major: 0,
            protocol_minor: 1,
            request_id: request_id.to_owned(),
            payload: Some(envelope::Payload::Cancel(Default::default())),
        };
        self.pipe.send_envelope(&cancel)?;

        match self.wait_for_exit(self.deadlines.cancel)? {
            Some(status) => Ok(ShutdownOutcome::Exited(status)),
            None => {
                self.terminate_and_reap(Duration::from_secs(5));
                Ok(ShutdownOutcome::Terminated)
            }
        }
    }

    fn receive_current(&self) -> Result<Envelope, SupervisorError> {
        let envelope = self.pipe.receive_envelope()?;
        let expected = self
            .current_request_id
            .as_ref()
            .expect("current request is set before receiving")
            .clone();
        if envelope.request_id != expected {
            return Err(SupervisorError::StaleResponse {
                expected,
                actual: envelope.request_id,
            });
        }
        Ok(envelope)
    }

    fn terminate_job(&mut self) {
        let _ = self.job.take();
    }

    fn terminate_and_reap(&mut self, timeout: Duration) {
        self.terminate_job();
        let mut wait =
            unsafe { WaitForSingleObject(self.child.as_raw_handle(), timeout_ms(timeout)) };
        if wait != WAIT_OBJECT_0 {
            let _ = self.child.kill();
            wait = unsafe {
                WaitForSingleObject(
                    self.child.as_raw_handle(),
                    timeout_ms(Duration::from_secs(1)),
                )
            };
        }
        if wait == WAIT_OBJECT_0 {
            let _ = self.child.wait();
        }
    }
}

impl Drop for WorkerSupervisor {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.terminate_and_reap(Duration::from_secs(1));
        }
    }
}

struct Job {
    handle: HANDLE,
}

impl Job {
    fn assign(process: HANDLE) -> Result<Self, SupervisorError> {
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(last_error("CreateJobObjectW"));
        }

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            unsafe {
                CloseHandle(handle);
            }
            return Err(last_error("SetInformationJobObject"));
        }

        if unsafe { AssignProcessToJobObject(handle, process) } == 0 {
            unsafe {
                CloseHandle(handle);
            }
            return Err(last_error("AssignProcessToJobObject"));
        }
        Ok(Self { handle })
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

use std::os::windows::io::AsRawHandle;

fn timeout_ms(duration: Duration) -> u32 {
    duration.as_millis().clamp(1, u32::MAX as u128) as u32
}

fn last_error(operation: &'static str) -> SupervisorError {
    SupervisorError::Windows {
        operation,
        source: io::Error::from_raw_os_error(unsafe { GetLastError() } as i32),
    }
}
