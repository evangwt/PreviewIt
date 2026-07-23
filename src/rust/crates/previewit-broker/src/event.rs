use crate::SessionPhase;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceRoleName {
    Primary,
    Secondary,
}

impl InstanceRoleName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrokerEvent {
    InstanceElected {
        role: InstanceRoleName,
    },
    EndpointStarted,
    EndpointStopped,
    LeaseReleased,
    CommandAccepted {
        command_id: String,
        request_id: Option<String>,
    },
    CommandRejected {
        command_id: Option<String>,
        reason: &'static str,
    },
    CommandDuplicate {
        command_id: String,
    },
    CommandQueueFull {
        command_id: String,
    },
    SessionTransition {
        from: SessionPhase,
        to: SessionPhase,
        request_id: Option<String>,
    },
    StaleIgnored {
        expected: Option<String>,
        actual: String,
    },
    SessionFailed {
        request_id: String,
        reason: &'static str,
    },
}

impl BrokerEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::InstanceElected { .. } => "instance-elected",
            Self::EndpointStarted => "endpoint-started",
            Self::EndpointStopped => "endpoint-stopped",
            Self::LeaseReleased => "lease-released",
            Self::CommandAccepted { .. } => "command-accepted",
            Self::CommandRejected { .. } => "command-rejected",
            Self::CommandDuplicate { .. } => "command-duplicate",
            Self::CommandQueueFull { .. } => "command-queue-full",
            Self::SessionTransition { .. } => "session-transition",
            Self::StaleIgnored { .. } => "stale-ignored",
            Self::SessionFailed { .. } => "session-failed",
        }
    }
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &BrokerEvent);
}

pub(crate) struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: &BrokerEvent) {}
}

pub struct StderrEventSink;

impl EventSink for StderrEventSink {
    fn emit(&self, event: &BrokerEvent) {
        match event {
            BrokerEvent::InstanceElected { role } => {
                eprintln!("event={} role={}", event.name(), role.as_str());
            }
            BrokerEvent::CommandAccepted {
                command_id,
                request_id,
            } => eprintln!(
                "event={} command_id={} request_id={}",
                event.name(),
                command_id,
                request_id.as_deref().unwrap_or("")
            ),
            BrokerEvent::CommandRejected { command_id, reason } => eprintln!(
                "event={} command_id={} error_code={reason}",
                event.name(),
                command_id.as_deref().unwrap_or("")
            ),
            BrokerEvent::CommandDuplicate { command_id }
            | BrokerEvent::CommandQueueFull { command_id } => {
                eprintln!("event={} command_id={command_id}", event.name());
            }
            BrokerEvent::SessionTransition {
                from,
                to,
                request_id,
            } => eprintln!(
                "event={} from={} to={} request_id={}",
                event.name(),
                from.as_str(),
                to.as_str(),
                request_id.as_deref().unwrap_or("")
            ),
            BrokerEvent::StaleIgnored { expected, actual } => eprintln!(
                "event={} expected_request_id={} actual_request_id={actual}",
                event.name(),
                expected.as_deref().unwrap_or("")
            ),
            BrokerEvent::SessionFailed { request_id, reason } => eprintln!(
                "event={} request_id={request_id} error_code={reason}",
                event.name()
            ),
            BrokerEvent::EndpointStarted
            | BrokerEvent::EndpointStopped
            | BrokerEvent::LeaseReleased => eprintln!("event={}", event.name()),
        }
    }
}
