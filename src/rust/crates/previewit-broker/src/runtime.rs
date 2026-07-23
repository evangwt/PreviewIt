use std::{collections::VecDeque, sync::Arc};

use crate::{
    BrokerCommandError, BrokerCommandServer, BrokerEvent, CommandAck, CommandRouter, EventSink,
    InstanceLease, InstanceRoleName, RouteDisposition, SessionEffect, SessionEvent, SessionPhase,
    SessionState, ValidatedCommand,
};

pub struct BrokerRuntime {
    lease: Option<InstanceLease>,
    server: Option<BrokerCommandServer>,
    router: CommandRouter,
    events: Arc<dyn EventSink>,
}

impl BrokerRuntime {
    pub fn new(
        lease: InstanceLease,
        server: BrokerCommandServer,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self::with_router(lease, server, CommandRouter::new(), events)
    }

    pub fn with_router(
        lease: InstanceLease,
        server: BrokerCommandServer,
        router: CommandRouter,
        events: Arc<dyn EventSink>,
    ) -> Self {
        events.emit(&BrokerEvent::InstanceElected {
            role: InstanceRoleName::Primary,
        });
        events.emit(&BrokerEvent::EndpointStarted);
        Self {
            lease: Some(lease),
            server: Some(server),
            router,
            events,
        }
    }

    pub fn handle(&mut self, command: ValidatedCommand) -> CommandAck {
        let before = StateSnapshot::capture(self.router.state());
        let command_id = command.command_id().as_str().to_owned();
        let result = self.router.route(command);

        match result.disposition {
            RouteDisposition::Routed => self.emit_ack(&result.ack),
            RouteDisposition::Duplicate => self
                .events
                .emit(&BrokerEvent::CommandDuplicate { command_id }),
        }
        self.emit_transition(before);
        self.drive_effects(result.effects);
        result.ack
    }

    pub fn handle_event(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
        let before = StateSnapshot::capture(self.router.state());
        let failed_request = match &event {
            SessionEvent::Failed(request_id) => Some(request_id.clone()),
            _ => None,
        };
        let effects = self.router.handle_event(event);

        if let Some(request_id) = failed_request
            && effects.iter().any(
                |effect| matches!(effect, SessionEffect::Cleanup(actual) if actual == &request_id),
            )
        {
            self.events.emit(&BrokerEvent::SessionFailed {
                request_id,
                reason: "reported-failure",
            });
        }
        self.emit_transition(before);
        self.drive_effects(effects.clone());
        effects
    }

    pub fn run(&mut self) -> Result<(), BrokerCommandError> {
        loop {
            let pending = match self
                .server
                .as_ref()
                .ok_or(BrokerCommandError::ListenerStopped)?
                .receive()
            {
                Ok(pending) => pending,
                Err(BrokerCommandError::Transport(crate::BrokerError::StartupTimeout)) => continue,
                Err(error) => return Err(error),
            };
            let command_id = pending.command().command_id().as_str().to_owned();
            let ack = self.handle(pending.command().clone());
            if let Err(error) = pending.respond(ack) {
                self.events.emit(&BrokerEvent::CommandRejected {
                    command_id: Some(command_id),
                    reason: error.code(),
                });
            }
        }
    }

    pub fn shutdown(&mut self) -> Result<(), BrokerCommandError> {
        let endpoint_result = if let Some(mut server) = self.server.take() {
            let result = server.shutdown();
            drop(server);
            self.events.emit(&BrokerEvent::EndpointStopped);
            result
        } else {
            Ok(())
        };

        if let Some(lease) = self.lease.take() {
            drop(lease);
            self.events.emit(&BrokerEvent::LeaseReleased);
        }
        endpoint_result
    }

    fn emit_ack(&self, ack: &CommandAck) {
        match ack {
            CommandAck::OpenAccepted {
                command_id,
                request_id,
            } => self.events.emit(&BrokerEvent::CommandAccepted {
                command_id: command_id.as_str().to_owned(),
                request_id: Some(request_id.clone()),
            }),
            CommandAck::CloseAccepted { command_id } => {
                self.events.emit(&BrokerEvent::CommandAccepted {
                    command_id: command_id.as_str().to_owned(),
                    request_id: None,
                });
            }
            CommandAck::Rejected { command_id, reason } => {
                self.events.emit(&BrokerEvent::CommandRejected {
                    command_id: command_id
                        .as_ref()
                        .map(|command_id| command_id.as_str().to_owned()),
                    reason: reason.as_str(),
                });
            }
        }
    }

    fn drive_effects(&mut self, initial: Vec<SessionEffect>) {
        let mut pending: VecDeque<_> = initial.into();
        while let Some(effect) = pending.pop_front() {
            match effect {
                SessionEffect::Cancel(request_id) | SessionEffect::Cleanup(request_id) => {
                    let before = StateSnapshot::capture(self.router.state());
                    pending.extend(
                        self.router
                            .handle_event(SessionEvent::CleanupComplete(request_id)),
                    );
                    self.emit_transition(before);
                }
                SessionEffect::StaleIgnored { expected, actual } => {
                    self.events
                        .emit(&BrokerEvent::StaleIgnored { expected, actual });
                }
                SessionEffect::BeginResolve(_)
                | SessionEffect::BeginPrepare(_)
                | SessionEffect::BeginRender(_)
                | SessionEffect::PublishReady(_)
                | SessionEffect::Superseded(_) => {}
            }
        }
    }

    fn emit_transition(&self, before: StateSnapshot) {
        let after = StateSnapshot::capture(self.router.state());
        if before != after {
            self.events.emit(&BrokerEvent::SessionTransition {
                from: before.phase,
                to: after.phase,
                request_id: after.request_id,
            });
        }
    }
}

impl Drop for BrokerRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[derive(PartialEq, Eq)]
struct StateSnapshot {
    phase: SessionPhase,
    request_id: Option<String>,
}

impl StateSnapshot {
    fn capture(state: &SessionState) -> Self {
        Self {
            phase: state.phase(),
            request_id: state.request_id().map(ToOwned::to_owned),
        }
    }
}
