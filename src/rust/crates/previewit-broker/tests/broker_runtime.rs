use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use previewit_broker::{
    BrokerCommand, BrokerCommandServer, BrokerEvent, BrokerRuntime, CommandId, CommandRouter,
    EventSink, InstanceLease, InstanceRole, SessionEvent, SessionPhase, ValidatedCommand,
};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Default)]
struct RecordingEventSink {
    events: Arc<Mutex<Vec<BrokerEvent>>>,
}

impl RecordingEventSink {
    fn names(&self) -> Vec<&'static str> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(BrokerEvent::name)
            .collect()
    }

    fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl EventSink for RecordingEventSink {
    fn emit(&self, event: &BrokerEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

fn validated_open(command_id: &str, path: PathBuf) -> ValidatedCommand {
    ValidatedCommand::new(
        CommandId::parse(command_id).unwrap(),
        BrokerCommand::Open(path),
    )
    .unwrap()
}

fn test_runtime(events: RecordingEventSink) -> (BrokerRuntime, String) {
    let product_id = format!("PreviewIt.Runtime.Test.{}", Uuid::new_v4().simple());
    let InstanceRole::Primary(lease) = InstanceLease::elect(&product_id).unwrap() else {
        panic!("unique runtime lease must elect primary");
    };
    let server = BrokerCommandServer::create(lease.pipe_name(), TIMEOUT, TIMEOUT).unwrap();
    let mut request_ids = VecDeque::from([
        "request-1".to_owned(),
        "request-2".to_owned(),
        "request-3".to_owned(),
    ]);
    let router =
        CommandRouter::with_id_source(8, move || request_ids.pop_front().expect("request id"));
    let runtime = BrokerRuntime::with_router(lease, server, router, Arc::new(events.clone()));
    events.clear();
    (runtime, product_id)
}

#[test]
fn duplicate_phase_and_stale_paths_emit_canonical_events() {
    let events = RecordingEventSink::default();
    let (mut runtime, _) = test_runtime(events.clone());
    let path = PathBuf::from(r"C:\definitely-missing-previewit\runtime.txt");

    runtime.handle(validated_open("command-1", path.clone()));
    runtime.handle(validated_open("command-1", path));
    runtime.handle_event(SessionEvent::Rendered("stale-request".into()));

    assert_eq!(
        events.names(),
        [
            "command-accepted",
            "session-transition",
            "command-duplicate",
            "stale-ignored",
        ]
    );
}

#[test]
fn current_failure_emits_failure_before_the_resulting_transition() {
    let events = RecordingEventSink::default();
    let (mut runtime, _) = test_runtime(events.clone());
    let path = PathBuf::from(r"C:\definitely-missing-previewit\runtime.txt");

    runtime.handle(validated_open("command-1", path));
    events.clear();
    runtime.handle_event(SessionEvent::Failed("request-1".into()));

    assert_eq!(
        events.names(),
        ["session-failed", "session-transition", "session-transition"]
    );
}

#[test]
fn replacement_emits_each_transition_while_runtime_drives_cleanup() {
    let events = RecordingEventSink::default();
    let (mut runtime, _) = test_runtime(events.clone());
    let path = PathBuf::from(r"C:\definitely-missing-previewit\runtime.txt");

    runtime.handle(validated_open("command-1", path.clone()));
    events.clear();
    runtime.handle(validated_open("command-2", path));

    let recorded = events.events.lock().unwrap().clone();
    assert!(matches!(
        recorded.as_slice(),
        [
            BrokerEvent::CommandAccepted {
                request_id: Some(next),
                ..
            },
            BrokerEvent::SessionTransition {
                from: SessionPhase::Resolving,
                to: SessionPhase::Closing,
                request_id: Some(old),
            },
            BrokerEvent::SessionTransition {
                from: SessionPhase::Closing,
                to: SessionPhase::Resolving,
                request_id: Some(promoted),
            },
        ] if next == "request-2" && old == "request-1" && promoted == "request-2"
    ));
}

#[test]
fn shutdown_stops_endpoint_before_releasing_lease() {
    let events = RecordingEventSink::default();
    let (mut runtime, product_id) = test_runtime(events.clone());

    runtime.shutdown().unwrap();

    assert_eq!(events.names(), ["endpoint-stopped", "lease-released"]);
    let InstanceRole::Primary(_replacement) = InstanceLease::elect(&product_id).unwrap() else {
        panic!("shutdown must release the session lease");
    };
}
