# Rust Broker Acceptance Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use `@executing-plans` to implement this plan task-by-task. Use `@cs` Do/debug rules and `@test-driven-development` for every production behavior.

**Goal:** Remove the four acceptance blockers from the x64 Broker control foundation while keeping one bounded endpoint, one Runtime-owned effect path, and no release-only testing API.

**Architecture:** Keep Tokio as the sole pipe I/O owner and add the missing admission instance beside the standby listener. Make `CommandRouter` perform one reducer step and let `BrokerRuntime` exhaustively drive effects and observe every state identity transition. Move real Win32 security inspection behind the crate's unit-test boundary and coordinate saturation only through bounded clients and typed events.

**Tech Stack:** Rust 2024, Tokio Windows Named Pipes, `windows-sys` 0.61, Prost, Cargo unit/integration tests, PowerShell foundation gate.

**Source of truth:** `.cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md`, including the approved acceptance remediation. The supporting design is `docs/plans/2026-07-23-rust-broker-acceptance-remediation-design.md`.

**Hard constraints:** Windows x64 only. Do not add ARM64 targets, branches, matrices, artifacts, test modes, or promises. Do not add Shell Resolver, Viewer, Renderer, x86 Dialog Adapter, commands, a second effect runner, or a second event/error vocabulary. Keep the Issue and behavior Explore open and the architecture Epic draft unless the user separately authorizes closing.

---

## Task 1: Keep the endpoint alive at combined capacity

**Files:**

- Modify: `src/rust/crates/previewit-broker/tests/broker_control.rs`
- Modify: `src/rust/crates/previewit-broker/src/command.rs`

### Step 1: Replace the unbounded queue-test coordination

Remove `FlushFileBuffers`, `AsRawHandle`, and `wait_until_server_reads_request`. Add one bounded client helper and keep the canonical queue-full event as the readiness proof:

```rust
type ClientResult = Result<CommandAck, BrokerCommandError>;

fn spawn_client(name: &str, request: BrokerControlRequest) -> JoinHandle<ClientResult> {
    let name = name.to_owned();
    std::thread::spawn(move || BrokerCommandClient::send(&name, &request, TIMEOUT, TIMEOUT))
}

fn fill_pending_queue(name: &str, label: &str, count: usize) -> Vec<JoinHandle<ClientResult>> {
    (0..count)
        .map(|index| {
            let client = spawn_client(
                name,
                close_request(&format!("command-{label}-{index}")),
            );
            // Pacing prevents this queue-specific setup from becoming a
            // decoder-saturation scenario. CommandQueueFull is the assertion.
            std::thread::sleep(Duration::from_millis(25));
            client
        })
        .collect()
}
```

Rewrite `full_pending_queue_returns_stable_rejection` to start nine bounded clients, wait for `CommandQueueFull`, drain/respond to exactly eight pending commands, join all clients, and assert eight accepted plus one `queue-full`. Do not use a raw client or an unbounded Win32 call.

### Step 2: Write the failing combined-capacity test

Add a real test that reaches every configured dimension together:

```rust
#[test]
fn combined_capacity_rejects_without_stopping_endpoint() {
    const QUEUE_CAPACITY: usize = 8;
    const DECODER_SLOTS: usize = 4;

    let name = pipe_name("combined-capacity");
    let (queue_full_tx, queue_full_rx) = mpsc::channel();
    let events: Arc<dyn EventSink> = Arc::new(QueueFullSignal(Mutex::new(queue_full_tx)));
    let server =
        BrokerCommandServer::create_with_event_sink(&name, TIMEOUT, TIMEOUT, events).unwrap();

    let active_client = spawn_client(&name, close_request("command-active"));
    let active = server.receive().unwrap();

    let queued_clients = fill_pending_queue(&name, "queued", QUEUE_CAPACITY + 1);
    queue_full_rx.recv_timeout(TIMEOUT).unwrap();

    let (decoder_ready_tx, decoder_ready_rx) = mpsc::channel();
    let mut decoder_releases = Vec::new();
    let mut decoder_clients = Vec::new();
    for _ in 0..DECODER_SLOTS {
        let (release_tx, release_rx) = mpsc::channel();
        decoder_releases.push(release_tx);
        let client_name = name.clone();
        let ready = decoder_ready_tx.clone();
        decoder_clients.push(std::thread::spawn(move || {
            hold_partial_frame(client_name, ready, release_rx, TIMEOUT)
        }));
    }
    drop(decoder_ready_tx);
    for _ in 0..DECODER_SLOTS {
        decoder_ready_rx.recv_timeout(TIMEOUT).unwrap();
    }

    let saturated = BrokerCommandClient::send(
        &name,
        &close_request("command-saturated"),
        TIMEOUT,
        TIMEOUT,
    );

    for release in decoder_releases {
        let _ = release.send(());
    }
    for client in decoder_clients {
        assert!(client.join().unwrap());
    }

    let active_id = active.command().command_id().clone();
    let _ = active.respond(CommandAck::CloseAccepted {
        command_id: active_id,
    });
    for _ in 0..QUEUE_CAPACITY {
        let pending = server.receive().unwrap();
        let command_id = pending.command().command_id().clone();
        pending
            .respond(CommandAck::CloseAccepted { command_id })
            .unwrap();
    }

    let _ = active_client.join().unwrap();
    let queued: Vec<_> = queued_clients
        .into_iter()
        .map(|client| client.join().unwrap())
        .collect();
    assert_eq!(
        queued
            .iter()
            .filter(|result| matches!(result, Ok(CommandAck::CloseAccepted { .. })))
            .count(),
        QUEUE_CAPACITY
    );
    assert_eq!(
        queued
            .iter()
            .filter(|result| matches!(
                result,
                Ok(CommandAck::Rejected {
                    reason: CommandRejectionCode::QueueFull,
                    ..
                })
            ))
            .count(),
        1
    );

    assert!(matches!(saturated, Err(BrokerCommandError::PrimaryBusy)));

    let recovered = spawn_client(&name, close_request("command-recovered"));
    let pending = server.receive().unwrap();
    let command_id = pending.command().command_id().clone();
    pending
        .respond(CommandAck::CloseAccepted { command_id })
        .unwrap();
    assert!(matches!(
        recovered.join().unwrap().unwrap(),
        CommandAck::CloseAccepted { .. }
    ));
}
```

Keep cleanup best-effort until the primary assertion so the RED run cannot leave long-lived test clients.

### Step 3: Run the test and verify RED

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control combined_capacity_rejects_without_stopping_endpoint -- --exact --test-threads=1
```

Expected: FAIL because replacement creation attempts a fifteenth server instance while the configured maximum is fourteen; the saturated client does not receive `PrimaryBusy`, and/or the endpoint reports `ListenerStopped` during cleanup.

### Step 4: Implement the minimal capacity correction

In `command.rs`, model both required reserves:

```rust
const MAX_QUEUED_COMMANDS: usize = 8;
const MAX_ACTIVE_COMMANDS: usize = 1;
const MAX_DECODING_CONNECTIONS: usize = 4;
const ADMISSION_RESERVE: usize = 1;
const LISTENER_RESERVE: usize = 1;
const MAX_PIPE_INSTANCES: usize = MAX_QUEUED_COMMANDS
    + MAX_ACTIVE_COMMANDS
    + MAX_DECODING_CONNECTIONS
    + ADMISSION_RESERVE
    + LISTENER_RESERVE;
```

Extract one `reject_primary_busy` helper so decoder saturation and recoverable listener replacement use the same typed event and ack. If replacement creation returns a `BrokerCommandError` whose nested Windows source is `ERROR_PIPE_BUSY`, write the canonical id-less `primary-busy` response on the connected listener, drop that connection, recreate the standby listener, and continue. Do not turn access/configuration errors into busy; non-capacity creation failures remain terminal.

### Step 5: Verify GREEN and regressions

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control combined_capacity_rejects_without_stopping_endpoint -- --exact --test-threads=1
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control -- --test-threads=1
```

Expected: the new test passes; the queue, decoder, slow-client, malformed-frame, deadline, and Drop/recreate tests all pass. `rg -n "FlushFileBuffers|wait_until_server_reads_request" src/rust/crates/previewit-broker/tests/broker_control.rs` has no matches.

### Step 6: Commit

```powershell
git add src/rust/crates/previewit-broker/src/command.rs src/rust/crates/previewit-broker/tests/broker_control.rs
git commit -m "fix: preserve broker endpoint at full capacity"
```

---

## Task 2: Remove inspection from the release API

**Files:**

- Modify: `src/rust/crates/previewit-broker/src/command.rs`
- Modify: `src/rust/crates/previewit-broker/src/windows_security.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`
- Delete: `src/rust/crates/previewit-broker/tests/command_pipe_security.rs`

### Step 1: Add test-owned real-handle inspection

Move the security descriptor/ACL/pipe flag inspection helpers into the existing `#[cfg(test)] mod tests` in `command.rs`. The unit test must create the real Tokio `NamedPipeServer` through the private `create_pipe_instance` function while a current-thread runtime is entered, then inspect its owned raw handle with `GetSecurityInfo`, `GetNamedPipeInfo`, and `GetHandleInformation`.

Keep two tests:

```rust
#[test]
fn command_pipe_dacl_contains_only_system_and_current_user() {
    let (server, _runtime, _security) = inspected_server("dacl");
    let inspection = inspect_command_pipe(server.as_raw_handle().cast());
    let mut expected = vec![SYSTEM_SID.to_owned(), current_user_sid().unwrap()];
    expected.sort();
    assert_eq!(inspection.allowed_sids, expected);
    assert!(!inspection.handle_inheritable);
}

#[test]
fn command_pipe_rejects_remote_clients_and_allows_current_user() {
    let name = test_name("local-only");
    let (server, _runtime, _security) = inspected_named_server(&name);
    let inspection = inspect_command_pipe(server.as_raw_handle().cast());
    assert!(inspection.rejects_remote_clients);
    drop(server);

    let server = BrokerCommandServer::create(&name, TIMEOUT, TIMEOUT).unwrap();
    // Complete the existing real current-user Close round trip.
}
```

The helper must keep the Tokio runtime and `CurrentUserSecurity` alive for at least as long as the inspected server. Change `current_user_sid` only to `pub(crate)` so the crate unit test can compute the independent expected SID; do not export it from the crate root.

### Step 2: Run the characterization tests

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker command_pipe_ -- --test-threads=1
```

Expected: PASS before removing the integration-test API. These tests prove the behavior that must survive the API cleanup.

### Step 3: Verify the release API guard is RED

Run:

```powershell
$matches = rg -n "inspection_handle|current_user_sid_for_inspection" src/rust/crates/previewit-broker/src
if ($LASTEXITCODE -eq 0) { $matches; throw "release inspection API still exists" }
```

Expected: FAIL and print the current `BrokerCommandServer` field/method plus crate-root SID export.

### Step 4: Remove the production inspection seam

- Change the endpoint ready channel from `Result<usize, BrokerCommandError>` to `Result<(), BrokerCommandError>`.
- Remove `BrokerCommandServer::inspection_handle`, its field, and all raw-handle capture during endpoint startup.
- Remove `current_user_sid_for_inspection` and its `pub use` from `lib.rs`; keep only the `pub(crate)` implementation required internally and by crate tests.
- Delete `tests/command_pipe_security.rs`; its assertions now live in the test-only module that directly owns the server handle.

No replacement public method, feature, snapshot type, raw integer, or duplicate security descriptor builder is allowed.

### Step 5: Verify GREEN

Run:

```powershell
$matches = rg -n "inspection_handle|current_user_sid_for_inspection" src/rust/crates/previewit-broker/src
if ($LASTEXITCODE -eq 0) { $matches; throw "release inspection API still exists" }
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker command_pipe_ -- --test-threads=1
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control -- --test-threads=1
```

Expected: the source guard and all real pipe/security/transport tests pass.

### Step 6: Commit

```powershell
git add src/rust/crates/previewit-broker/src/command.rs src/rust/crates/previewit-broker/src/windows_security.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/tests/command_pipe_security.rs
git commit -m "test: hide broker pipe inspection from release api"
```

---

## Task 3: Make Runtime the only session effect owner

**Files:**

- Modify: `src/rust/crates/previewit-broker/src/router.rs`
- Modify: `src/rust/crates/previewit-broker/src/runtime.rs`
- Modify: `src/rust/crates/previewit-broker/tests/command_routing.rs`
- Modify: `src/rust/crates/previewit-broker/tests/broker_runtime.rs`

### Step 1: Write the pure-router expectations

Replace the immediate-cleanup routing assertion with one-step reducer behavior:

```rust
#[test]
fn rapid_replacement_stays_closing_and_keeps_only_the_latest_pending_request() {
    let path = absolute_path("replacement");
    let mut router = router_with_ids(&["request-1", "request-2", "request-3"], 8);

    router.route(open("command-1", path.clone()));
    let second = router.route(open("command-2", path.clone()));
    let latest = router.route(open("command-3", path.clone()));

    assert_eq!(second.effects, vec![SessionEffect::Cancel("request-1".into())]);
    assert_eq!(
        latest.effects,
        vec![SessionEffect::Superseded("request-2".into())]
    );
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
```

Update the Close test to expect `Closing` from the pure router. The reducer suite already proves `CleanupComplete` promotion.

### Step 2: Write the failing Runtime replacement test

Give `test_runtime` enough deterministic request IDs and add:

```rust
#[test]
fn replacement_emits_each_transition_while_runtime_drives_cleanup() {
    let events = RecordingEventSink::default();
    let (mut runtime, _) = test_runtime(events.clone());
    let path = PathBuf::from(r"C:\definitely-missing-previewit\runtime.txt");

    runtime.handle(validated_open("command-1", path.clone()));
    events.clear();
    runtime.handle(validated_open("command-2", path));

    let recorded = events.events.lock().unwrap().clone();
    assert!(matches!(recorded.as_slice(), [
        BrokerEvent::CommandAccepted { request_id: Some(next), .. },
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
    ] if next == "request-2" && old == "request-1" && promoted == "request-2"));
}
```

Update `current_failure_emits_failure_before_the_resulting_transition` to require `session-failed`, `Resolving -> Closing`, then `Closing -> Idle`.

### Step 3: Run tests and verify RED

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test command_routing -- --test-threads=1
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_runtime -- --test-threads=1
```

Expected: FAIL because Router currently fabricates cleanup completion and Runtime collapses replacement to the same phase.

### Step 4: Make Router perform one reducer step

Replace the `VecDeque` loop in `CommandRouter::handle_event` with:

```rust
pub(crate) fn handle_event(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
    self.reducer.handle(event)
}
```

Remove the unused `VecDeque` import only if the duplicate cache no longer needs it; the cache still does, so retain the import and remove only the effect queue logic.

### Step 5: Add one exhaustive Runtime effect pump

In `BrokerRuntime`, emit the initial command/session transition immediately after its reducer step, then drive effects through one queue:

```rust
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
            | SessionEffect::Superseded(_) => {
                // This Issue has no execution resource. The future effect
                // adapter is added here, not in Router or main.
            }
        }
    }
}
```

Do not retain the old `emit_effects` partial matcher. Derive `PartialEq, Eq` for `StateSnapshot` and emit when the full snapshot differs:

```rust
if before != after {
    // existing typed SessionTransition
}
```

Preserve event ordering: command accepted/rejected or session failed first, transition caused by the input second, transitions caused by feedback afterward.

### Step 6: Verify GREEN and state-machine regressions

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test command_routing -- --test-threads=1
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_runtime -- --test-threads=1
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test request_state_machine -- --test-threads=1
```

Expected: all pass. Router remains filesystem-free and one-step; Runtime emits both replacement transitions and preserves stale/failure semantics.

### Step 7: Commit

```powershell
git add src/rust/crates/previewit-broker/src/router.rs src/rust/crates/previewit-broker/src/runtime.rs src/rust/crates/previewit-broker/tests/command_routing.rs src/rust/crates/previewit-broker/tests/broker_runtime.rs
git commit -m "refactor: make broker runtime own session effects"
```

---

## Task 4: Re-run the x64 acceptance gate and replace stale evidence

**Files:**

- Modify: `.cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md`
- Modify: `tools/test-foundation.ps1` only if the workspace test no longer reaches the moved security tests

### Step 1: Format, lint, and run focused suites

Run:

```powershell
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker -- --test-threads=1
```

Expected: all pass with no warnings.

### Step 2: Run the hostile scenarios repeatedly

Run each loop without retrying a failed iteration:

```powershell
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control combined_capacity_rejects_without_stopping_endpoint -- --exact --test-threads=1 }
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control full_pending_queue_returns_stable_rejection -- --exact --test-threads=1 }
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control exhausted_decode_slots_report_primary_busy -- --exact --test-threads=1 }
1..20 | ForEach-Object { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_runtime replacement_emits_each_transition_while_runtime_drives_cleanup -- --exact --test-threads=1 }
```

Expected: 20/20 for every scenario.

### Step 3: Run the complete foundation gate

```powershell
pwsh -NoProfile -File tools/test-foundation.ps1
```

Expected: exit 0 with `QUICKLOOK_BASELINE_OK`, `LEGACY_BUILD_OK`, `FOUNDATION_STEP=broker-single-instance`, and `FOUNDATION_GATE_OK`.

### Step 4: Reconfirm x64-only and release API boundaries

```powershell
rustup target list --installed
rg -n "aarch64|ARM64" rust-toolchain.toml src/rust .github/workflows/foundation.yml tools/test-foundation.ps1
dumpbin /headers src/rust/target/debug/previewit-broker.exe | Select-String "8664 machine"
rg -n "inspection_handle|current_user_sid_for_inspection|FlushFileBuffers" src/rust/crates/previewit-broker/src src/rust/crates/previewit-broker/tests
git diff 3ef0edc..HEAD --check
git status --short
```

Expected: only `x86_64-pc-windows-msvc` is installed; the ARM and removed-boundary searches have no matches; PE contains `8664 machine (x64)`; diff check passes; only the Issue evidence is dirty before the evidence commit.

### Step 5: Replace remediation evidence in the open Issue

Record exact commands/counts for combined saturation and recovery, bounded queue coordination, test-only security inspection, pure Router/Runtime effects, focused repetitions, the complete gate, and x64 inspection. Keep `7e62a7b`, `1738f0e`, and their limitations explicitly historical. Do not close or rename the Issue, behavior Explore, or Epic.

### Step 6: Commit the verified evidence

```powershell
git add .cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md tools/test-foundation.ps1
git commit -m "test: gate broker acceptance remediation"
```

### Step 7: Stop for harsh review and explicit closing authorization

Report every task commit, exact gate results, remaining scope, and unchanged CodeStable states. Run a new harsh acceptance review before requesting any authorization to close or graduate knowledge.
