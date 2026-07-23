use std::ffi::c_void;
use std::ptr::null_mut;
use std::time::Duration;

use previewit_broker::{
    BrokerCommandClient, BrokerCommandServer, CommandAck, current_user_sid_for_inspection,
};
use previewit_protocol::v0::{BrokerControlRequest, ClosePreview, broker_control_request};
use previewit_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR};
use uuid::Uuid;
use windows_sys::Win32::Foundation::{
    ERROR_SUCCESS, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GRANT_ACCESS, GetExplicitEntriesFromAclW, GetSecurityInfo,
    SE_KERNEL_OBJECT, TRUSTEE_IS_SID,
};
use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
use windows_sys::Win32::System::Pipes::{GetNamedPipeInfo, PIPE_REJECT_REMOTE_CLIENTS};

const TIMEOUT: Duration = Duration::from_secs(2);
const SYSTEM_SID: &str = "S-1-5-18";

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

struct PipeInspection {
    allowed_sids: Vec<String>,
    handle_inheritable: bool,
    rejects_remote_clients: bool,
}

fn pipe_name(label: &str) -> String {
    format!(
        "PreviewIt.Test.Security.{label}.{}",
        Uuid::new_v4().simple()
    )
}

fn close_request() -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: "command-security".into(),
        command: Some(broker_control_request::Command::ClosePreview(
            ClosePreview {},
        )),
    }
}

fn inspect_command_pipe(handle_value: usize) -> PipeInspection {
    let handle = handle_value as HANDLE;
    let mut handle_flags = 0;
    assert_ne!(
        unsafe { GetHandleInformation(handle, &mut handle_flags) },
        0,
        "GetHandleInformation failed: {}",
        std::io::Error::last_os_error()
    );

    let mut pipe_flags = 0;
    assert_ne!(
        unsafe { GetNamedPipeInfo(handle, &mut pipe_flags, null_mut(), null_mut(), null_mut(),) },
        0,
        "GetNamedPipeInfo failed: {}",
        std::io::Error::last_os_error()
    );

    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(status, ERROR_SUCCESS, "GetSecurityInfo failed: {status}");
    let _descriptor = LocalAllocation(descriptor);
    assert!(!dacl.is_null(), "command pipe must have a DACL");

    let mut entry_count = 0;
    let mut entries = null_mut();
    let status = unsafe { GetExplicitEntriesFromAclW(dacl, &mut entry_count, &mut entries) };
    assert_eq!(
        status, ERROR_SUCCESS,
        "GetExplicitEntriesFromAclW failed: {status}"
    );
    assert!(entry_count > 0, "command pipe DACL must not be empty");
    assert!(!entries.is_null(), "ACL enumeration returned no entries");
    let _entries = LocalAllocation(entries.cast());
    let entries = unsafe { std::slice::from_raw_parts(entries, entry_count as usize) };
    let mut allowed_sids = Vec::with_capacity(entries.len());
    for entry in entries {
        assert_eq!(entry.grfAccessMode, GRANT_ACCESS);
        assert_eq!(entry.grfInheritance, 0);
        assert_eq!(entry.Trustee.TrusteeForm, TRUSTEE_IS_SID);
        allowed_sids.push(sid_to_string(entry.Trustee.ptstrName.cast()));
    }
    allowed_sids.sort();

    PipeInspection {
        allowed_sids,
        handle_inheritable: handle_flags & HANDLE_FLAG_INHERIT != 0,
        rejects_remote_clients: pipe_flags & PIPE_REJECT_REMOTE_CLIENTS != 0,
    }
}

fn sid_to_string(sid: *mut c_void) -> String {
    let mut text = null_mut();
    assert_ne!(
        unsafe { ConvertSidToStringSidW(sid, &mut text) },
        0,
        "ConvertSidToStringSidW failed: {}",
        std::io::Error::last_os_error()
    );
    let _text = LocalAllocation(text.cast());
    let length = (0..)
        .find(|index| unsafe { *text.add(*index) == 0 })
        .expect("SID string terminator");
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
}

#[test]
fn command_pipe_dacl_contains_only_system_and_current_user() {
    let name = pipe_name("dacl");
    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();

    let inspection = inspect_command_pipe(server.inspection_handle());
    let mut expected = vec![
        SYSTEM_SID.to_owned(),
        current_user_sid_for_inspection().unwrap(),
    ];
    expected.sort();

    assert_eq!(inspection.allowed_sids, expected);
    assert!(!inspection.handle_inheritable);
}

#[test]
fn command_pipe_rejects_remote_clients_and_allows_current_user() {
    let name = pipe_name("local-only");
    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    let inspection = inspect_command_pipe(server.inspection_handle());
    assert!(inspection.rejects_remote_clients);

    let client_name = name.clone();
    let client = std::thread::spawn(move || {
        BrokerCommandClient::send(&client_name, &close_request(), TIMEOUT, TIMEOUT)
    });
    let pending = server.receive().unwrap();
    let command_id = pending.command().command_id().clone();
    pending
        .respond(CommandAck::CloseAccepted { command_id })
        .unwrap();

    assert!(matches!(
        client.join().unwrap().unwrap(),
        CommandAck::CloseAccepted { .. }
    ));
}
