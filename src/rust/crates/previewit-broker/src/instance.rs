use std::io;

use thiserror::Error;
use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, ReleaseMutex, WaitForSingleObject,
};

use crate::windows_security::{CurrentUserSecurity, OwnedHandle, WindowsSecurityError};

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error("the Broker product id is invalid")]
    InvalidProductId,
    #[error("the instance contender no longer owns a Mutex handle")]
    ContenderConsumed,
    #[error("{operation} failed: {source}")]
    Windows {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

impl From<WindowsSecurityError> for InstanceError {
    fn from(error: WindowsSecurityError) -> Self {
        let WindowsSecurityError::Windows { operation, source } = error;
        Self::Windows { operation, source }
    }
}

pub enum InstanceRole {
    Primary(InstanceLease),
    Secondary(InstanceContender),
}

pub struct InstanceLease {
    handle: Option<OwnedHandle>,
    mutex_name: String,
    pipe_name: String,
}

impl InstanceLease {
    pub fn elect(product_id: &str) -> Result<InstanceRole, InstanceError> {
        validate_product_id(product_id)?;
        let session_id = current_session_id()?;
        let mutex_name = format!(r"Local\{product_id}.{session_id}");
        let pipe_name = format!("{product_id}.{session_id}");
        let security = CurrentUserSecurity::new()?;
        let wide_name = wide(&mutex_name);
        let handle = unsafe { CreateMutexW(security.attributes(), 0, wide_name.as_ptr()) };
        if handle.is_null() {
            return Err(last_error("CreateMutexW"));
        }
        let handle = OwnedHandle::new(handle);

        match unsafe { WaitForSingleObject(handle.raw(), 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(InstanceRole::Primary(Self {
                handle: Some(handle),
                mutex_name,
                pipe_name,
            })),
            WAIT_TIMEOUT => Ok(InstanceRole::Secondary(InstanceContender {
                handle: Some(handle),
                mutex_name,
                pipe_name,
            })),
            _ => Err(last_error("WaitForSingleObject(instance Mutex)")),
        }
    }

    pub fn mutex_name(&self) -> &str {
        &self.mutex_name
    }

    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }
}

impl Drop for InstanceLease {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                ReleaseMutex(handle.raw());
            }
        }
    }
}

pub struct InstanceContender {
    handle: Option<OwnedHandle>,
    mutex_name: String,
    pipe_name: String,
}

impl InstanceContender {
    pub fn try_take_over(&mut self) -> Result<Option<InstanceLease>, InstanceError> {
        let handle = self
            .handle
            .as_ref()
            .ok_or(InstanceError::ContenderConsumed)?;
        match unsafe { WaitForSingleObject(handle.raw(), 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(InstanceLease {
                handle: self.handle.take(),
                mutex_name: self.mutex_name.clone(),
                pipe_name: self.pipe_name.clone(),
            })),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(last_error("WaitForSingleObject(instance contender)")),
        }
    }

    pub fn mutex_name(&self) -> &str {
        &self.mutex_name
    }

    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }
}

fn validate_product_id(product_id: &str) -> Result<(), InstanceError> {
    let valid = !product_id.is_empty()
        && product_id.len() <= 128
        && product_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(InstanceError::InvalidProductId)
    }
}

fn current_session_id() -> Result<u32, InstanceError> {
    let mut session_id = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id) } == 0 {
        return Err(last_error("ProcessIdToSessionId"));
    }
    Ok(session_id)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn last_error(operation: &'static str) -> InstanceError {
    InstanceError::Windows {
        operation,
        source: io::Error::last_os_error(),
    }
}
