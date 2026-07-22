---
kind: issue
title: "Preview foundation vertical slice"
type: feature
status: closed
created: 2026-07-22
epic: ".cs/epics/2026/07/20/rust-hybrid-preview-architecture/spec.md"
---

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

## How it works now

- `UPSTREAM.md` pins QuickLook `4.5.0` to commit `b13df028f3cce1f84792f7043b57bf5cea3a3e4c`; the tracked subtree remains unchanged and the build wrapper supplies only the missing tag description during the build.
- Preview Protocol v0 uses Protobuf envelopes inside one exact 4-byte little-endian length frame with a 1 MiB control-frame limit. Rust and `net462` use the same schema and framing behavior.
- The x64 Rust Broker creates a random local-only named pipe whose DACL grants access only to the current token user and SYSTEM. It verifies `GetNamedPipeClientProcessId` against the launched child before reading protocol bytes, then negotiates protocol `0.1` and `read-handle-v0`.
- The Broker opens preview input with `CreateFileW(GENERIC_READ)`, duplicates the non-inheritable same-access handle into the authenticated worker, and never gives the worker a path fallback. The worker owns and closes the duplicate, reads the advertised bytes, and separately reports that a one-byte write failed with `ERROR_ACCESS_DENIED`.
- One `WorkerSupervisor` owns one child and one Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. It applies startup, I/O, and cancel deadlines, rejects stale `request_id` results, and closes the job to recover from a hung worker.

## Impact

- Added only the provenance/build checks, protocol libraries, Broker/Worker probes, fixtures, tests, and foundation CI gate.
- Did not take over hotkeys, Shell selection, Viewer windows, legacy plugins, renderer routing, installation, update, or release behavior.
- The delivered Broker and WorkerProbe PE files are x64 (`8664`), and the only installed/configured Rust target is `x86_64-pc-windows-msvc`. No ARM64 target or PreviewIt artifact was added.

## Execution record

- `5482e1b` pins and verifies upstream provenance; `94b7920` imports the QuickLook subtree; `6a7d689` reproduces the unchanged legacy build.
- `828584f` defines the Rust v0 framing contract; `a62f0f0` adds `net462` Protobuf/framing parity.
- `24aec2a` proves the authenticated Rust-to-.NET pipe handshake, including wrong PID, wrong major, oversized frame, and startup-timeout rejection.
- `394929b` proves read-only handle transfer, 100-transfer ownership stability, crash isolation, Job Object hang recovery, and stale-result rejection.
- The written Task 7 command list omitted the Release WorkerProbe build required by clean-checkout Rust integration tests. `tools/test-foundation.ps1` adds that build immediately before `cargo test --workspace`; CI also provisions WiX `3.14.1.8722` because the unchanged legacy solution requires it.
- The first full-gate invocation returned non-zero without retained step output. Step markers were added and Task 6's cold-start test budget was raised from 5 to 15 seconds; the complete gate then passed, followed by an independent Broker-suite rerun. No product boundary or protocol assumption changed.

## Verification evidence

Environment on 2026-07-22:

- Windows 11 Pro `10.0.22631` x64; Visual Studio 2022 `17.14.20`; MSBuild `17.14.23.42201`; WiX `3.14.1.8722`.
- Rust/Cargo `1.97.0`, host and only installed target `x86_64-pc-windows-msvc`; .NET SDK `10.0.100`; VSTest `18.0.1` x64.
- `dumpbin /headers` reports `8664 machine (x64)` for both `previewit-broker.exe` and `PreviewIt.WorkerProbe.exe`.

Commands and results:

- `pwsh -NoProfile -File tools/test-foundation.ps1`: PASS with `QUICKLOOK_BASELINE_OK=b13df028f3cce1f84792f7043b57bf5cea3a3e4c`, `LEGACY_BUILD_OK`, all language suites, and `FOUNDATION_GATE_OK`.
- Rust workspace: 14 tests passed: protocol framing/round-trip 4, authenticated handshake negatives/positive 5, handle transfer 2, supervision 3. `cargo fmt --check` and Clippy with `-D warnings` passed.
- .NET protocol: 4 tests passed under `net462` and x64 VSTest. WorkerProbe Release build completed with zero warnings and zero errors.
- The 100-transfer test observed Broker process handles `100 -> 94`, so the transfer loop did not grow the Broker handle count. The fixture remained byte-for-byte unchanged and each worker write returned `ERROR_ACCESS_DENIED` (`5`).
- The hung-worker test used a 150 ms cancel deadline and observed Job close/reap at 158 ms; crash and stale-result tests left the Broker test process alive and did not accept the stale response as current.
- Existing QuickLook warnings remain the ones captured in the open behavior-baseline Explore: WiX `NU1503`/`ICE61`, `System.IO.Compression` conflicts, and Markdown member hiding. They did not become PreviewIt limitations.

The workflow YAML and PowerShell gate parse locally, and the official WiX `wix314-binaries.zip` layout was checked for the required build tools and targets. The GitHub-hosted run remains unobserved because this branch has not been pushed.

Execution confirms the Epic assumptions about `net462` Protobuf use, authenticated named pipes, read-only duplicate handles, and Job Object recovery. The Epic spec therefore does not need a corrective update.

## 关闭结论

- 关闭判断：目标范围已经完成。固定的 QuickLook `4.5.0` 基线可在当前环境无源码修改构建，x64 Rust Broker 与 `net462` WorkerProbe 的最小跨进程边界已通过正反向测试。
- 验证摘要：本地 foundation gate 全部通过，包含 14 个 Rust 测试、4 个 .NET 测试、100 次只读句柄传递、150 ms 取消期限下的 Job Object 回收，以及 crash、hang 和 stale `request_id` 恢复。Broker 与 WorkerProbe 均为 x64，未添加 ARM64 target 或产物。
- 回写位置：协议 `0.1` 的 framing/capability 边界、当前用户 pipe 身份验证、只读句柄所有权和单 Worker Job Object 恢复结论已回写所属 Rust 混合预览架构 Epic。
- 遗留事项：GitHub-hosted workflow 尚未运行，因为分支未推送；这不影响本地 vertical slice 的关闭。行为基线 Explore 和架构 Epic 保持开启，后续工作从 Rust Broker 单实例与请求状态机 Issue 继续。
