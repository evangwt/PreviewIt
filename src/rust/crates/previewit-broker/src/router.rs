use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use previewit_protocol::v0::{BrokerControlRequest, BrokerControlResponse, broker_control_request};
use uuid::Uuid;

use crate::{PreviewRequest, SessionEffect, SessionEvent, SessionReducer, SessionState};

const PROTOCOL_MAJOR: u32 = 0;
const PROTOCOL_MINOR: u32 = 1;
const DEFAULT_CACHE_CAPACITY: usize = 256;

pub struct CommandRouter {
    reducer: SessionReducer,
    cache: VecDeque<(String, BrokerControlResponse)>,
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

    pub fn route(
        &mut self,
        request: BrokerControlRequest,
    ) -> (BrokerControlResponse, Vec<SessionEffect>) {
        if let Some((_, response)) = self
            .cache
            .iter()
            .find(|(command_id, _)| command_id == &request.command_id)
        {
            return (response.clone(), Vec::new());
        }

        let command_id = request.command_id.clone();
        let (response, effects) = match request.command {
            Some(broker_control_request::Command::OpenPath(open)) => {
                match decode_path(&open.path_utf16le) {
                    Ok(path) if !path.is_absolute() => {
                        (rejected(&command_id, "path-not-absolute"), Vec::new())
                    }
                    Ok(path) if !path.exists() => {
                        (rejected(&command_id, "path-not-found"), Vec::new())
                    }
                    Ok(path) => {
                        let request_id = (self.request_ids)();
                        let effects = self.handle(SessionEvent::Open(PreviewRequest {
                            request_id: request_id.clone(),
                            path,
                        }));
                        (accepted(&command_id, &request_id), effects)
                    }
                    Err(code) => (rejected(&command_id, code), Vec::new()),
                }
            }
            Some(broker_control_request::Command::ClosePreview(_)) => {
                let effects = self.handle(SessionEvent::Close);
                (accepted(&command_id, ""), effects)
            }
            None => (rejected(&command_id, "invalid-command"), Vec::new()),
        };

        self.remember(command_id, response.clone());
        (response, effects)
    }

    fn handle(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
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

    fn remember(&mut self, command_id: String, response: BrokerControlResponse) {
        if self.cache.len() == self.cache_capacity {
            self.cache.pop_front();
        }
        self.cache.push_back((command_id, response));
    }
}

impl Default for CommandRouter {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_path(bytes: &[u8]) -> Result<PathBuf, &'static str> {
    if !bytes.len().is_multiple_of(2) {
        return Err("invalid-utf16");
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    if units.contains(&0) || char::decode_utf16(units.iter().copied()).any(|unit| unit.is_err()) {
        return Err("invalid-utf16");
    }
    Ok(PathBuf::from(OsString::from_wide(&units)))
}

fn accepted(command_id: &str, request_id: &str) -> BrokerControlResponse {
    BrokerControlResponse {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: command_id.to_owned(),
        accepted: true,
        request_id: request_id.to_owned(),
        error_code: String::new(),
    }
}

fn rejected(command_id: &str, error_code: &str) -> BrokerControlResponse {
    BrokerControlResponse {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: command_id.to_owned(),
        accepted: false,
        request_id: String::new(),
        error_code: error_code.to_owned(),
    }
}
