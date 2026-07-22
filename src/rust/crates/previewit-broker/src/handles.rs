use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use thiserror::Error;
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileSizeEx, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

#[derive(Debug, Error)]
pub enum HandleError {
    #[error("{operation} failed: {source}")]
    Windows {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("the file size is not representable as an unsigned 64-bit value")]
    InvalidSize,
    #[error("the preview path has no display name")]
    MissingDisplayName,
}

pub struct ReadOnlyDocument {
    source: OwnedHandle,
    size: u64,
    display_name: String,
}

impl ReadOnlyDocument {
    pub fn open(path: &Path) -> Result<Self, HandleError> {
        let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null::<SECURITY_ATTRIBUTES>(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateFileW"));
        }

        let source = OwnedHandle(handle);
        let mut size = 0_i64;
        if unsafe { GetFileSizeEx(source.0, &mut size) } == 0 {
            return Err(last_error("GetFileSizeEx"));
        }
        let size = u64::try_from(size).map_err(|_| HandleError::InvalidSize)?;
        let display_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or(HandleError::MissingDisplayName)?;

        Ok(Self {
            source,
            size,
            display_name,
        })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// # Safety
    ///
    /// `target_process` must be a live process handle with permission to receive
    /// a duplicated handle and must remain valid until the returned guard is
    /// consumed or dropped.
    pub unsafe fn duplicate_into(
        &self,
        target_process: HANDLE,
    ) -> Result<RemoteHandle, HandleError> {
        let mut target_handle = null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.source.0,
                target_process,
                &mut target_handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(last_error("DuplicateHandle"));
        }

        Ok(RemoteHandle {
            target_process,
            target_handle,
            transferred: false,
        })
    }
}

pub struct RemoteHandle {
    target_process: HANDLE,
    target_handle: HANDLE,
    transferred: bool,
}

impl RemoteHandle {
    pub fn value(&self) -> u64 {
        self.target_handle as usize as u64
    }

    pub fn into_transferred_value(mut self) -> u64 {
        self.transferred = true;
        self.value()
    }
}

impl Drop for RemoteHandle {
    fn drop(&mut self) {
        if self.transferred {
            return;
        }

        let mut local_handle = null_mut();
        let duplicated = unsafe {
            DuplicateHandle(
                self.target_process,
                self.target_handle,
                GetCurrentProcess(),
                &mut local_handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS | DUPLICATE_CLOSE_SOURCE,
            )
        };
        if duplicated != 0 && !local_handle.is_null() {
            unsafe {
                CloseHandle(local_handle);
            }
        }
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub fn current_process_handle_count() -> Result<u32, HandleError> {
    let mut count = 0;
    if unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) } == 0 {
        return Err(last_error("GetProcessHandleCount"));
    }
    Ok(count)
}

fn last_error(operation: &'static str) -> HandleError {
    HandleError::Windows {
        operation,
        source: io::Error::from_raw_os_error(unsafe { GetLastError() } as i32),
    }
}
