use std::collections::VecDeque;

use uuid::Uuid;

use crate::{
    BrokerCommand, CommandAck, CommandId, PreviewRequest, SessionEffect, SessionEvent,
    SessionReducer, SessionState, ValidatedCommand,
};

const DEFAULT_CACHE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteDisposition {
    Routed,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteResult {
    pub ack: CommandAck,
    pub effects: Vec<SessionEffect>,
    pub disposition: RouteDisposition,
}

pub struct CommandRouter {
    reducer: SessionReducer,
    cache: VecDeque<(CommandId, CommandAck)>,
    cache_capacity: usize,
    request_ids: Box<dyn FnMut() -> String>,
}

impl CommandRouter {
    pub fn new() -> Self {
        Self::with_id_source(DEFAULT_CACHE_CAPACITY, || Uuid::new_v4().to_string())
    }

    pub fn with_id_source(
        cache_capacity: usize,
        request_ids: impl FnMut() -> String + 'static,
    ) -> Self {
        Self {
            reducer: SessionReducer::new(),
            cache: VecDeque::with_capacity(cache_capacity),
            cache_capacity: cache_capacity.max(1),
            request_ids: Box::new(request_ids),
        }
    }

    pub fn state(&self) -> &SessionState {
        self.reducer.state()
    }

    pub fn route(&mut self, command: ValidatedCommand) -> RouteResult {
        let (command_id, command) = command.into_parts();
        if let Some((_, ack)) = self.cache.iter().find(|(cached, _)| cached == &command_id) {
            return RouteResult {
                ack: ack.clone(),
                effects: Vec::new(),
                disposition: RouteDisposition::Duplicate,
            };
        }

        let (ack, effects) = match command {
            BrokerCommand::Open(path) => {
                let request_id = (self.request_ids)();
                let effects = self.handle_event(SessionEvent::Open(PreviewRequest {
                    request_id: request_id.clone(),
                    path,
                }));
                (
                    CommandAck::OpenAccepted {
                        command_id: command_id.clone(),
                        request_id,
                    },
                    effects,
                )
            }
            BrokerCommand::Close => (
                CommandAck::CloseAccepted {
                    command_id: command_id.clone(),
                },
                self.handle_event(SessionEvent::Close),
            ),
        };

        self.remember(command_id, ack.clone());
        RouteResult {
            ack,
            effects,
            disposition: RouteDisposition::Routed,
        }
    }

    pub(crate) fn handle_event(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
        let mut pending: VecDeque<_> = self.reducer.handle(event).into();
        let mut effects = Vec::new();
        while let Some(effect) = pending.pop_front() {
            let cleanup_id = match &effect {
                SessionEffect::Cancel(request_id) | SessionEffect::Cleanup(request_id) => {
                    Some(request_id.clone())
                }
                _ => None,
            };
            effects.push(effect);
            if let Some(request_id) = cleanup_id {
                pending.extend(
                    self.reducer
                        .handle(SessionEvent::CleanupComplete(request_id)),
                );
            }
        }
        effects
    }

    fn remember(&mut self, command_id: CommandId, ack: CommandAck) {
        if self.cache.len() == self.cache_capacity {
            self.cache.pop_front();
        }
        self.cache.push_back((command_id, ack));
    }
}

impl Default for CommandRouter {
    fn default() -> Self {
        Self::new()
    }
}
