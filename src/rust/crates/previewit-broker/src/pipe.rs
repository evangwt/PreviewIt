use std::io;
use std::os::windows::io::AsRawHandle;
use std::time::Duration;

use previewit_protocol::v0::{Envelope, HelloAck, envelope};
use previewit_protocol::{MAX_CONTROL_FRAME, PROTOCOL_MAJOR, PROTOCOL_MINOR, encode_frame};
use prost::Message;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::runtime::{Builder, Runtime};
use tokio::time::timeout;
use uuid::Uuid;
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

use crate::windows_security::{CurrentUserSecurity, WindowsSecurityError};

const REQUIRED_CAPABILITY: &str = "read-handle-v0";
const PIPE_BUFFER_SIZE: u32 = (MAX_CONTROL_FRAME + 4) as u32;

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("{operation} failed: {source}")]
    Windows {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("the worker did not connect before the startup deadline")]
    StartupTimeout,
    #[error("a pipe read did not complete before its deadline")]
    ReadTimeout,
    #[error("a pipe write did not complete before its deadline")]
    WriteTimeout,
    #[error("expected pipe client process {expected}, got {actual}")]
    UnexpectedClientProcess { expected: u32, actual: u32 },
    #[error("control frame declares {declared} bytes, above the 1 MiB limit")]
    ControlFrameTooLarge { declared: usize },
    #[error("the pipe closed before a complete control frame arrived")]
    TruncatedControlFrame,
    #[error("invalid Protobuf envelope: {0}")]
    InvalidEnvelope(#[from] prost::DecodeError),
    #[error("unsupported protocol major {major}")]
    ProtocolMajorMismatch { major: u32 },
    #[error("unsupported protocol minor {minor}")]
    ProtocolMinorMismatch { minor: u32 },
    #[error("the first control frame was not Hello")]
    ExpectedHello,
    #[error("Hello did not advertise read-handle-v0")]
    MissingReadHandleCapability,
    #[error("cannot encode control frame: {0}")]
    Frame(#[from] previewit_protocol::FrameError),
}

impl From<WindowsSecurityError> for BrokerError {
    fn from(error: WindowsSecurityError) -> Self {
        let WindowsSecurityError::Windows { operation, source } = error;
        Self::Windows { operation, source }
    }
}

pub struct PipeServer {
    handle: NamedPipeServer,
    runtime: Runtime,
    name: String,
    startup_timeout: Duration,
    io_timeout: Duration,
}

impl PipeServer {
    pub fn create(timeout: Duration) -> Result<Self, BrokerError> {
        Self::create_with_timeouts(timeout, timeout)
    }

    pub fn create_with_timeouts(
        startup_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<Self, BrokerError> {
        let name = format!("PreviewIt-{}", Uuid::new_v4().simple());
        let full_name = format!(r"\\.\pipe\{name}");
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|source| BrokerError::Windows {
                operation: "create Tokio pipe runtime",
                source,
            })?;
        let security = CurrentUserSecurity::new()?;
        let _entered = runtime.enter();
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .max_instances(1)
            .in_buffer_size(PIPE_BUFFER_SIZE)
            .out_buffer_size(PIPE_BUFFER_SIZE);
        let handle = unsafe {
            options.create_with_security_attributes_raw(
                &full_name,
                security.attributes().cast_mut().cast(),
            )
        }
        .map_err(|source| BrokerError::Windows {
            operation: "CreateNamedPipeW",
            source,
        })?;

        Ok(Self {
            handle,
            runtime,
            name,
            startup_timeout,
            io_timeout,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn accept_hello(&mut self, expected_pid: u32) -> Result<Envelope, BrokerError> {
        self.connect()?;

        let mut actual_pid = 0;
        if unsafe {
            GetNamedPipeClientProcessId(self.handle.as_raw_handle().cast(), &mut actual_pid)
        } == 0
        {
            return Err(last_error("GetNamedPipeClientProcessId"));
        }
        if actual_pid != expected_pid {
            return Err(BrokerError::UnexpectedClientProcess {
                expected: expected_pid,
                actual: actual_pid,
            });
        }

        let envelope = self.receive_envelope()?;
        if envelope.protocol_major != PROTOCOL_MAJOR {
            return Err(BrokerError::ProtocolMajorMismatch {
                major: envelope.protocol_major,
            });
        }
        if envelope.protocol_minor != PROTOCOL_MINOR {
            return Err(BrokerError::ProtocolMinorMismatch {
                minor: envelope.protocol_minor,
            });
        }

        let Some(envelope::Payload::Hello(hello)) = envelope.payload.as_ref() else {
            return Err(BrokerError::ExpectedHello);
        };
        if !hello
            .capabilities
            .iter()
            .any(|capability| capability == REQUIRED_CAPABILITY)
        {
            return Err(BrokerError::MissingReadHandleCapability);
        }

        Ok(envelope)
    }

    pub fn send_hello_ack(&mut self, hello_ack: HelloAck) -> Result<(), BrokerError> {
        let envelope = Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            request_id: "handshake-1".to_owned(),
            payload: Some(envelope::Payload::HelloAck(hello_ack)),
        };
        self.send_envelope(&envelope)
    }

    pub fn send_envelope(&mut self, envelope: &Envelope) -> Result<(), BrokerError> {
        self.write_frame(&envelope.encode_to_vec())
    }

    pub fn receive_envelope(&mut self) -> Result<Envelope, BrokerError> {
        let payload = self.read_frame()?;
        Ok(Envelope::decode(payload.as_slice())?)
    }

    fn connect(&mut self) -> Result<(), BrokerError> {
        let Self {
            handle,
            runtime,
            startup_timeout,
            ..
        } = self;
        runtime.block_on(async {
            match timeout(*startup_timeout, handle.connect()).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(source)) => Err(BrokerError::Windows {
                    operation: "ConnectNamedPipe",
                    source,
                }),
                Err(_) => Err(BrokerError::StartupTimeout),
            }
        })
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, BrokerError> {
        let Self {
            handle,
            runtime,
            io_timeout,
            ..
        } = self;
        runtime.block_on(read_frame(handle, *io_timeout))
    }

    fn write_frame(&mut self, payload: &[u8]) -> Result<(), BrokerError> {
        let Self {
            handle,
            runtime,
            io_timeout,
            ..
        } = self;
        runtime.block_on(write_frame(handle, payload, *io_timeout))
    }
}

pub(crate) async fn read_frame<T>(io: &mut T, io_timeout: Duration) -> Result<Vec<u8>, BrokerError>
where
    T: AsyncRead + Unpin,
{
    let operation = async {
        let mut length_bytes = [0_u8; 4];
        io.read_exact(&mut length_bytes)
            .await
            .map_err(normalize_read_error)?;
        let declared = u32::from_le_bytes(length_bytes) as usize;
        if declared > MAX_CONTROL_FRAME {
            return Err(BrokerError::ControlFrameTooLarge { declared });
        }

        let mut payload = vec![0_u8; declared];
        io.read_exact(&mut payload)
            .await
            .map_err(normalize_read_error)?;
        Ok(payload)
    };

    match timeout(io_timeout, operation).await {
        Ok(result) => result,
        Err(_) => Err(BrokerError::ReadTimeout),
    }
}

pub(crate) async fn write_frame<T>(
    io: &mut T,
    payload: &[u8],
    io_timeout: Duration,
) -> Result<(), BrokerError>
where
    T: AsyncWrite + Unpin,
{
    let frame = encode_frame(payload)?;
    match timeout(io_timeout, io.write_all(&frame)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(source)) => Err(BrokerError::Windows {
            operation: "write pipe frame",
            source,
        }),
        Err(_) => Err(BrokerError::WriteTimeout),
    }
}

fn normalize_read_error(source: io::Error) -> BrokerError {
    if source.kind() == io::ErrorKind::UnexpectedEof
        || source.kind() == io::ErrorKind::BrokenPipe
        || matches!(source.raw_os_error(), Some(code) if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_NO_DATA as i32)
    {
        BrokerError::TruncatedControlFrame
    } else {
        BrokerError::Windows {
            operation: "read pipe frame",
            source,
        }
    }
}

fn last_error(operation: &'static str) -> BrokerError {
    BrokerError::Windows {
        operation,
        source: io::Error::last_os_error(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::thread;
    use std::time::Instant;

    use super::*;

    #[test]
    fn delayed_partial_frame_is_stably_truncated() {
        let mut server =
            PipeServer::create_with_timeouts(Duration::from_secs(2), Duration::from_secs(2))
                .unwrap();
        let pipe_path = format!(r"\\.\pipe\{}", server.name());
        let client = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut pipe = loop {
                match OpenOptions::new().read(true).write(true).open(&pipe_path) {
                    Ok(pipe) => break pipe,
                    Err(_) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("raw client could not connect: {error}"),
                }
            };
            pipe.write_all(&[10, 0, 0, 0, 1, 2]).unwrap();
            pipe.flush().unwrap();
            thread::sleep(Duration::from_millis(300));
        });

        server.connect().unwrap();
        let error = server.receive_envelope().unwrap_err();

        assert!(
            matches!(error, BrokerError::TruncatedControlFrame),
            "unexpected error: {error:?}"
        );
        client.join().unwrap();
    }
}
