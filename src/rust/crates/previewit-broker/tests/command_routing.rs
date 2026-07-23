use std::collections::VecDeque;
use std::path::PathBuf;

use previewit_broker::{
    BrokerCommand, CommandAck, CommandId, CommandRouter, PreviewRequest, RouteDisposition,
    SessionEffect, SessionState, ValidatedCommand,
};

fn absolute_path(label: &str) -> PathBuf {
    PathBuf::from(format!(r"C:\definitely-missing-previewit\{label}.txt"))
}

fn open(command_id: &str, path: PathBuf) -> ValidatedCommand {
    ValidatedCommand::new(
        CommandId::parse(command_id).unwrap(),
        BrokerCommand::Open(path),
    )
    .unwrap()
}

fn close(command_id: &str) -> ValidatedCommand {
    ValidatedCommand::new(CommandId::parse(command_id).unwrap(), BrokerCommand::Close).unwrap()
}

fn router_with_ids(ids: &[&str], capacity: usize) -> CommandRouter {
    let mut ids: VecDeque<String> = ids.iter().map(|id| (*id).to_owned()).collect();
    CommandRouter::with_id_source(capacity, move || ids.pop_front().expect("request id"))
}

#[test]
fn absolute_missing_path_enters_resolving_without_filesystem_io() {
    let path = absolute_path("missing");
    let mut router = router_with_ids(&["request-1"], 8);

    let result = router.route(open("command-1", path.clone()));

    assert!(matches!(
        result.ack,
        CommandAck::OpenAccepted { ref request_id, .. } if request_id == "request-1"
    ));
    assert_eq!(result.disposition, RouteDisposition::Routed);
    assert_eq!(
        result.effects,
        vec![SessionEffect::BeginResolve(PreviewRequest {
            request_id: "request-1".into(),
            path: path.clone(),
        })]
    );
    assert_eq!(
        router.state(),
        &SessionState::Resolving(PreviewRequest {
            request_id: "request-1".into(),
            path,
        })
    );
}

#[test]
fn close_is_accepted_and_idempotent() {
    let path = absolute_path("open");
    let mut router = router_with_ids(&["request-1"], 8);
    router.route(open("command-open", path.clone()));

    let first = router.route(close("command-close-1"));
    let second = router.route(close("command-close-2"));

    assert!(matches!(first.ack, CommandAck::CloseAccepted { .. }));
    assert_eq!(first.disposition, RouteDisposition::Routed);
    assert_eq!(
        first.effects,
        vec![SessionEffect::Cancel("request-1".into())]
    );
    assert!(matches!(second.ack, CommandAck::CloseAccepted { .. }));
    assert_eq!(second.disposition, RouteDisposition::Routed);
    assert!(second.effects.is_empty());
    assert_eq!(
        router.state(),
        &SessionState::Closing {
            old: PreviewRequest {
                request_id: "request-1".into(),
                path,
            },
            next: None,
        }
    );
}

#[test]
fn duplicate_command_has_explicit_disposition_and_no_second_transition() {
    let path = absolute_path("duplicate");
    let mut router = router_with_ids(&["request-1", "request-unused"], 8);

    let first = router.route(open("command-duplicate", path.clone()));
    let state_after_first = router.state().clone();
    let duplicate = router.route(open("command-duplicate", path));

    assert_eq!(duplicate.ack, first.ack);
    assert_eq!(first.disposition, RouteDisposition::Routed);
    assert_eq!(duplicate.disposition, RouteDisposition::Duplicate);
    assert!(!first.effects.is_empty());
    assert!(duplicate.effects.is_empty());
    assert_eq!(router.state(), &state_after_first);
}

#[test]
fn duplicate_cache_evicts_the_oldest_command() {
    let path = absolute_path("eviction");
    let mut router = router_with_ids(&["request-1", "request-2", "request-3", "request-4"], 2);

    let first = router.route(open("command-1", path.clone()));
    router.route(open("command-2", path.clone()));
    router.route(open("command-3", path.clone()));
    let after_eviction = router.route(open("command-1", path));

    assert!(
        matches!(first.ack, CommandAck::OpenAccepted { ref request_id, .. }
        if request_id == "request-1")
    );
    assert!(
        matches!(after_eviction.ack, CommandAck::OpenAccepted { ref request_id, .. }
        if request_id == "request-4")
    );
    assert_eq!(after_eviction.disposition, RouteDisposition::Routed);
}

#[test]
fn rapid_replacement_stays_closing_and_keeps_only_the_latest_pending_request() {
    let path = absolute_path("replacement");
    let mut router = router_with_ids(&["request-1", "request-2", "request-3"], 8);

    router.route(open("command-1", path.clone()));
    let second = router.route(open("command-2", path.clone()));
    let latest = router.route(open("command-3", path.clone()));

    assert_eq!(
        second.effects,
        vec![SessionEffect::Cancel("request-1".into())]
    );
    assert_eq!(
        latest.effects,
        vec![SessionEffect::Superseded("request-2".into())]
    );
    assert_eq!(second.disposition, RouteDisposition::Routed);
    assert_eq!(latest.disposition, RouteDisposition::Routed);
    assert_eq!(
        router.state(),
        &SessionState::Closing {
            old: PreviewRequest {
                request_id: "request-1".into(),
                path: path.clone(),
            },
            next: Some(PreviewRequest {
                request_id: "request-3".into(),
                path,
            }),
        }
    );
}
