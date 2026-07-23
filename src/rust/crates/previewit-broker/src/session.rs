use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRequest {
    pub request_id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Resolving(PreviewRequest),
    Preparing(PreviewRequest),
    Rendering(PreviewRequest),
    Ready(PreviewRequest),
    Closing {
        old: PreviewRequest,
        next: Option<PreviewRequest>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    Idle,
    Resolving,
    Preparing,
    Rendering,
    Ready,
    Closing,
}

impl SessionPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Resolving => "resolving",
            Self::Preparing => "preparing",
            Self::Rendering => "rendering",
            Self::Ready => "ready",
            Self::Closing => "closing",
        }
    }
}

impl SessionState {
    pub fn phase(&self) -> SessionPhase {
        match self {
            Self::Idle => SessionPhase::Idle,
            Self::Resolving(_) => SessionPhase::Resolving,
            Self::Preparing(_) => SessionPhase::Preparing,
            Self::Rendering(_) => SessionPhase::Rendering,
            Self::Ready(_) => SessionPhase::Ready,
            Self::Closing { .. } => SessionPhase::Closing,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::Resolving(request)
            | Self::Preparing(request)
            | Self::Rendering(request)
            | Self::Ready(request) => Some(&request.request_id),
            Self::Closing { old, .. } => Some(&old.request_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Open(PreviewRequest),
    Close,
    Resolved(String),
    Prepared(String),
    Rendered(String),
    Failed(String),
    CleanupComplete(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEffect {
    BeginResolve(PreviewRequest),
    BeginPrepare(PreviewRequest),
    BeginRender(PreviewRequest),
    PublishReady(PreviewRequest),
    Cancel(String),
    Cleanup(String),
    Superseded(String),
    StaleIgnored {
        expected: Option<String>,
        actual: String,
    },
}

pub struct SessionReducer {
    state: SessionState,
}

impl SessionReducer {
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
        }
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn handle(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
        let current = std::mem::replace(&mut self.state, SessionState::Idle);
        let (next, effects) = match current {
            SessionState::Idle => match event {
                SessionEvent::Open(request) => {
                    let effect = SessionEffect::BeginResolve(request.clone());
                    (SessionState::Resolving(request), vec![effect])
                }
                SessionEvent::Close => (SessionState::Idle, Vec::new()),
                event => (SessionState::Idle, stale_event(None, event_id(&event))),
            },
            SessionState::Resolving(request) => {
                active_transition(request, event, ActivePhase::Resolving)
            }
            SessionState::Preparing(request) => {
                active_transition(request, event, ActivePhase::Preparing)
            }
            SessionState::Rendering(request) => {
                active_transition(request, event, ActivePhase::Rendering)
            }
            SessionState::Ready(request) => active_transition(request, event, ActivePhase::Ready),
            SessionState::Closing { old, next } => closing_transition(old, next, event),
        };
        self.state = next;
        effects
    }
}

#[derive(Clone, Copy)]
enum ActivePhase {
    Resolving,
    Preparing,
    Rendering,
    Ready,
}

fn active_transition(
    request: PreviewRequest,
    event: SessionEvent,
    phase: ActivePhase,
) -> (SessionState, Vec<SessionEffect>) {
    match event {
        SessionEvent::Open(next) => (
            SessionState::Closing {
                old: request.clone(),
                next: Some(next),
            },
            vec![SessionEffect::Cancel(request.request_id)],
        ),
        SessionEvent::Close => (
            SessionState::Closing {
                old: request.clone(),
                next: None,
            },
            vec![SessionEffect::Cancel(request.request_id)],
        ),
        SessionEvent::Failed(request_id) if request_id == request.request_id => (
            SessionState::Closing {
                old: request.clone(),
                next: None,
            },
            vec![SessionEffect::Cleanup(request.request_id)],
        ),
        SessionEvent::Resolved(request_id)
            if matches!(phase, ActivePhase::Resolving) && request_id == request.request_id =>
        {
            let effect = SessionEffect::BeginPrepare(request.clone());
            (SessionState::Preparing(request), vec![effect])
        }
        SessionEvent::Prepared(request_id)
            if matches!(phase, ActivePhase::Preparing) && request_id == request.request_id =>
        {
            let effect = SessionEffect::BeginRender(request.clone());
            (SessionState::Rendering(request), vec![effect])
        }
        SessionEvent::Rendered(request_id)
            if matches!(phase, ActivePhase::Rendering) && request_id == request.request_id =>
        {
            let effect = SessionEffect::PublishReady(request.clone());
            (SessionState::Ready(request), vec![effect])
        }
        event => (
            state_for_phase(phase, request.clone()),
            stale_event(Some(&request.request_id), event_id(&event)),
        ),
    }
}

fn state_for_phase(phase: ActivePhase, request: PreviewRequest) -> SessionState {
    match phase {
        ActivePhase::Resolving => SessionState::Resolving(request),
        ActivePhase::Preparing => SessionState::Preparing(request),
        ActivePhase::Rendering => SessionState::Rendering(request),
        ActivePhase::Ready => SessionState::Ready(request),
    }
}

fn closing_transition(
    old: PreviewRequest,
    next: Option<PreviewRequest>,
    event: SessionEvent,
) -> (SessionState, Vec<SessionEffect>) {
    match event {
        SessionEvent::Open(request) => {
            let effects = next
                .as_ref()
                .map(|pending| vec![SessionEffect::Superseded(pending.request_id.clone())])
                .unwrap_or_default();
            (
                SessionState::Closing {
                    old,
                    next: Some(request),
                },
                effects,
            )
        }
        SessionEvent::Close => {
            let effects = next
                .map(|pending| vec![SessionEffect::Superseded(pending.request_id)])
                .unwrap_or_default();
            (SessionState::Closing { old, next: None }, effects)
        }
        SessionEvent::CleanupComplete(request_id) if request_id == old.request_id => match next {
            Some(request) => {
                let effect = SessionEffect::BeginResolve(request.clone());
                (SessionState::Resolving(request), vec![effect])
            }
            None => (SessionState::Idle, Vec::new()),
        },
        event => (
            SessionState::Closing {
                old: old.clone(),
                next,
            },
            stale_event(Some(&old.request_id), event_id(&event)),
        ),
    }
}

fn event_id(event: &SessionEvent) -> String {
    match event {
        SessionEvent::Resolved(id)
        | SessionEvent::Prepared(id)
        | SessionEvent::Rendered(id)
        | SessionEvent::Failed(id)
        | SessionEvent::CleanupComplete(id) => id.clone(),
        SessionEvent::Open(request) => request.request_id.clone(),
        SessionEvent::Close => String::new(),
    }
}

fn stale_event(expected: Option<&str>, actual: String) -> Vec<SessionEffect> {
    vec![SessionEffect::StaleIgnored {
        expected: expected.map(ToOwned::to_owned),
        actual,
    }]
}

impl Default for SessionReducer {
    fn default() -> Self {
        Self::new()
    }
}
