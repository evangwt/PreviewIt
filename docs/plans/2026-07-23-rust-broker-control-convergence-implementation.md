# Rust Broker Control Convergence Implementation Plan

> **For Claude/Codex:** REQUIRED SUB-SKILL: Use `@executing-plans` to implement this plan task-by-task. Use `@cs` Do rules and `@test-driven-development` for every production behavior.

**Goal:** Replace the unsafe and divergent Broker control implementation with one x64-only, bounded, lifecycle-safe Named Pipe transport and one authoritative request/ack/session contract.

**Architecture:** Tokio owns all Windows Named Pipe asynchronous I/O, framing deadlines, connection tasks, and cancellation; the existing current-user/SYSTEM security descriptor and session Mutex remain Win32-native. Protobuf is converted once at the endpoint into `ValidatedCommand`, a single `BrokerRuntime` owns routing/reducer state, and a typed `BrokerEvent` is the sole source for observable event names.

**Tech Stack:** Rust 2024, Tokio Windows Named Pipes, `windows-sys`, Prost, existing `previewit-protocol`, real Windows process/ACL integration tests, PowerShell foundation gate.

**Source of truth:** `.cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md`, especially the approved `## 实现设计`. If this plan and the Issue disagree, update the Issue first. Keep the Issue and behavior Explore open and the architecture Epic draft unless the user separately authorizes closing.

**Hard constraints:** Windows x64 only. Do not add ARM64 targets, matrices, artifacts, conditional branches, or promises. Do not add Shell Resolver, Viewer, Renderer, x86 Dialog Adapter, new Broker commands, or a second framing/event/error implementation.

---

## Task 1: Establish one Broker control contract

**Files:**

- Modify: `src/rust/crates/previewit-protocol/src/lib.rs`
- Create: `src/rust/crates/previewit-broker/src/control.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`
- Create: `src/rust/crates/previewit-broker/tests/broker_control_contract.rs`

**Step 1: Write failing contract tests**

Create table-driven tests for the approved precedence and ack invariants. Use the public domain interface, not helpers from `command.rs`:

```rust
#[test]
fn invalid_command_id_precedes_wrong_protocol_version() {
    let request = wire_close("bad/id", 99, 99);
    let rejection = BrokerControlContract::decode_request(request).unwrap_err();

    assert_eq!(rejection.code(), CommandRejectionCode::InvalidCommandId);
    assert_eq!(rejection.safe_command_id(), None);
}

#[test]
fn response_must_match_request_and_ack_shape() {
    let id = CommandId::parse("command-1").unwrap();
    let expected = ExpectedAck::open(id.clone());

    let mut wrong_id = accepted_open("command-2", "request-1");
    assert_eq!(
        BrokerControlContract::decode_response(&expected, wrong_id).unwrap_err().code(),
        "response-command-mismatch"
    );

    wrong_id.command_id = id.as_str().into();
    wrong_id.error_code = "must-be-empty".into();
    assert_eq!(
        BrokerControlContract::decode_response(&expected, wrong_id).unwrap_err().code(),
        "invalid-response-shape"
    );
}

#[test]
fn absolute_missing_path_is_structurally_valid() {
    let command = BrokerControlContract::decode_request(wire_open(
        "command-1",
        Path::new(r"C:\definitely-missing-previewit\file.txt"),
    ))
    .unwrap();

    assert!(matches!(command.command(), BrokerCommand::Open(path)
        if path == Path::new(r"C:\definitely-missing-previewit\file.txt")));
}
```

Also cover odd UTF-16LE, embedded NUL, overlong path, relative path, missing command, wrong major/minor after a valid ID, accepted Open without request ID, accepted response with an error, rejected response with a request ID, and rejected response without an error.

**Step 2: Run the focused suite and verify RED**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control_contract
```

Expected: FAIL because `BrokerControlContract`, `CommandId`, `BrokerCommand`, `ExpectedAck`, and rejection/ack types do not exist.

**Step 3: Add the minimal shared constants and domain types**

Export protocol constants once from `previewit-protocol`:

```rust
pub const PROTOCOL_MAJOR: u32 = 0;
pub const PROTOCOL_MINOR: u32 = 1;
pub const MAX_CONTROL_FRAME: usize = 1024 * 1024;
```

In `control.rs`, provide one conversion boundary:

```rust
pub struct CommandId(String);

pub enum BrokerCommand {
    Open(PathBuf),
    Close,
}

pub struct ValidatedCommand {
    command_id: CommandId,
    command: BrokerCommand,
}

pub enum CommandAck {
    Accepted { command_id: CommandId, request_id: Option<String> },
    Rejected { command_id: Option<CommandId>, reason: CommandRejectionCode },
}

pub struct BrokerControlContract;

impl BrokerControlContract {
    pub fn decode_request(
        request: BrokerControlRequest,
    ) -> Result<ValidatedCommand, CommandRejection>;

    pub fn encode_response(ack: &CommandAck) -> BrokerControlResponse;

    pub fn decode_response(
        expected: &ExpectedAck,
        response: BrokerControlResponse,
    ) -> Result<CommandAck, InvalidCommandResponse>;
}
```

Keep error strings behind `CommandRejectionCode::as_str()` and response-validation errors behind `InvalidCommandResponse::code()`. Delete or stop exporting any parallel response factory. Path conversion happens here exactly once; it performs only structural checks and never calls `exists`, `metadata`, or another filesystem API.

**Step 4: Run contract and protocol tests**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control_contract
cargo test --manifest-path src/rust/Cargo.toml -p previewit-protocol
```

Expected: all PASS. No test needs a real file fixture for path validation.

**Step 5: Run formatting/lint and commit**

Run:

```powershell
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
git add src/rust/crates/previewit-protocol/src/lib.rs src/rust/crates/previewit-broker/src/control.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/tests/broker_control_contract.rs src/rust/Cargo.lock
git commit -m "refactor: unify broker control contract"
```

## Task 2: Replace handwritten Overlapped I/O with one Tokio framed pipe

**Files:**

- Modify: `src/rust/crates/previewit-broker/Cargo.toml`
- Modify: `src/rust/crates/previewit-broker/src/pipe.rs`
- Modify: `src/rust/crates/previewit-broker/src/supervisor.rs`
- Modify: `src/rust/crates/previewit-broker/tests/dotnet_handshake.rs`
- Modify: `src/rust/crates/previewit-broker/tests/handle_transfer.rs`
- Modify: `src/rust/crates/previewit-broker/tests/supervision.rs`
- Modify: `src/rust/Cargo.lock`

**Step 1: Add a failing delayed-close transport test**

Exercise the read-pending path, not the existing immediate close path:

```rust
#[test]
fn delayed_partial_frame_is_stably_truncated() {
    let mut server = PipeServer::create_with_timeouts(
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .unwrap();
    let name = server.name().to_owned();
    let client = thread::spawn(move || {
        raw_write_hold_and_close(&name, &[10, 0, 0, 0, 1, 2], Duration::from_millis(300));
    });

    let error = server.receive_envelope().unwrap_err();
    assert!(matches!(error, BrokerError::TruncatedControlFrame));
    client.join().unwrap();
}
```

**Step 2: Verify the test exposes the existing defect**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker delayed_partial_frame_is_stably_truncated -- --exact
```

Expected: FAIL because the pending `GetOverlappedResult(ReadFile)` error becomes the generic Windows/transport error.

**Step 3: Add Tokio and implement the single async frame path**

Add only the needed Tokio features:

```toml
tokio = { version = "1.53.1", features = ["io-util", "macros", "net", "rt", "sync", "time"] }
```

Keep the frame codec in `previewit-protocol`; `pipe.rs` owns only async I/O and error normalization:

```rust
pub(crate) async fn read_frame<T>(
    io: &mut T,
    deadline: Duration,
) -> Result<Vec<u8>, BrokerError>
where
    T: AsyncRead + Unpin;

pub(crate) async fn write_frame<T>(
    io: &mut T,
    payload: &[u8],
    deadline: Duration,
) -> Result<(), BrokerError>
where
    T: AsyncWrite + Unpin;
```

Both functions wrap the complete operation in `tokio::time::timeout`. Normalize `UnexpectedEof`, `BrokenPipe`, and raw `ERROR_BROKEN_PIPE`/`ERROR_NO_DATA` during reads to `TruncatedControlFrame`. Preserve distinct startup/read/write timeouts.

Create `NamedPipeServer` through `ServerOptions` inside an entered Tokio runtime, using:

```rust
ServerOptions::new()
    .first_pipe_instance(first)
    .reject_remote_clients(true)
    .max_instances(max_instances)
    .in_buffer_size(PIPE_BUFFER_SIZE)
    .out_buffer_size(PIPE_BUFFER_SIZE)
```

Call `create_with_security_attributes_raw` with the existing current-user/SYSTEM attributes. Do not duplicate SID/SDDL logic.

Migrate `PipeServer` to an internal current-thread runtime plus `NamedPipeServer`. It may change its I/O methods from `&self` to `&mut self`; update `WorkerSupervisor` and tests rather than adding interior-mutable wrappers. Retain `GetNamedPipeClientProcessId` via `AsRawHandle`.

Delete `read_once`, `write_once`, `connect_pipe`, `wait_for_overlapped`, `CANCELLATION_GRACE`, and their direct `ReadFile`/`WriteFile`/`CancelIoEx` imports. There must be no old fallback.

**Step 4: Run all Worker boundary regressions**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test dotnet_handshake
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test handle_transfer
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test supervision
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker delayed_partial_frame_is_stably_truncated -- --exact
```

Expected: all PASS. Worker PID authentication, protocol negotiation, read-only handle transfer, hang/crash recovery, and stale rejection are unchanged.

**Step 5: Prove the unsafe implementation is gone and commit**

Run:

```powershell
rg -n "ReadFile|WriteFile|CancelIoEx|GetOverlappedResult|CANCELLATION_GRACE" src/rust/crates/previewit-broker/src/pipe.rs
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
```

Expected: `rg` has no matches; formatting and Clippy pass.

Commit:

```powershell
git add src/rust/crates/previewit-broker/Cargo.toml src/rust/crates/previewit-broker/src/pipe.rs src/rust/crates/previewit-broker/src/supervisor.rs src/rust/crates/previewit-broker/tests/dotnet_handshake.rs src/rust/crates/previewit-broker/tests/handle_transfer.rs src/rust/crates/previewit-broker/tests/supervision.rs src/rust/Cargo.lock
git commit -m "fix: unify safe named pipe transport"
```

## Task 3: Rebuild the command endpoint around owned connection tasks

**Files:**

- Modify: `src/rust/crates/previewit-broker/src/command.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`
- Modify: `src/rust/crates/previewit-broker/tests/broker_control.rs`

**Step 1: Add failing lifecycle, concurrency, and response tests**

Add real-pipe tests for four independent behaviors:

```rust
#[test]
fn one_slow_client_does_not_block_a_normal_command() { /* partial raw client + normal client */ }

#[test]
fn exhausted_decode_slots_report_primary_busy() { /* fill every decode permit */ }

#[test]
fn dropping_server_allows_immediate_same_name_recreation() {
    let name = pipe_name("drop-recreate");
    drop(BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap());
    BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
}

#[test]
fn client_rejects_mismatched_response_id() { /* raw server writes a valid but wrong-id ack */ }
```

For the slow-client test, start the partial client first, wait until it has written the length plus one byte, then complete a normal Close round trip through a second connection in less than the 500 ms startup deadline. Do not use a sleep as the readiness assertion; coordinate with a channel/event from the raw client.

**Step 2: Verify RED against the current listener**

Run each new test by exact name. Expected failures:

- normal client returns `primary-not-ready` while the slow client owns the listener;
- drop/recreate fails intermittently or requires waiting;
- mismatched response is accepted;
- no `primary-busy` distinction exists.

**Step 3: Replace listener/pending-response state with the deep endpoint interface**

The state-side interface is:

```rust
pub struct PendingCommand {
    command: ValidatedCommand,
    reply: tokio::sync::oneshot::Sender<CommandAck>,
}

impl PendingCommand {
    pub fn command(&self) -> &ValidatedCommand;
    pub fn respond(self, ack: CommandAck) -> Result<(), BrokerCommandError>;
}

pub struct BrokerCommandServer {
    receiver: Receiver<PendingCommand>,
    shutdown: Option<oneshot::Sender<()>>,
    endpoint_thread: Option<JoinHandle<()>>,
}
```

`BrokerCommandServer::receive()` returns one `PendingCommand`; remove `pending_response` and `send_response`. The endpoint thread owns a Tokio runtime, listener task, connection `JoinSet`, and the only pipe handles.

The accept loop must create the replacement server before decoding the connected instance. Bound these dimensions separately and derive `MAX_PIPE_INSTANCES` from them:

```rust
const MAX_QUEUED_COMMANDS: usize = 8;
const MAX_ACTIVE_COMMANDS: usize = 1;
const MAX_DECODING_CONNECTIONS: usize = 4;
const LISTENER_RESERVE: usize = 1;
const MAX_PIPE_INSTANCES: usize = MAX_QUEUED_COMMANDS
    + MAX_ACTIVE_COMMANDS
    + MAX_DECODING_CONNECTIONS
    + LISTENER_RESERVE;
```

A connection task decodes with `BrokerControlContract`, uses `try_send` for the state queue, waits for reply with the response deadline, encodes one response, and closes. Protocol rejection and decoded queue-full are written on that connection. Saturated connection slots remain a client-side `primary-busy`; an endpoint never observed remains `primary-not-ready`.

`BrokerCommandClient::send` may use a short-lived current-thread Tokio runtime because a secondary sends once. It must call `BrokerControlContract::decode_response` before returning an ack.

`shutdown()` signals the endpoint, stops accept, aborts/awaits connection tasks, drops all pipe instances, and joins the endpoint thread. `Drop` calls the same idempotent path. Never detach a task or thread.

**Step 4: Run command transport tests repeatedly**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control -- --test-threads=1
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker one_slow_client_does_not_block_a_normal_command -- --exact }
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker dropping_server_allows_immediate_same_name_recreation -- --exact }
```

Expected: all PASS; no Broker process/thread remains after each test.

**Step 5: Run quality checks and commit**

```powershell
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
git add src/rust/crates/previewit-broker/src/command.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/tests/broker_control.rs
git commit -m "fix: bound broker command endpoint"
```

## Task 4: Make routing pure and apply the approved path semantics

**Files:**

- Modify: `src/rust/crates/previewit-broker/src/router.rs`
- Modify: `src/rust/crates/previewit-broker/tests/command_routing.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`

**Step 1: Rewrite router tests against domain commands**

Remove Protobuf builders and filesystem fixtures from this suite. Fix the approved behavior with a missing absolute path:

```rust
#[test]
fn absolute_missing_path_enters_resolving_without_filesystem_io() {
    let path = PathBuf::from(r"C:\definitely-missing-previewit\file.txt");
    let mut router = router_with_ids(&["request-1"], 8);

    let result = router.route(validated_open("command-1", path.clone()));

    assert!(matches!(result.ack, CommandAck::Accepted { .. }));
    assert_eq!(result.disposition, RouteDisposition::Routed);
    assert_eq!(
        result.effects,
        vec![SessionEffect::BeginResolve(PreviewRequest {
            request_id: "request-1".into(),
            path,
        })]
    );
}
```

Duplicate tests must assert `RouteDisposition::Duplicate`; do not infer duplication from an empty effect list.

**Step 2: Verify RED**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test command_routing
```

Expected: FAIL because router still accepts raw Protobuf, calls `Path::exists`, and has no explicit disposition.

**Step 3: Implement the pure router result**

Use this single return shape:

```rust
pub struct RouteResult {
    pub ack: CommandAck,
    pub effects: Vec<SessionEffect>,
    pub disposition: RouteDisposition,
}

pub enum RouteDisposition {
    Routed,
    Duplicate,
}
```

`CommandRouter::route` accepts `ValidatedCommand`. Delete `decode_path`, every protocol/version constant, response factory, and `Path::exists`. Keep request-ID generation, reducer ownership, bounded FIFO duplicate cache, and immediate fake cleanup exactly once.

**Step 4: Prove router has no filesystem/protocol boundary**

Run:

```powershell
rg -n "exists\(|metadata\(|BrokerControlRequest|BrokerControlResponse|PROTOCOL_(MAJOR|MINOR)|decode_path" src/rust/crates/previewit-broker/src/router.rs
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test command_routing
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test request_state_machine
```

Expected: `rg` has no matches and both suites pass.

**Step 5: Commit**

```powershell
git add src/rust/crates/previewit-broker/src/router.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/tests/command_routing.rs
git commit -m "refactor: keep broker routing pure"
```

## Task 5: Centralize runtime lifecycle and observable events

**Files:**

- Create: `src/rust/crates/previewit-broker/src/runtime.rs`
- Create: `src/rust/crates/previewit-broker/src/event.rs`
- Modify: `src/rust/crates/previewit-broker/src/command.rs`
- Modify: `src/rust/crates/previewit-broker/src/main.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`
- Modify: `src/rust/crates/previewit-broker/src/session.rs`
- Create: `src/rust/crates/previewit-broker/tests/broker_runtime.rs`
- Modify: `src/rust/crates/previewit-broker/tests/broker_single_instance.rs`

**Step 1: Write failing event/lifecycle tests**

Use an in-memory sink through the same interface as production:

```rust
#[test]
fn duplicate_phase_and_stale_paths_emit_canonical_events() {
    let events = RecordingEventSink::default();
    let mut runtime = test_runtime(events.clone());

    runtime.handle(validated_open("command-1", absolute_path()));
    runtime.handle(validated_open("command-1", absolute_path()));
    runtime.handle_event(SessionEvent::Rendered("stale-request".into()));

    assert_eq!(
        events.names(),
        ["command-accepted", "session-transition", "command-duplicate", "stale-ignored"]
    );
}
```

Add a shutdown-order test asserting `endpoint-stopped` occurs before `lease-released`. Do not add `lease-lost`; it is not a real live-process Mutex transition.

**Step 2: Verify RED**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_runtime
```

Expected: FAIL because runtime/event interfaces do not exist and `main` discards effects.

**Step 3: Add the single event vocabulary and runtime owner**

`event.rs` owns the enum and stable names:

```rust
pub enum BrokerEvent {
    InstanceElected { role: InstanceRoleName },
    EndpointStarted,
    EndpointStopped,
    LeaseReleased,
    CommandAccepted { command_id: String, request_id: Option<String> },
    CommandRejected { command_id: Option<String>, reason: &'static str },
    CommandDuplicate { command_id: String },
    CommandQueueFull { command_id: String },
    SessionTransition { from: SessionPhase, to: SessionPhase, request_id: Option<String> },
    StaleIgnored { expected: Option<String>, actual: String },
    SessionFailed { request_id: String, reason: &'static str },
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &BrokerEvent);
}
```

Only `BrokerEvent::name()` maps to stable strings. Project `SessionState` to a small `SessionPhase` enum through one `SessionState::phase()` method instead of duplicating transition rules. Production formatting omits full paths. Tests use `RecordingEventSink`; do not maintain a second list of event names.

`BrokerRuntime` consumes the lease and server, owns router/sink, and exposes the run loop. It observes state before/after route/reducer calls to emit transitions, handles `PendingCommand::respond`, and calls explicit endpoint shutdown before dropping/releasing the lease.

Reduce `main.rs` to argument parsing, election, constructing the runtime/client, printing the one CLI result, and mapping typed errors to exit status. It must not interpret raw response fields or discard effects.

**Step 4: Run runtime and real-process suites**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_runtime
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_single_instance -- --test-threads=1
```

Expected: all PASS; ten-process election and crash takeover remain unchanged, and event assertions use the typed sink.

**Step 5: Commit**

```powershell
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
git add src/rust/crates/previewit-broker/src/runtime.rs src/rust/crates/previewit-broker/src/event.rs src/rust/crates/previewit-broker/src/command.rs src/rust/crates/previewit-broker/src/main.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/src/session.rs src/rust/crates/previewit-broker/tests/broker_runtime.rs src/rust/crates/previewit-broker/tests/broker_single_instance.rs
git commit -m "feat: centralize broker runtime events"
```

## Task 6: Prove the actual Windows security and capacity boundaries

**Files:**

- Modify: `src/rust/crates/previewit-broker/src/windows_security.rs`
- Modify: `src/rust/crates/previewit-broker/src/command.rs`
- Modify: `src/rust/crates/previewit-broker/tests/broker_control.rs`
- Create: `src/rust/crates/previewit-broker/tests/command_pipe_security.rs`

**Step 1: Write failing security inspection tests**

Create a real command pipe and inspect it through Win32 rather than asserting builder inputs:

```rust
#[test]
fn command_pipe_dacl_contains_only_system_and_current_user() {
    let inspection = inspect_command_pipe_security(unique_name());
    assert_eq!(inspection.allowed_sids, [system_sid(), current_user_sid()]);
    assert!(!inspection.handle_inheritable);
}

#[test]
fn command_pipe_rejects_remote_clients_and_allows_current_user() {
    let inspection = inspect_command_pipe(unique_name());
    assert!(inspection.rejects_remote_clients);
    assert!(inspection.current_user_round_trip_succeeded);
}
```

Use `GetSecurityInfo`/ACL enumeration for the DACL and `GetNamedPipeInfo` for the pipe flags. Sort SID strings before comparison. Do not depend on SMB, machine-name resolution, firewall state, or a second OS account.

**Step 2: Verify RED**

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test command_pipe_security
```

Expected: FAIL because no inspection test support exists.

**Step 3: Add the smallest read-only inspection support**

Keep construction in `CurrentUserSecurity`; expose test-only helpers only where Win32 introspection cannot be performed through the public endpoint. Do not add a second security descriptor builder. Verify Tokio receives the same `SECURITY_ATTRIBUTES` pointer used by Worker and command servers.

Also add capacity assertions that decoded queue saturation returns `queue-full`, connection saturation returns `primary-busy`, and both recover after clients disconnect.

**Step 4: Run security and transport stress**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test command_pipe_security -- --test-threads=1
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker full_pending_queue_returns_stable_rejection -- --exact }
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker exhausted_decode_slots_report_primary_busy -- --exact }
```

Expected: all PASS with no leaked child process or endpoint.

**Step 5: Commit**

```powershell
git add src/rust/crates/previewit-broker/src/windows_security.rs src/rust/crates/previewit-broker/src/command.rs src/rust/crates/previewit-broker/tests/broker_control.rs src/rust/crates/previewit-broker/tests/command_pipe_security.rs
git commit -m "test: prove broker command security boundaries"
```

## Task 7: Run the full x64 gate and replace stale Issue evidence

**Files:**

- Modify: `tools/test-foundation.ps1` only if a new integration test is not already reached by the workspace test
- Modify: `.cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md`

**Step 1: Run focused repetition before the full gate**

```powershell
1..100 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker delayed_partial_frame_is_stably_truncated -- --exact }
1..100 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker broker_control_round_trips_open_path -- --exact }
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker one_slow_client_does_not_block_a_normal_command -- --exact }
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker dropping_server_allows_immediate_same_name_recreation -- --exact }
```

Expected: every iteration passes. Stop on the first failure; do not hide flakes with a retry wrapper.

**Step 2: Run the complete local gate**

```powershell
pwsh -NoProfile -File tools/test-foundation.ps1
```

Expected: exit 0 with `QUICKLOOK_BASELINE_OK`, `LEGACY_BUILD_OK`, `FOUNDATION_STEP=broker-single-instance`, and `FOUNDATION_GATE_OK`.

**Step 3: Reconfirm x64-only and inspect the final diff**

```powershell
rustup target list --installed
rg -n "aarch64|ARM64" rust-toolchain.toml src/rust .github/workflows/foundation.yml tools/test-foundation.ps1
dumpbin /headers src/rust/target/debug/previewit-broker.exe | Select-String "8664 machine"
git diff 7e62a7b..HEAD --check
git status --short
```

Expected: only `x86_64-pc-windows-msvc` is installed, the ARM search has no matches, PE output contains `8664 machine (x64)`, and diff check is clean.

**Step 4: Replace superseded evidence in the open Issue**

Record exact test counts and commands for:

- contract and response validation;
- delayed broken-pipe normalization;
- slow-client concurrency and bounded saturation;
- immediate Drop/recreate and shutdown-before-lease ordering;
- DACL ACEs, remote flag, and current-user connection;
- runtime events and async path semantics;
- ten-process election/crash takeover;
- full gate and x64 inspection.

Keep the earlier `7e62a7b` evidence explicitly historical. Do not claim a remote network connection test if only the deterministic pipe flag was inspected. Do not close the Issue, behavior Explore, or Epic.

**Step 5: Commit the verified evidence**

```powershell
git add tools/test-foundation.ps1 .cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md
git commit -m "test: gate broker control convergence"
```

**Step 6: Stop for closing authorization**

Report the commit list, exact gate output, remaining scope, and the unchanged CodeStable states. Request explicit user authorization before closing or graduating any entity.
