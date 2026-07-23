use std::collections::VecDeque;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use previewit_broker::{CommandRouter, PreviewRequest, SessionEffect, SessionState};
use previewit_protocol::v0::{
    BrokerControlRequest, ClosePreview, OpenPath, broker_control_request,
};

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

fn open(command_id: &str, path: &Path) -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: command_id.into(),
        command: Some(broker_control_request::Command::OpenPath(OpenPath {
            path_utf16le: path
                .as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect(),
        })),
    }
}

fn close(command_id: &str) -> BrokerControlRequest {
    BrokerControlRequest {
        protocol_major: 0,
        protocol_minor: 1,
        command_id: command_id.into(),
        command: Some(broker_control_request::Command::ClosePreview(
            ClosePreview {},
        )),
    }
}

fn router_with_ids(ids: &[&str], capacity: usize) -> CommandRouter {
    let mut ids: VecDeque<String> = ids.iter().map(|id| (*id).to_owned()).collect();
    CommandRouter::with_id_source(capacity, move || ids.pop_front().expect("request id"))
}

#[test]
fn absolute_existing_path_is_accepted() {
    let path = fixture();
    let request = PreviewRequest {
        request_id: "request-1".into(),
        path: path.clone(),
    };
    let mut router = router_with_ids(&["request-1"], 8);

    let (response, effects) = router.route(open("command-1", &path));

    assert!(response.accepted);
    assert_eq!(response.command_id, "command-1");
    assert_eq!(response.request_id, "request-1");
    assert!(response.error_code.is_empty());
    assert_eq!(effects, vec![SessionEffect::BeginResolve(request.clone())]);
    assert_eq!(router.state(), &SessionState::Resolving(request));
}

#[test]
fn relative_and_missing_paths_are_rejected_before_allocating_request_ids() {
    let path = fixture();
    let missing = path.with_file_name("missing-previewit-fixture.txt");
    assert!(!missing.exists());
    let mut router = router_with_ids(&["request-after-rejections"], 8);

    let (relative, relative_effects) = router.route(open(
        "command-relative",
        Path::new(r"tests\fixtures\handles\read-only.txt"),
    ));
    let (absent, absent_effects) = router.route(open("command-missing", &missing));
    let (accepted, _) = router.route(open("command-valid", &path));

    assert!(!relative.accepted);
    assert_eq!(relative.error_code, "path-not-absolute");
    assert!(relative_effects.is_empty());
    assert!(!absent.accepted);
    assert_eq!(absent.error_code, "path-not-found");
    assert!(absent_effects.is_empty());
    assert_eq!(accepted.request_id, "request-after-rejections");
}

#[test]
fn close_is_accepted_and_idempotent() {
    let path = fixture();
    let mut router = router_with_ids(&["request-1"], 8);
    router.route(open("command-open", &path));

    let (first, first_effects) = router.route(close("command-close-1"));
    let (second, second_effects) = router.route(close("command-close-2"));

    assert!(first.accepted);
    assert!(first.request_id.is_empty());
    assert_eq!(
        first_effects,
        vec![SessionEffect::Cancel("request-1".into())]
    );
    assert!(second.accepted);
    assert!(second.request_id.is_empty());
    assert!(second_effects.is_empty());
    assert_eq!(router.state(), &SessionState::Idle);
}

#[test]
fn duplicate_command_replays_response_without_second_transition() {
    let path = fixture();
    let request = open("command-duplicate", &path);
    let mut router = router_with_ids(&["request-1", "request-unused"], 8);

    let (first, first_effects) = router.route(request.clone());
    let state_after_first = router.state().clone();
    let (duplicate, duplicate_effects) = router.route(request);

    assert_eq!(duplicate, first);
    assert!(!first_effects.is_empty());
    assert!(duplicate_effects.is_empty());
    assert_eq!(router.state(), &state_after_first);
}

#[test]
fn duplicate_cache_evicts_the_oldest_command() {
    let path = fixture();
    let mut router = router_with_ids(&["request-1", "request-2", "request-3", "request-4"], 2);

    let (first, _) = router.route(open("command-1", &path));
    router.route(open("command-2", &path));
    router.route(open("command-3", &path));
    let (after_eviction, _) = router.route(open("command-1", &path));

    assert_eq!(first.request_id, "request-1");
    assert_eq!(after_eviction.request_id, "request-4");
}

#[test]
fn rapid_replacement_cleans_up_each_old_request_and_keeps_the_latest() {
    let path = fixture();
    let mut router = router_with_ids(&["request-1", "request-2", "request-3"], 8);

    router.route(open("command-1", &path));
    let (_, second_effects) = router.route(open("command-2", &path));
    let (_, latest_effects) = router.route(open("command-3", &path));

    assert_eq!(
        second_effects,
        vec![
            SessionEffect::Cancel("request-1".into()),
            SessionEffect::BeginResolve(PreviewRequest {
                request_id: "request-2".into(),
                path: path.clone(),
            }),
        ]
    );
    assert_eq!(
        latest_effects,
        vec![
            SessionEffect::Cancel("request-2".into()),
            SessionEffect::BeginResolve(PreviewRequest {
                request_id: "request-3".into(),
                path: path.clone(),
            }),
        ]
    );
    assert_eq!(
        router.state(),
        &SessionState::Resolving(PreviewRequest {
            request_id: "request-3".into(),
            path,
        })
    );
}
