# Rust Hybrid Preview Foundation Implementation Plan

> **For Claude/Codex:** REQUIRED SKILL: Use `@executing-plans` to execute this plan task by task in a dedicated worktree.

**Goal:** Import the pinned QuickLook 4.5.0 baseline reproducibly and prove the smallest x64 Rust Broker ↔ .NET worker seam with framed Protobuf, authenticated Named Pipe transport, read-only handle transfer, cancellation, and crash containment.

**Architecture:** Keep the upstream QuickLook tree intact under `src/legacy/quicklook` so its WPF application and plugin contracts remain a buildable compatibility reference. Add a language-neutral protocol at the repository root, a Rust workspace for the future Broker, and a small .NET Framework worker probe that proves the legacy boundary without attempting to split the production Viewer yet. This is the first vertical slice of the approved design, not the whole Epic.

**Tech Stack:** QuickLook 4.5.0 at `b13df028f3cce1f84792f7043b57bf5cea3a3e4c`, .NET Framework 4.6.2/MSBuild, Rust 1.97 x64 MSVC, Protobuf, Windows Named Pipes, Windows process handles, Job Objects, MSTest, Cargo tests, PowerShell.

---

## Scope and exit criteria

This plan deliberately stops before taking over the global keyboard hook, Shell selection, Viewer window, plugin loading, system `IPreviewHandler`, or a real file Renderer. Those changes need separate CodeStable issues after the pinned source has been inspected in the actual PreviewIt repository.

The foundation slice is complete when:

- the exact QuickLook baseline builds from `src/legacy/quicklook/QuickLook.sln` without source edits;
- provenance and the selective-upstream-patch policy are recorded;
- Rust and .NET encode the same v0 protocol and enforce the same frame limit;
- an x64 Rust process accepts only its expected .NET child on a current-user Named Pipe;
- the child can read, but cannot write through, a handle opened by Broker with read-only access;
- timeout, child crash, and stale `request_id` tests leave the Broker probe alive;
- automated checks run locally with one command;
- the existing Explore issue is updated with evidence but remains open until the user authorizes closure.

## Task 1: Open the foundation issue and import the pinned upstream tree

**Files:**

- Create: `.cs/issues/2026/07/22/open-preview-foundation-vertical-slice.md`
- Create through `git subtree`: `src/legacy/quicklook/**`
- Create: `UPSTREAM.md`
- Create: `tools/upstream/verify-quicklook-baseline.ps1`

**Step 1: Create the CodeStable issue**

Read `@cs` design/do rules, then create `.cs/issues/2026/07/22/open-preview-foundation-vertical-slice.md` with this observable goal:

```markdown
# Preview foundation vertical slice

## Goal

The pinned QuickLook 4.5.0 source builds unchanged, and an x64 Rust Broker probe can supervise a .NET Framework worker over the v0 Preview Protocol without granting write access to the previewed file.

## Scope

- Import and verify the pinned upstream source.
- Establish the Rust/.NET framed Protobuf boundary.
- Prove current-user pipe access, child identity, read-only handle transfer, timeout, cancellation, and crash containment.
- Do not take over production hotkeys, Shell resolution, Viewer, plugins, or rendering.

## Verification

- Run `pwsh -File tools/test-foundation.ps1`.
- Review the QuickLook baseline provenance check.
- Review the cross-process failure tests and captured evidence.
```

Do not close the issue during execution; closing still requires explicit user authorization.

**Step 2: Commit the issue boundary**

```powershell
git add .cs/issues/2026/07/22/open-preview-foundation-vertical-slice.md
git commit -m "docs: define preview foundation vertical slice"
```

**Step 3: Fetch and import the exact upstream commit**

```powershell
git remote add quicklook-upstream https://github.com/QL-Win/QuickLook.git
git fetch quicklook-upstream tag 4.5.0
git rev-parse FETCH_HEAD
```

Expected: `b13df028f3cce1f84792f7043b57bf5cea3a3e4c`.

Import it without rewriting the upstream directory structure:

```powershell
git subtree add --prefix=src/legacy/quicklook quicklook-upstream b13df028f3cce1f84792f7043b57bf5cea3a3e4c --squash
```

Expected: a subtree commit containing `src/legacy/quicklook/QuickLook.sln` and `LICENSE-GPL.txt`.

**Step 4: Add a failing provenance check**

Create `tools/upstream/verify-quicklook-baseline.ps1` so it fails unless the solution, license, and pinned values exist:

```powershell
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$required = @(
    'src\legacy\quicklook\QuickLook.sln',
    'src\legacy\quicklook\LICENSE-GPL.txt',
    'UPSTREAM.md'
)

foreach ($path in $required) {
    if (-not (Test-Path -LiteralPath (Join-Path $root $path))) {
        throw "Missing required upstream artifact: $path"
    }
}

$provenance = Get-Content -Raw (Join-Path $root 'UPSTREAM.md')
$expected = 'b13df028f3cce1f84792f7043b57bf5cea3a3e4c'
if (-not $provenance.Contains($expected)) {
    throw "UPSTREAM.md does not pin $expected"
}

Write-Output "QUICKLOOK_BASELINE_OK=$expected"
```

Run it before creating `UPSTREAM.md`:

```powershell
pwsh -NoProfile -File tools/upstream/verify-quicklook-baseline.ps1
```

Expected: FAIL with `Missing required upstream artifact: UPSTREAM.md`.

**Step 5: Write the provenance record and rerun the check**

`UPSTREAM.md` must contain:

```markdown
# Upstream provenance

PreviewIt is a GPLv3 derivative of QL-Win/QuickLook.

- Repository: https://github.com/QL-Win/QuickLook
- Release: 4.5.0
- Commit: b13df028f3cce1f84792f7043b57bf5cea3a3e4c
- Imported path: `src/legacy/quicklook`
- License: `src/legacy/quicklook/LICENSE-GPL.txt`

The `master` branch is a patch source, not a moving build baseline. Every
post-4.5.0 patch must be linked to its upstream commit, applied separately,
and verified against the behavior and plugin compatibility fixtures.
```

Run:

```powershell
pwsh -NoProfile -File tools/upstream/verify-quicklook-baseline.ps1
```

Expected: `QUICKLOOK_BASELINE_OK=b13df028f3cce1f84792f7043b57bf5cea3a3e4c`.

**Step 6: Commit provenance tooling**

```powershell
git add UPSTREAM.md tools/upstream/verify-quicklook-baseline.ps1
git commit -m "docs: pin QuickLook 4.5.0 provenance"
```

## Task 2: Make the unchanged legacy baseline reproducibly buildable

**Files:**

- Create: `tools/build-legacy.ps1`
- Create: `tests/baseline/legacy-build.tests.ps1`
- Modify: `.cs/issues/2026/07/20/open-quicklook-behavior-baseline/compatibility-matrix.md`

**Step 1: Write the failing build smoke test**

Create `tests/baseline/legacy-build.tests.ps1`:

```powershell
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
& (Join-Path $root 'tools\build-legacy.ps1') -Configuration Release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$expected = Join-Path $root 'src\legacy\quicklook\Build'
if (-not (Test-Path -LiteralPath $expected)) {
    throw "Legacy build did not create $expected"
}
Write-Output 'LEGACY_BUILD_OK'
```

**Step 2: Run it and verify the missing-script failure**

```powershell
pwsh -NoProfile -File tests/baseline/legacy-build.tests.ps1
```

Expected: FAIL because `tools/build-legacy.ps1` does not exist.

**Step 3: Implement the minimal build wrapper**

Create `tools/build-legacy.ps1`:

```powershell
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$solution = Join-Path $root 'src\legacy\quicklook\QuickLook.sln'
$msbuild = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" `
    -latest -products * -requires Microsoft.Component.MSBuild `
    -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1

if (-not $msbuild) { throw 'Visual Studio MSBuild was not found' }

& $msbuild $solution /m /restore `
    "/p:Configuration=$Configuration" '/p:Platform=Any CPU' /v:minimal
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

Do not patch QuickLook source to make this pass. Missing machine prerequisites belong in the issue evidence and setup documentation.

**Step 4: Run the baseline build**

```powershell
pwsh -NoProfile -File tests/baseline/legacy-build.tests.ps1
```

Expected: `Build succeeded` followed by `LEGACY_BUILD_OK`.

**Step 5: Record evidence without closing Explore**

Append the command, OS/Visual Studio version, result, and output artifact location to `.cs/issues/2026/07/20/open-quicklook-behavior-baseline/compatibility-matrix.md`. Do not turn a failed prerequisite into a product limitation.

**Step 6: Commit the reproducible build**

```powershell
git add tools/build-legacy.ps1 tests/baseline/legacy-build.tests.ps1 .cs/issues/2026/07/20/open-quicklook-behavior-baseline/compatibility-matrix.md
git commit -m "build: reproduce QuickLook 4.5 baseline"
```

## Task 3: Define the v0 protocol and Rust framing contract

**Files:**

- Create: `rust-toolchain.toml`
- Create: `src/rust/Cargo.toml`
- Create: `src/rust/Cargo.lock`
- Create: `src/rust/crates/previewit-protocol/Cargo.toml`
- Create: `src/rust/crates/previewit-protocol/build.rs`
- Create: `src/rust/crates/previewit-protocol/src/lib.rs`
- Create: `src/rust/crates/previewit-protocol/tests/framing.rs`
- Create: `protocol/preview/v0/preview.proto`

**Step 1: Pin the x64 Rust toolchain**

Create `rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.97.0"
profile = "minimal"
components = ["clippy", "rustfmt"]
targets = ["x86_64-pc-windows-msvc"]
```

No ARM64 target belongs in this file.

**Step 2: Create the workspace and protocol crate**

`src/rust/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/previewit-protocol"]

[workspace.package]
edition = "2024"
license = "GPL-3.0-only"
rust-version = "1.97"
```

Create the library package before adding dependencies:

```powershell
cargo new --lib src/rust/crates/previewit-protocol
```

Initialize dependencies with Cargo so exact compatible versions land in `Cargo.lock`:

```powershell
cargo add --manifest-path src/rust/crates/previewit-protocol/Cargo.toml bytes prost thiserror
cargo add --manifest-path src/rust/crates/previewit-protocol/Cargo.toml --build prost-build protoc-bin-vendored
```

**Step 3: Define the smallest useful schema**

Create `protocol/preview/v0/preview.proto`:

```proto
syntax = "proto3";
package previewit.preview.v0;

message Envelope {
  uint32 protocol_major = 1;
  uint32 protocol_minor = 2;
  string request_id = 3;
  oneof payload {
    Hello hello = 10;
    HelloAck hello_ack = 11;
    OpenDocument open_document = 12;
    Cancel cancel = 13;
    Result result = 14;
    PreviewError error = 15;
  }
}

message Hello {
  string component_id = 1;
  repeated string capabilities = 2;
}

message HelloAck {
  repeated string accepted_capabilities = 1;
}

message OpenDocument {
  fixed64 duplicated_handle = 1;
  uint64 size = 2;
  string display_name = 3;
}

message Cancel {}

message Result {
  string status = 1;
  bytes payload = 2;
}

message PreviewError {
  string code = 1;
  string message = 2;
}
```

Keep v0 narrow. Do not add `DocumentModel`, `SharedSurface`, routing, cache, or plugin manifest messages until their own issues have tests.

**Step 4: Write failing framing tests**

Create `src/rust/crates/previewit-protocol/tests/framing.rs` with tests for:

```rust
use previewit_protocol::{decode_frame, encode_frame, MAX_CONTROL_FRAME};

#[test]
fn frame_round_trips() {
    let payload = b"previewit";
    let frame = encode_frame(payload).unwrap();
    assert_eq!(decode_frame(&frame).unwrap(), payload);
}

#[test]
fn oversized_control_frame_is_rejected() {
    let payload = vec![0_u8; MAX_CONTROL_FRAME + 1];
    assert!(encode_frame(&payload).is_err());
}

#[test]
fn truncated_frame_is_rejected() {
    assert!(decode_frame(&[4, 0, 0, 0, 1, 2]).is_err());
}
```

Run:

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-protocol
```

Expected: FAIL because the framing API does not exist.

**Step 5: Implement minimal length framing**

In `src/lib.rs`, expose generated messages and a 1 MiB control-frame limit. The length prefix is an unsigned 32-bit little-endian integer:

```rust
pub const MAX_CONTROL_FRAME: usize = 1024 * 1024;

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_CONTROL_FRAME {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let len = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge(payload.len()))?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}
```

`decode_frame` must require exactly one complete frame and reject declared lengths above `MAX_CONTROL_FRAME`. `build.rs` must use `protoc-bin-vendored` and compile the root schema; no machine-global `protoc` dependency is allowed.

**Step 6: Run Rust quality checks**

```powershell
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src/rust/Cargo.toml --workspace
```

Expected: all PASS.

**Step 7: Commit the Rust contract**

```powershell
git add rust-toolchain.toml src/rust protocol/preview/v0/preview.proto
git commit -m "feat: define preview protocol v0 framing"
```

## Task 4: Implement the same protocol contract for .NET Framework

**Files:**

- Create: `src/dotnet/PreviewIt.Protocol/PreviewIt.Protocol.csproj`
- Create: `src/dotnet/PreviewIt.Protocol/FramedProtocol.cs`
- Create generated-at-build source through: `protocol/preview/v0/preview.proto`
- Create: `tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj`
- Create: `tests/dotnet/PreviewIt.Protocol.Tests/FramedProtocolTests.cs`

**Step 1: Create projects with locked package restore**

The library targets `net462` because it must be loadable by the QuickLook compatibility side. Set `RestorePackagesWithLockFile` to `true`. Add `Google.Protobuf` and `Grpc.Tools` (`PrivateAssets="All"`) to the library; add MSTest and `Microsoft.NET.Test.Sdk` to the test project. Reference the shared `.proto` via a relative path and generate C# during build.

**Step 2: Write failing parity tests**

`FramedProtocolTests.cs` must cover the same cases as Rust:

```csharp
[TestMethod]
public void FrameRoundTrips()
{
    var payload = Encoding.UTF8.GetBytes("previewit");
    var frame = FramedProtocol.Encode(payload);
    CollectionAssert.AreEqual(payload, FramedProtocol.Decode(frame));
}

[TestMethod]
public void OversizedControlFrameIsRejected()
{
    Assert.ThrowsException<InvalidDataException>(() =>
        FramedProtocol.Encode(new byte[FramedProtocol.MaxControlFrame + 1]));
}
```

Run:

```powershell
dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release
```

Expected: FAIL because `FramedProtocol` does not exist.

**Step 3: Implement exact framing parity**

Implement a 4-byte little-endian prefix, exact-frame validation, and the same 1 MiB maximum. Do not use `BinaryReader.ReadString`, JSON, or unbounded stream reads.

**Step 4: Add Protobuf round-trip coverage**

Construct an `Envelope` with protocol `0.1`, `request_id="request-1"`, and `Hello.component_id="dotnet-probe"`; serialize and parse it, then assert every field and capability. Add an equivalent Rust unit test using the generated `prost` types.

**Step 5: Run both language suites**

```powershell
dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release
cargo test --manifest-path src/rust/Cargo.toml --workspace
```

Expected: all PASS and committed `packages.lock.json`/`Cargo.lock` files.

**Step 6: Commit**

```powershell
git add src/dotnet tests/dotnet src/rust protocol
git commit -m "feat: add dotnet preview protocol parity"
```

## Task 5: Prove an authenticated Rust-to-.NET Named Pipe handshake

**Files:**

- Modify: `src/rust/Cargo.toml`
- Create: `src/rust/crates/previewit-broker/Cargo.toml`
- Create: `src/rust/crates/previewit-broker/src/main.rs`
- Create: `src/rust/crates/previewit-broker/src/pipe.rs`
- Create: `src/rust/crates/previewit-broker/tests/dotnet_handshake.rs`
- Create: `src/dotnet/PreviewIt.WorkerProbe/PreviewIt.WorkerProbe.csproj`
- Create: `src/dotnet/PreviewIt.WorkerProbe/Program.cs`

**Step 1: Write the failing end-to-end test**

The Rust integration test must:

1. create a unique pipe name containing a random nonce, not a file path;
2. launch the built `PreviewIt.WorkerProbe.exe` with `--pipe <name>`;
3. read a framed `Hello`;
4. call `GetNamedPipeClientProcessId` and compare it to the launched child PID;
5. negotiate protocol major `0`, minor `1`, and the `read-handle-v0` capability;
6. send `HelloAck` and assert a clean child exit.

Run it before implementation:

```powershell
dotnet build src/dotnet/PreviewIt.WorkerProbe/PreviewIt.WorkerProbe.csproj -c Release
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test dotnet_handshake
```

Expected: FAIL because the Broker crate and worker do not exist.

**Step 2: Implement the current-user pipe boundary**

Create the server with `CreateNamedPipeW`, local-only mode, and a DACL containing only `SYSTEM` and the current token user. After connection, verify the client PID equals the expected child. Reject any mismatch before reading protocol bytes.

The worker connects with `NamedPipeClientStream`, writes a framed `Hello`, reads `HelloAck`, and exits. Both sides must time out rather than block forever.

**Step 3: Add negative tests**

Add tests that reject:

- a client process other than the launched child;
- a mismatched protocol major;
- a control frame larger than 1 MiB;
- a connection that does not complete before the startup deadline.

**Step 4: Run and commit**

```powershell
dotnet build src/dotnet/PreviewIt.WorkerProbe/PreviewIt.WorkerProbe.csproj -c Release
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test dotnet_handshake
git add src/rust src/dotnet
git commit -m "feat: prove authenticated broker worker handshake"
```

## Task 6: Prove read-only handle transfer and process containment

**Files:**

- Create: `tests/fixtures/handles/read-only.txt`
- Modify: `src/rust/crates/previewit-broker/src/main.rs`
- Create: `src/rust/crates/previewit-broker/src/handles.rs`
- Create: `src/rust/crates/previewit-broker/src/supervisor.rs`
- Create: `src/rust/crates/previewit-broker/tests/handle_transfer.rs`
- Create: `src/rust/crates/previewit-broker/tests/supervision.rs`
- Modify: `src/dotnet/PreviewIt.WorkerProbe/Program.cs`

**Step 1: Add a fixed fixture**

`tests/fixtures/handles/read-only.txt` contains exactly:

```text
previewit-read-only
```

**Step 2: Write failing handle tests**

The positive test opens the fixture with `GENERIC_READ`, shares only what the use case needs, duplicates the handle into the authenticated child process with read access, sends the target-process handle value in `OpenDocument`, and expects `Result.status="read-ok"` with the fixture bytes.

The negative assertion asks the worker to write one byte using the received handle and expects `ERROR_ACCESS_DENIED`. Verify the file contents remain unchanged.

Do not mark a writable handle inheritable and do not pass an unrestricted path as a fallback.

**Step 3: Implement minimal handle transfer**

Use `CreateFileW` with `GENERIC_READ`, then `DuplicateHandle` into the known child process without broadening access. The Broker owns and closes the source handle; the .NET worker wraps the target-process duplicate in an owning `SafeFileHandle`, reads the advertised size, reports the write failure separately, and closes its duplicate.

Ensure exactly one side owns and closes each handle. Add a repeated test (at least 100 transfers) and assert the Broker handle count returns to its starting range.

**Step 4: Write failing supervision tests**

Give the worker explicit test-only modes:

- `--mode crash`: exit with a non-zero code after handshake;
- `--mode hang`: ignore `Cancel` beyond the test deadline;
- `--mode stale`: return a result for the previous `request_id` after a new request begins.

Tests must assert that the Broker process remains alive, terminates the hung child through its Job Object, and never accepts the stale result as current.

**Step 5: Implement the smallest supervisor**

The foundation supervisor owns one child, one Job Object, startup/render/cancel deadlines, and the current `request_id`. It is not yet a general worker pool. On timeout it closes the job, records a typed error, and can start a fresh probe for the next test.

**Step 6: Run and commit**

```powershell
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test handle_transfer
cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test supervision
dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release
git add tests/fixtures src/rust src/dotnet
git commit -m "feat: prove handle isolation and worker recovery"
```

## Task 7: Add one local quality gate and capture evidence

**Files:**

- Create: `tools/test-foundation.ps1`
- Create: `.github/workflows/foundation.yml`
- Modify: `.cs/issues/2026/07/22/open-preview-foundation-vertical-slice.md`
- Modify: `.cs/epics/2026/07/20/rust-hybrid-preview-architecture/spec.md` only if execution disproves an approved assumption

**Step 1: Write the local gate**

`tools/test-foundation.ps1` runs, in order:

```powershell
pwsh -NoProfile -File tools/upstream/verify-quicklook-baseline.ps1
pwsh -NoProfile -File tests/baseline/legacy-build.tests.ps1
cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check
cargo clippy --manifest-path src/rust/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src/rust/Cargo.toml --workspace
dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release
```

The script stops at the first failure and returns the failing exit code.

**Step 2: Add Windows CI**

`.github/workflows/foundation.yml` uses a Windows runner, checks out the repository (the subtree source is already tracked), installs the pinned Rust toolchain, restores NuGet/Cargo caches, and runs only `pwsh -File tools/test-foundation.ps1`. It does not publish packages or alter releases.

**Step 3: Run the full gate locally**

```powershell
pwsh -NoProfile -File tools/test-foundation.ps1
```

Expected markers include:

```text
QUICKLOOK_BASELINE_OK=b13df028f3cce1f84792f7043b57bf5cea3a3e4c
LEGACY_BUILD_OK
test result: ok
```

**Step 4: Update CodeStable evidence**

Record the exact commands, environment, test counts, handle-count observation, timeout behavior, and any incompatibility in the foundation issue. If evidence contradicts the Epic—for example, net462 cannot consume the chosen Protobuf package or a read-only duplicated handle is not viable—update the Epic before continuing. Do not silently substitute path access or weaken the current-user boundary.

Do not close the foundation or Explore issue. Stop and request the user's closing authorization after presenting evidence.

**Step 5: Commit the verified gate**

```powershell
git add tools/test-foundation.ps1 .github/workflows/foundation.yml .cs
git commit -m "test: gate preview foundation vertical slice"
git status --short
```

Expected: clean worktree.

## Follow-on issue order

After this plan passes and the user authorizes issue closure, create and design the next CodeStable issues one at a time:

1. Rust Broker single-instance and request state machine.
2. Shell Resolver and restricted x86 Dialog Adapter.
3. Viewer/Legacy Host process boundary.
4. Renderer registry, manifest, supervisor policy, and cache.
5. System `IPreviewHandler` route.
6. Internal text Renderer v0.
7. Internal image Renderer v0.
8. Internal archive Renderer v0.
9. Public Renderer SDK v1 readiness.

Each issue must start from the source and evidence available at that time. Do not turn this sequence into one long-lived implementation branch.
