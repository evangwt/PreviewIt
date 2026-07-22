---
kind: issue
title: "Preview foundation vertical slice"
type: feature
status: open
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
