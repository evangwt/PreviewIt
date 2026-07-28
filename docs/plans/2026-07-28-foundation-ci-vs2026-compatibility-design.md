# Foundation CI VS 2026 Compatibility Design

**Status:** Approved by the user on 2026-07-28.

**Source of truth:** `.cs/issues/2026/07/28/open-foundation-ci-gate-failure.md`. This document preserves accepted choice; Issue remains authoritative if wording differs.

**Scope:** Restore Foundation CI for QuickLook 4.5 baseline. Do not change QuickLook source, native include paths, warning policy, WiX version, Rust setup, existing gate-step order, or production behavior. Prepend one workflow contract check to Foundation gate.

## Problem

`windows-latest` now resolves to `windows-2025-vs2026`. `tools/build-legacy.ps1` deliberately discovers latest MSBuild, so it selects MSBuild 18.7.8. The QuickLook Native32 and Native64 projects then fail with `C1083` because `atlcomcli.h` is unavailable to that compiler environment.

After pinning Windows Server 2022, the native projects compile, exposing installer failure: workflow writes `WixToolPath=$wixBin` without a trailing separator, while WiX v3 targets concatenate the tool name. The resulting `wix314bin\heat` path cannot exist.

## Considered approaches

### A. Pin Foundation to Windows Server 2022 — selected

Set `runs-on: windows-2022`. Its hosted image provides VS 2022/MSBuild 17, matching the local successful legacy build. The runner label is the sole environment-selection point, so this confines change to CI and preserves the fixed QuickLook baseline.

### B. Repair VS 2026 ATL setup — rejected

Install or configure ATL explicitly on the VS 2026 runner. The image manifest already claims that component, while compilation still cannot find its header. Adding path overrides or installation steps would hide an upstream runner inconsistency, increase CI complexity, and lack a local reproduction.

### C. Modify QuickLook native projects — rejected

Hard-code include paths or replace ATL usage. This alters fixed upstream behavior to accommodate a CI image and risks incompatible binaries; it does not repair the environment selection error.

### D. Correct WiX environment value — selected

Write `WixToolPath=$wixBin\`. WiX v3 targets own the tool invocation and require this path form; correcting the workflow value preserves the pinned archive, installer source, and existing build wrapper.

## Accepted design

Foundation runs on Windows Server 2022, which keeps the legacy solution on its known working Visual Studio generation. Its WiX install step writes a trailing separator in `WixToolPath`, allowing WiX v3 targets to locate `heat`. `build-legacy.ps1` retains ownership of MSBuild discovery and build invocation. Foundation gate runs a small workflow contract test first, so future edits cannot silently restore either requirement.

## Verification

1. Focused test fails for both floating runner label and missing WiX trailing separator.
2. Set `windows-2022` and `WixToolPath=$wixBin\`.
3. Foundation gate invokes focused test first, then local `tools/test-foundation.ps1` stays green.
4. GitHub Actions must complete `FOUNDATION_GATE_OK` on `windows-2022`, including installer packaging.
