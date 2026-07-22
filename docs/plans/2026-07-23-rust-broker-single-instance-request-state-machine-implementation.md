# Rust Broker Single-Instance and Request State Machine Implementation Plan

> **For Claude/Codex:** REQUIRED SUB-SKILL: Use `@executing-plans` to implement this plan task-by-task. Use `@cs` Do rules and `@test-driven-development` for every production behavior.

**Goal:** Run exactly one x64 Rust Broker per interactive Windows session, forward typed commands from later invocations over a bounded current-user pipe, and serialize preview requests through a `request_id` state machine that cancels old work and rejects stale events.

**Architecture:** A session-scoped Win32 Mutex elects the Broker before any control-plane initialization. The primary owns a deterministic current-user/SYSTEM Named Pipe; secondary processes send a separate Protobuf broker-control request and wait for a bounded acknowledgment. A pure reducer owns `Idle -> Resolving -> Preparing -> Rendering -> Ready -> Closing`; `Closing { old, next }` serializes cleanup and coalesces rapid requests to the latest pending request.

**Tech Stack:** Rust 1.97 x64 MSVC, Rust standard library, `windows-sys` 0.61, Protobuf/`prost` 0.14, UUID v4, Windows Named Pipes and Mutexes, Cargo integration tests, PowerShell foundation gate.

---

## Scope and execution rules

- Work only on `.cs/issues/2026/07/23/open-rust-broker-single-instance-request-state-machine.md`.
- The worktree is `D:\Code\PreviewIt-foundation`; implementation branch is `feat/broker-instance-state-machine`.
- Keep the Issue `open`, behavior-baseline Explore `open`, and architecture Epic `draft` until the user explicitly authorizes closure.
- Keep `rust-toolchain.toml` limited to `x86_64-pc-windows-msvc`. Do not add ARM64 targets, conditionals, build jobs, artifacts, claims, or fallback behavior.
- Do not take over hotkeys, Shell selection, x86 Dialog Adapter, Viewer/WPF, plugins, Renderer routing, installer, updater, or release behavior.
- For each behavior, write the smallest test first, run it, and retain the expected failing output in the execution notes before writing production code.
- Commit each numbered task separately after its focused checks pass. Do not amend, rebase, push, or close CodeStable entities.

## Task 1: Define the broker control protocol without overloading Worker Envelope

**Files:**

- Modify: `protocol/preview/v0/preview.proto`
- Modify: `src/rust/crates/previewit-protocol/tests/framing.rs`
- Modify: `tests/dotnet/PreviewIt.Protocol.Tests/FramedProtocolTests.cs`
- Generated at build: Rust and C# Protobuf sources

**Contract:**

Add messages separate from `Envelope`:

```proto
message BrokerControlRequest {
  uint32 protocol_major = 1;
  uint32 protocol_minor = 2;
  string command_id = 3;
  oneof command {
    OpenPath open_path = 10;
    ClosePreview close_preview = 11;
  }
}

message OpenPath {
  bytes path_utf16le = 1;
}

message ClosePreview {}

message BrokerControlResponse {
  uint32 protocol_major = 1;
  uint32 protocol_minor = 2;
  string command_id = 3;
  bool accepted = 4;
  string request_id = 5;
  string error_code = 6;
}
```

The secondary chooses `command_id`; only the primary chooses `request_id`. An accepted `ClosePreview` may return an empty `request_id`. The protocol remains `0.1`; adding new message types does not change the existing Worker envelope.

**Step 1: Write failing Rust and .NET parity tests**

Add a Rust test that constructs `BrokerControlRequest { 0, 1, "command-1", OpenPath { path_utf16le } }`, serializes/parses it, and asserts every field. Encode `C:\\fixtures\\preview.txt` with `encode_utf16().flat_map(u16::to_le_bytes)`.

Add a .NET test that constructs the equivalent generated message with `ByteString.CopyFrom(Encoding.Unicode.GetBytes(path))`, serializes/parses it, and asserts every field. Add a response round trip asserting accepted, request ID, and empty error code.

**Step 2: Run tests and verify the schema symbols are missing**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-protocol protobuf_broker_control_round_trips
dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release --filter BrokerControl
```

Expected: both builds FAIL because `BrokerControlRequest`, `OpenPath`, or `BrokerControlResponse` does not exist.

**Step 3: Add the minimal schema**

Add exactly the messages above. Do not add Toggle, Viewer, Shell, Renderer, cache, manifest, queue, or diagnostic messages.

**Step 4: Run protocol parity checks**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-protocol
dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release
```

Expected: Rust protocol tests and all .NET protocol tests PASS.

**Step 5: Commit**

```powershell
git add protocol/preview/v0/preview.proto src/rust/crates/previewit-protocol/tests/framing.rs tests/dotnet/PreviewIt.Protocol.Tests/FramedProtocolTests.cs
git commit -m "feat: define broker control protocol"
```

## Task 2: Build the pure request state reducer

**Files:**

- Create: `src/rust/crates/previewit-broker/src/session.rs`
- Create: `src/rust/crates/previewit-broker/tests/request_state_machine.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`

**Public model:**

```rust
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
    StaleIgnored { expected: Option<String>, actual: String },
}
```

`SessionReducer::handle(event)` returns effects and is the only operation that mutates state.

**Step 1: Write the first failing state test**

Test `Idle + Open(request-1)` produces `Resolving(request-1)` and one `BeginResolve(request-1)` effect. Test the success path through `Ready` using matching IDs.

**Step 2: Verify RED**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test request_state_machine idle_request_reaches_ready
```

Expected: FAIL because the session module/API does not exist.

**Step 3: Implement only the happy-path reducer and verify GREEN**

Export the types from `lib.rs`. Implement `Idle -> Resolving -> Preparing -> Rendering -> Ready`; each completion must compare its ID with the active request.

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test request_state_machine idle_request_reaches_ready
```

Expected: PASS.

**Step 4: Add failing replacement, close, and stale tests**

Cover these behaviors separately:

- `Rendering(request-1) + Open(request-2)` becomes `Closing { old: request-1, next: request-2 }` and emits `Cancel(request-1)` once.
- `Closing { old: request-1, next: request-2 } + Open(request-3)` retains only request-3 and emits `Superseded(request-2)` without another cancel.
- `CleanupComplete(request-1)` promotes request-3 to `Resolving` and emits `BeginResolve(request-3)`.
- `Close` clears a pending `next`; cleanup returns to `Idle`.
- late resolved/rendered/cleanup events cannot advance, close, or overwrite a newer request and emit `StaleIgnored`.

**Step 5: Verify RED, implement the minimal transitions, and verify GREEN**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test request_state_machine
```

Expected before implementation: replacement/stale tests FAIL for missing behavior. Expected after implementation: all request state tests PASS.

**Step 6: Run focused quality checks and commit**

```powershell
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
git add src/rust/crates/previewit-broker/src/session.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/tests/request_state_machine.rs
git commit -m "feat: add broker request state machine"
```

## Task 3: Elect one Broker per interactive session

**Files:**

- Create: `src/rust/crates/previewit-broker/src/instance.rs`
- Create: `src/rust/crates/previewit-broker/src/windows_security.rs`
- Create: `src/rust/crates/previewit-broker/tests/instance_lease.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`
- Modify: `src/rust/crates/previewit-broker/src/pipe.rs`
- Modify: `src/rust/crates/previewit-broker/Cargo.toml` only if an additional `windows-sys` feature is required

**API and ownership:**

```rust
pub enum InstanceRole {
    Primary(InstanceLease),
    Secondary(InstanceContender),
}

impl InstanceLease {
    pub fn elect(product_id: &str) -> Result<InstanceRole, InstanceError>;
    pub fn pipe_name(&self) -> &str;
}

impl InstanceContender {
    pub fn try_take_over(&mut self) -> Result<Option<InstanceLease>, InstanceError>;
    pub fn pipe_name(&self) -> &str;
}
```

Derive both names from a fixed product ID plus `ProcessIdToSessionId(GetCurrentProcessId())`:

- Mutex: `Local\\PreviewIt.Broker.<session-id>`
- Pipe: `PreviewIt.Broker.<session-id>`

Tests pass a unique product ID suffix so parallel Cargo tests do not collide. `windows_security.rs` owns reusable RAII `OwnedHandle` and a current-user + SYSTEM `SECURITY_ATTRIBUTES`; refactor the existing Worker pipe to use the helper without changing its public behavior.

Create the Mutex unowned, then call `WaitForSingleObject(handle, 0)`. `WAIT_OBJECT_0` and `WAIT_ABANDONED` produce a primary lease; `WAIT_TIMEOUT` produces a contender retaining its handle. The owning thread releases the Mutex exactly once before closing the handle.

**Step 1: Write failing lease tests**

Use two threads because Win32 Mutex ownership is thread-scoped:

- owner thread elects primary and holds it behind a barrier;
- contender thread elects secondary for the same unique ID;
- after owner drops, contender `try_take_over()` returns a primary lease;
- different product IDs can each elect a primary;
- generated names contain `Local\\` only for the Mutex and contain the current session ID.

**Step 2: Verify RED**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test instance_lease
```

Expected: FAIL because `InstanceLease`/`InstanceRole` does not exist.

**Step 3: Implement shared Windows security/handle ownership and lease election**

Move, do not duplicate, the existing SID lookup, SDDL conversion, `SECURITY_ATTRIBUTES`, and owned handle logic from `pipe.rs`. Keep the DACL exactly current token user + SYSTEM and handles non-inheritable.

Implement session lookup, input validation for `product_id`, Mutex creation, zero-time ownership test, takeover, and RAII release.

**Step 4: Run lease and existing pipe suites**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test instance_lease
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test dotnet_handshake
```

Expected: all tests PASS; the Worker pipe remains random and child-PID authenticated.

**Step 5: Run quality checks and commit**

```powershell
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
git add src/rust/crates/previewit-broker/src/instance.rs src/rust/crates/previewit-broker/src/windows_security.rs src/rust/crates/previewit-broker/tests/instance_lease.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/src/pipe.rs src/rust/crates/previewit-broker/Cargo.toml src/rust/Cargo.lock
git commit -m "feat: elect one broker per user session"
```

## Task 4: Add the deterministic broker command transport

**Files:**

- Create: `src/rust/crates/previewit-broker/src/command.rs`
- Create: `src/rust/crates/previewit-broker/tests/broker_control.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`
- Modify: `src/rust/crates/previewit-broker/src/pipe.rs`

**Transport contract:**

- Reuse the existing overlapped frame read/write implementation through one internal handle-based helper; do not duplicate timeout/framing logic.
- `BrokerCommandServer` creates deterministic pipe instances with the current-user + SYSTEM DACL, `PIPE_REJECT_REMOTE_CLIENTS`, byte mode, overlapped I/O, 1 MiB maximum, and a fixed maximum pending command count.
- Each accepted connection carries exactly one `BrokerControlRequest` and one `BrokerControlResponse`.
- `BrokerCommandClient` retries `ERROR_FILE_NOT_FOUND`/`ERROR_PIPE_BUSY` only until its startup deadline, then returns `PrimaryNotReady`. It does not retry a request after receiving any response.
- Protocol major mismatch, malformed Protobuf, invalid command ID, invalid UTF-16LE path, embedded NUL, overlong path, queue full, read timeout, and write timeout map to stable error codes; no error includes a full path.

**Step 1: Write failing positive transport tests**

For a unique deterministic name, run the server on a thread and send a real `OpenPath` request from `BrokerCommandClient`. Assert the server decodes the UTF-16LE path exactly and the client receives the matching accepted response. Add the same test for `ClosePreview`.

**Step 2: Verify RED**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control broker_control_round_trips
```

Expected: FAIL because the command transport does not exist.

**Step 3: Implement the minimal server/client and verify GREEN**

Start with one connection at a time. Extract only the existing frame I/O needed by both Worker and command pipes; rerun `dotnet_handshake` after the refactor.

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control broker_control_round_trips
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test dotnet_handshake
```

Expected: both PASS.

**Step 4: Add failing negative/deadline tests**

Cover wrong major, oversized declared length, truncated request, odd UTF-16LE length, embedded NUL, overlong path, missing/oversized command ID, connect deadline, and response deadline. Add a test-only raw client helper inside `command.rs`'s unit-test module for malformed frames; do not expose raw writes in the production public API.

**Step 5: Implement validation and bounded pending commands**

Use a bounded `sync_channel`. Once a request has been decoded, transfer the connected pipe plus request to the single Broker event-loop consumer; if `try_send` reports full, write a `queue-full` response and disconnect. The listener creates the next pipe instance before waiting for a routed response, so at most the configured queue capacity plus listener connection can remain open.

**Step 6: Run command, handshake, formatting, and lint checks**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_control
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test dotnet_handshake
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
```

Expected: all PASS with no warnings.

**Step 7: Commit**

```powershell
git add src/rust/crates/previewit-broker/src/command.rs src/rust/crates/previewit-broker/tests/broker_control.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/src/pipe.rs
git commit -m "feat: add broker command channel"
```

## Task 5: Route commands through one Broker state owner and wire the executable

**Files:**

- Create: `src/rust/crates/previewit-broker/src/router.rs`
- Create: `src/rust/crates/previewit-broker/tests/command_routing.rs`
- Create: `src/rust/crates/previewit-broker/tests/broker_single_instance.rs`
- Modify: `src/rust/crates/previewit-broker/src/lib.rs`
- Modify: `src/rust/crates/previewit-broker/src/main.rs`

**Router contract:**

`CommandRouter` owns one `SessionReducer`, a fixed-capacity FIFO duplicate cache, and a request-ID source. Its only command entry point returns a `BrokerControlResponse` plus reducer effects.

- Repeating `command_id` returns the cached response without a second state transition.
- A valid `OpenPath` decodes to `OsString`, rejects non-absolute/missing paths, creates a UUID v4 `request_id`, and submits one `Open` event.
- `ClosePreview` submits `Close`; accepted close has an empty request ID.
- The cache has a fixed capacity; eviction removes the oldest command ID.
- The minimal effect runner does not claim rendering success. It treats `Cancel`/`Cleanup` as immediate when no execution resource is attached, allowing `Closing { old, next }` to promote the latest request to `Resolving`; it leaves `BeginResolve` pending for the future Shell issue.

**Executable contract:**

- Parse only `--open <path>` and `--close`; invalid/multiple commands exit nonzero without electing a Broker.
- Elect before initializing the command endpoint.
- Primary starts the deterministic endpoint, routes its own optional startup command, prints one concise role/ack line for diagnostics, and serves until terminated.
- Secondary forwards once, prints the accepted/rejected response without the path, and exits `0` only for an accepted response.
- If a contender cannot connect before the startup deadline, it rechecks the Mutex. A successful takeover starts the primary with its original command; a still-owned lease returns `primary-not-ready` and exits nonzero.

**Step 1: Write failing router tests**

Use an injected deterministic ID closure. Cover OpenPath acceptance, missing/relative path rejection, Close idempotency, duplicate response replay, bounded cache eviction, and rapid request replacement that promotes only the latest request after immediate cleanup.

**Step 2: Verify RED, implement router, verify GREEN**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test command_routing
```

Expected before implementation: FAIL because `CommandRouter` does not exist. Expected after implementation: all routing tests PASS.

**Step 3: Write failing real-process tests**

Use `env!("CARGO_BIN_EXE_previewit-broker")` and the read-only fixture:

- launch ten instances concurrently with the same session/product ID override reserved for tests;
- after the startup deadline, exactly one process remains alive as primary and nine exit successfully as secondaries;
- each secondary output contains an accepted ack and never the fixture path;
- kill the primary, start a new process, and assert it becomes primary;
- hold a lease without starting a pipe and assert a secondary exits nonzero with `primary-not-ready` within the deadline;
- inspect the built executable with the existing Visual Studio toolchain and assert `8664 machine (x64)`.

Use a test-only environment variable for the product-ID suffix and bounded deadlines. Do not add an architecture override or an ARM64 test mode.

**Step 4: Verify RED, implement `main`, verify GREEN**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_single_instance -- --test-threads=1
```

Expected before implementation: tests FAIL because the executable is still an integration-probe stub. Expected after implementation: all real-process tests PASS and no child process remains running.

**Step 5: Run all Broker tests and commit**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml -p previewit-broker --all-targets -- -D warnings
git add src/rust/crates/previewit-broker/src/router.rs src/rust/crates/previewit-broker/src/lib.rs src/rust/crates/previewit-broker/src/main.rs src/rust/crates/previewit-broker/tests/command_routing.rs src/rust/crates/previewit-broker/tests/broker_single_instance.rs
git commit -m "feat: run single-instance broker control loop"
```

## Task 6: Gate the feature and capture CodeStable evidence

**Files:**

- Modify: `tools/test-foundation.ps1`
- Modify: `.github/workflows/foundation.yml` only if the unchanged command cannot execute the new tests
- Modify: `.cs/issues/2026/07/23/open-rust-broker-single-instance-request-state-machine.md`
- Do not modify Epic/project spec during Do; closing authorization is still separate

**Step 1: Add an explicit Broker process-test gate**

Before the general Rust workspace tests, add a checked x64 Broker build and the serialized multi-process test command. Keep the existing worker build before all other Rust tests.

```powershell
Invoke-Checked 'broker-build' { cargo build --manifest-path src/rust/Cargo.toml -p previewit-broker }
Invoke-Checked 'broker-single-instance' { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_single_instance -- --test-threads=1 }
```

The general workspace test may run the same suite again; correctness is preferred over a hidden CI-only test. Do not add ARM64 matrix entries.

**Step 2: Run the complete local gate**

```powershell
pwsh -NoProfile -File tools/test-foundation.ps1
```

Expected markers:

```text
QUICKLOOK_BASELINE_OK=b13df028f3cce1f84792f7043b57bf5cea3a3e4c
LEGACY_BUILD_OK
FOUNDATION_STEP=broker-single-instance
FOUNDATION_GATE_OK
```

All Rust/.NET tests, rustfmt, Clippy, Worker build, legacy build, and x64 architecture assertions must pass.

**Step 3: Check architecture and repository scope**

```powershell
rustup target list --installed
rg -n "aarch64|ARM64" rust-toolchain.toml src/rust .github/workflows/foundation.yml tools/test-foundation.ps1
git status --short
```

Expected: the only Rust target is `x86_64-pc-windows-msvc`; no new ARM64 configuration or artifact exists; only the gate and target Issue are dirty.

**Step 4: Update the open Issue execution evidence**

Record:

- each task commit;
- the exact RED failure reason and GREEN commands;
- protocol test counts;
- state/replacement/stale test counts;
- ten-process election and crash-takeover observations;
- command timeout/queue/validation behavior;
- PE `8664` evidence and installed Rust target;
- any small design deviation and why it preserves the goal.

Do not change the Issue status or filename. Do not close the Explore or Epic. Do not write stable conclusions into Epic until explicit closing authorization.

**Step 5: Commit the verified gate/evidence**

```powershell
git add tools/test-foundation.ps1 .github/workflows/foundation.yml .cs/issues/2026/07/23/open-rust-broker-single-instance-request-state-machine.md
git commit -m "test: gate broker instance state machine"
git status --short --branch
```

Expected: clean worktree on `feat/broker-instance-state-machine`.

## Completion checkpoint

After Task 6, report the per-task commits and full gate evidence. Stop and request explicit authorization before closing the Broker Issue, changing Epic conclusions/status, closing the behavior-baseline Explore, pushing, or starting the Shell Resolver/x86 Dialog Adapter Issue.
