use std::path::PathBuf;

use previewit_broker::{PreviewRequest, SessionEffect, SessionEvent, SessionReducer, SessionState};

fn request(id: &str) -> PreviewRequest {
    PreviewRequest {
        request_id: id.to_owned(),
        path: PathBuf::from(format!(r"C:\fixtures\{id}.txt")),
    }
}

fn advance_to_rendering(reducer: &mut SessionReducer, request: &PreviewRequest) {
    reducer.handle(SessionEvent::Open(request.clone()));
    reducer.handle(SessionEvent::Resolved(request.request_id.clone()));
    reducer.handle(SessionEvent::Prepared(request.request_id.clone()));
}

fn advance_to_ready(reducer: &mut SessionReducer, request: &PreviewRequest) {
    advance_to_rendering(reducer, request);
    reducer.handle(SessionEvent::Rendered(request.request_id.clone()));
}

#[test]
fn idle_request_reaches_ready() {
    let request = request("request-1");
    let mut reducer = SessionReducer::new();

    assert_eq!(
        reducer.handle(SessionEvent::Open(request.clone())),
        vec![SessionEffect::BeginResolve(request.clone())]
    );
    assert_eq!(reducer.state(), &SessionState::Resolving(request.clone()));

    assert_eq!(
        reducer.handle(SessionEvent::Resolved(request.request_id.clone())),
        vec![SessionEffect::BeginPrepare(request.clone())]
    );
    assert_eq!(reducer.state(), &SessionState::Preparing(request.clone()));

    assert_eq!(
        reducer.handle(SessionEvent::Prepared(request.request_id.clone())),
        vec![SessionEffect::BeginRender(request.clone())]
    );
    assert_eq!(reducer.state(), &SessionState::Rendering(request.clone()));

    assert_eq!(
        reducer.handle(SessionEvent::Rendered(request.request_id.clone())),
        vec![SessionEffect::PublishReady(request.clone())]
    );
    assert_eq!(reducer.state(), &SessionState::Ready(request));
}

#[test]
fn latest_request_waits_for_old_cleanup() {
    let first = request("request-1");
    let second = request("request-2");
    let latest = request("request-3");
    let mut reducer = SessionReducer::new();
    advance_to_rendering(&mut reducer, &first);

    assert_eq!(
        reducer.handle(SessionEvent::Open(second.clone())),
        vec![SessionEffect::Cancel(first.request_id.clone())]
    );
    assert_eq!(
        reducer.state(),
        &SessionState::Closing {
            old: first.clone(),
            next: Some(second.clone()),
        }
    );

    assert_eq!(
        reducer.handle(SessionEvent::Open(latest.clone())),
        vec![SessionEffect::Superseded(second.request_id.clone())]
    );
    assert_eq!(
        reducer.state(),
        &SessionState::Closing {
            old: first.clone(),
            next: Some(latest.clone()),
        }
    );

    assert_eq!(
        reducer.handle(SessionEvent::CleanupComplete(first.request_id)),
        vec![SessionEffect::BeginResolve(latest.clone())]
    );
    assert_eq!(reducer.state(), &SessionState::Resolving(latest));
}

#[test]
fn close_discards_pending_request_and_returns_to_idle() {
    let first = request("request-1");
    let pending = request("request-2");
    let mut reducer = SessionReducer::new();
    advance_to_ready(&mut reducer, &first);
    reducer.handle(SessionEvent::Open(pending.clone()));

    assert_eq!(
        reducer.handle(SessionEvent::Close),
        vec![SessionEffect::Superseded(pending.request_id)]
    );
    assert_eq!(
        reducer.state(),
        &SessionState::Closing {
            old: first.clone(),
            next: None,
        }
    );

    assert!(
        reducer
            .handle(SessionEvent::CleanupComplete(first.request_id))
            .is_empty()
    );
    assert_eq!(reducer.state(), &SessionState::Idle);
    assert!(reducer.handle(SessionEvent::Close).is_empty());
}

#[test]
fn failed_request_is_cleaned_up() {
    let failed = request("request-1");
    let mut reducer = SessionReducer::new();
    advance_to_rendering(&mut reducer, &failed);

    assert_eq!(
        reducer.handle(SessionEvent::Failed(failed.request_id.clone())),
        vec![SessionEffect::Cleanup(failed.request_id.clone())]
    );
    assert_eq!(
        reducer.state(),
        &SessionState::Closing {
            old: failed.clone(),
            next: None,
        }
    );

    assert!(
        reducer
            .handle(SessionEvent::CleanupComplete(failed.request_id))
            .is_empty()
    );
    assert_eq!(reducer.state(), &SessionState::Idle);
}

#[test]
fn stale_events_do_not_change_current_request() {
    let current = request("request-2");
    let mut reducer = SessionReducer::new();
    reducer.handle(SessionEvent::Open(current.clone()));

    assert_eq!(
        reducer.handle(SessionEvent::Resolved("request-1".into())),
        vec![SessionEffect::StaleIgnored {
            expected: Some(current.request_id.clone()),
            actual: "request-1".into(),
        }]
    );
    assert_eq!(reducer.state(), &SessionState::Resolving(current.clone()));

    assert_eq!(
        reducer.handle(SessionEvent::CleanupComplete("request-1".into())),
        vec![SessionEffect::StaleIgnored {
            expected: Some(current.request_id.clone()),
            actual: "request-1".into(),
        }]
    );
    assert_eq!(reducer.state(), &SessionState::Resolving(current));
}

#[test]
fn close_cancels_an_active_request_once() {
    let active = request("request-1");
    let mut reducer = SessionReducer::new();
    reducer.handle(SessionEvent::Open(active.clone()));

    assert_eq!(
        reducer.handle(SessionEvent::Close),
        vec![SessionEffect::Cancel(active.request_id.clone())]
    );
    assert_eq!(
        reducer.state(),
        &SessionState::Closing {
            old: active,
            next: None,
        }
    );
    assert!(reducer.handle(SessionEvent::Close).is_empty());
}
