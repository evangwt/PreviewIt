# Foundation CI VS 2026 Compatibility Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restore Foundation CI by pinning VS 2022-compatible Windows Server 2022 and supplying QuickLook Installer with a correctly terminated WiX root path.

**Architecture:** Keep runner and WiX root environment selection in `.github/workflows/foundation.yml`, where workflow already owns hosted-environment choice. Add dependency-free PowerShell contract test for both values and run it as Foundation gate's first checked step; leave QuickLook source and `tools/build-legacy.ps1` untouched.

**Tech Stack:** GitHub Actions YAML, PowerShell 7, existing Foundation gate.

---

### Task 1: Prove workflow environment contract

**Files:**
- Create: `tests/baseline/foundation-workflow.tests.ps1`
- Test: `tests/baseline/foundation-workflow.tests.ps1`

**Step 1: Write failing test**

Create PowerShell test that resolves repository root, reads `.github/workflows/foundation.yml`, and throws unless file contains job-level `runs-on: windows-2022` plus QuickLook's WiX root assignment `WIX=$wixRoot\`. Print `FOUNDATION_WORKFLOW_OK` only after assertions succeed.

**Step 2: Run test to verify it fails**

Run:

```powershell
pwsh -NoProfile -File tests/baseline/foundation-workflow.tests.ps1
```

Expected: nonzero exit for missing `windows-2022` or missing `WIX` trailing separator.

**Step 3: Commit test checkpoint**

Do not commit automatically. Prepare this checkpoint only with explicit user authorization.

### Task 2: Pin Foundation environment and enforce QuickLook contract

**Files:**
- Modify: `.github/workflows/foundation.yml:11`
- Modify: `tools/test-foundation.ps1:20`
- Test: `tests/baseline/foundation-workflow.tests.ps1`

**Step 1: Write minimal implementation**

Replace:

```yaml
runs-on: windows-latest
```

with:

```yaml
runs-on: windows-2022
```

Also replace:

```powershell
"WIX=$wixRoot"
```

with:

```powershell
"WIX=$wixRoot\"
```

Prepend this checked step before provenance; do not alter existing step order:

```powershell
Invoke-Checked 'workflow-runner' { pwsh -NoProfile -File tests/baseline/foundation-workflow.tests.ps1 }
```

Do not change WiX version/archive, Rust, cache, permissions, `tools/build-legacy.ps1`, or QuickLook files.

**Step 2: Run focused test to verify it passes**

Run:

```powershell
pwsh -NoProfile -File tests/baseline/foundation-workflow.tests.ps1
```

Expected: exit 0 and `FOUNDATION_WORKFLOW_OK`.

**Step 3: Run local regression gate**

Run:

```powershell
pwsh -NoProfile -File tools/test-foundation.ps1
```

Expected: exit 0; first lines contain `FOUNDATION_STEP=workflow-runner`, `FOUNDATION_WORKFLOW_OK`, and final output contains `FOUNDATION_GATE_OK`.

**Step 4: Check changed files**

Run:

```powershell
git diff --check
git diff -- .github/workflows/foundation.yml tools/test-foundation.ps1 tests/baseline/foundation-workflow.tests.ps1
```

Expected: whitespace check succeeds; diff contains runner pin, WiX root separator, contract test, and its gate entry.

**Step 5: Commit implementation checkpoint**

Do not commit or push automatically. With user authorization, commit workflow, test, CodeStable issue, and approved design/plan documents together.

### Task 3: Verify hosted environment

**Files:**
- Modify: `.cs/issues/2026/07/28/closed-foundation-ci-gate-failure.md`

**Step 1: Push and observe CI after explicit authorization**

Run normal user-authorized push. Then inspect resulting Foundation run:

```powershell
gh run list --repo evangwt/PreviewIt --workflow Foundation --branch master --limit 1
gh run view <run-id> --repo evangwt/PreviewIt --log-failed
```

Expected: runner reports `windows-2022`; installer runs `heat` from a separated `WIX` root; Foundation exits 0 and logs `FOUNDATION_GATE_OK`.

**Step 2: Record evidence**

Update Issue verification and execution records with run id, runner image, and final gate result. Do not close issue without user authorization.
