use std::io;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use previewit_protocol::v0::{Envelope, HelloAck, envelope};
use previewit_protocol::{MAX_CONTROL_FRAME, encode_frame};
use prost::Message;
use thiserror::Error;
use uuid::Uuid;
use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::windows_security::{CurrentUserSecurity, OwnedHandle, WindowsSecurityError};

const PROTOCOL_MAJOR: u32 = 0;
const PROTOCOL_MINOR: u32 = 1;
const REQUIRED_CAPABILITY: &str = "read-handle-v0";
const PIPE_BUFFER_SIZE: u32 = (MAX_CONTROL_FRAME + 4) as u32;
const CANCELLATION_GRACE: Duration = Duration::from_secs(1);

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
    handle: OwnedHandle,
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
        let full_name = wide(&format!(r"\\.\pipe\{name}"));
        let security = CurrentUserSecurity::new()?;

        let handle = unsafe {
            CreateNamedPipeW(
                full_name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                PIPE_BUFFER_SIZE,
                PIPE_BUFFER_SIZE,
                0,
                security.attributes(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateNamedPipeW"));
        }

        Ok(Self {
            handle: OwnedHandle::new(handle),
            name,
            startup_timeout,
            io_timeout,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn accept_hello(&self, expected_pid: u32) -> Result<Envelope, BrokerError> {
        self.connect()?;

        let mut actual_pid = 0;
        if unsafe { GetNamedPipeClientProcessId(self.handle.raw(), &mut actual_pid) } == 0 {
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

    pub fn send_hello_ack(&self, hello_ack: HelloAck) -> Result<(), BrokerError> {
        let envelope = Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            request_id: "handshake-1".to_owned(),
            payload: Some(envelope::Payload::HelloAck(hello_ack)),
        };
        self.send_envelope(&envelope)
    }

    pub fn send_envelope(&self, envelope: &Envelope) -> Result<(), BrokerError> {
        self.write_frame(&envelope.encode_to_vec())
    }

    pub fn receive_envelope(&self) -> Result<Envelope, BrokerError> {
        let payload = self.read_frame()?;
        Ok(Envelope::decode(payload.as_slice())?)
    }

    fn connect(&self) -> Result<(), BrokerError> {
        let event = create_event()?;
        let mut overlapped = Box::new(unsafe { zeroed::<OVERLAPPED>() });
        overlapped.hEvent = event.raw();

        let connected = unsafe { ConnectNamedPipe(self.handle.raw(), overlapped.as_mut()) };
        if connected != 0 {
            return Ok(());
        }

        let error = unsafe { GetLastError() };
        if error == ERROR_PIPE_CONNECTED {
            return Ok(());
        }
        if error != ERROR_IO_PENDING {
            return Err(error_code("ConnectNamedPipe", error));
        }

        wait_for_overlapped(
            self.handle.raw(),
            overlapped,
            event,
            self.startup_timeout,
            BrokerError::StartupTimeout,
            "GetOverlappedResult(ConnectNamedPipe)",
        )?;
        Ok(())
    }

    fn read_frame(&self) -> Result<Vec<u8>, BrokerError> {
        let deadline = Instant::now() + self.io_timeout;
        let mut length_bytes = [0_u8; 4];
        self.read_exact(&mut length_bytes, deadline)?;
        let declared = u32::from_le_bytes(length_bytes) as usize;
        if declared > MAX_CONTROL_FRAME {
            return Err(BrokerError::ControlFrameTooLarge { declared });
        }

        let mut payload = vec![0_u8; declared];
        self.read_exact(&mut payload, deadline)?;
        Ok(payload)
    }

    fn read_exact(&self, buffer: &mut [u8], deadline: Instant) -> Result<(), BrokerError> {
        let mut offset = 0;
        while offset < buffer.len() {
            let timeout = deadline
                .checked_duration_since(Instant::now())
                .ok_or(BrokerError::ReadTimeout)?;
            let read = self.read_once(&mut buffer[offset..], timeout)?;
            if read == 0 {
                return Err(BrokerError::TruncatedControlFrame);
            }
            offset += read;
        }
        Ok(())
    }

    fn read_once(&self, buffer: &mut [u8], timeout: Duration) -> Result<usize, BrokerError> {
        let event = create_event()?;
        let mut overlapped = Box::new(unsafe { zeroed::<OVERLAPPED>() });
        overlapped.hEvent = event.raw();
        let length = u32::try_from(buffer.len()).expect("control frame is bounded by 1 MiB");

        let started = unsafe {
            ReadFile(
                self.handle.raw(),
                buffer.as_mut_ptr().cast(),
                length,
                null_mut(),
                overlapped.as_mut(),
            )
        };
        if started == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(error_code("ReadFile", error));
            }
        }

        wait_for_overlapped(
            self.handle.raw(),
            overlapped,
            event,
            timeout,
            BrokerError::ReadTimeout,
            "GetOverlappedResult(ReadFile)",
        )
        .map(|read| read as usize)
    }

    fn write_frame(&self, payload: &[u8]) -> Result<(), BrokerError> {
        let frame = encode_frame(payload)?;
        let deadline = Instant::now() + self.io_timeout;
        let mut offset = 0;
        while offset < frame.len() {
            let timeout = deadline
                .checked_duration_since(Instant::now())
                .ok_or(BrokerError::WriteTimeout)?;
            let written = self.write_once(&frame[offset..], timeout)?;
            if written == 0 {
                return Err(error_code("WriteFile", 0));
            }
            offset += written;
        }
        Ok(())
    }

    fn write_once(&self, buffer: &[u8], timeout: Duration) -> Result<usize, BrokerError> {
        let event = create_event()?;
        let mut overlapped = Box::new(unsafe { zeroed::<OVERLAPPED>() });
        overlapped.hEvent = event.raw();
        let length = u32::try_from(buffer.len()).expect("control frame is bounded by 1 MiB");

        let started = unsafe {
            WriteFile(
                self.handle.raw(),
                buffer.as_ptr().cast(),
                length,
                null_mut(),
                overlapped.as_mut(),
            )
        };
        if started == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(error_code("WriteFile", error));
            }
        }

        wait_for_overlapped(
            self.handle.raw(),
            overlapped,
            event,
            timeout,
            BrokerError::WriteTimeout,
            "GetOverlappedResult(WriteFile)",
        )
        .map(|written| written as usize)
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        unsafe {
            DisconnectNamedPipe(self.handle.raw());
        }
    }
}

fn create_event() -> Result<OwnedHandle, BrokerError> {
    let handle = unsafe { CreateEventW(null(), 1, 0, null()) };
    if handle.is_null() {
        Err(last_error("CreateEventW"))
    } else {
        Ok(OwnedHandle::new(handle))
    }
}

fn wait_for_overlapped(
    handle: HANDLE,
    mut overlapped: Box<OVERLAPPED>,
    event: OwnedHandle,
    timeout: Duration,
    timeout_error: BrokerError,
    result_operation: &'static str,
) -> Result<u32, BrokerError> {
    let wait = unsafe { WaitForSingleObject(event.raw(), timeout_ms(timeout)) };
    if wait == WAIT_TIMEOUT {
        unsafe {
            CancelIoEx(handle, overlapped.as_mut());
        }
        if unsafe { WaitForSingleObject(event.raw(), timeout_ms(CANCELLATION_GRACE)) }
            != WAIT_OBJECT_0
        {
            std::mem::forget(overlapped);
            std::mem::forget(event);
        }
        return Err(timeout_error);
    }
    if wait != WAIT_OBJECT_0 {
        return Err(last_error("WaitForSingleObject"));
    }

    let mut transferred = 0;
    if unsafe { GetOverlappedResult(handle, overlapped.as_mut(), &mut transferred, 0) } == 0 {
        return Err(last_error(result_operation));
    }
    Ok(transferred)
}

fn timeout_ms(duration: Duration) -> u32 {
    duration.as_millis().clamp(1, u32::MAX as u128) as u32
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn last_error(operation: &'static str) -> BrokerError {
    BrokerError::Windows {
        operation,
        source: io::Error::last_os_error(),
    }
}

fn error_code(operation: &'static str, code: u32) -> BrokerError {
    BrokerError::Windows {
        operation,
        source: io::Error::from_raw_os_error(code as i32),
    }
}
