# Rust Broker Acceptance Remediation Design

**Status:** Approved by the user on 2026-07-23.

**Source of truth:** `.cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md`. This document records the accepted remediation choice; the Issue remains authoritative when wording differs.

**Scope:** Fix the four acceptance findings in the x64 Broker control foundation. Keep the Issue and behavior Explore open and the architecture Epic draft. Do not add ARM64, Shell Resolver, Viewer, Renderer, x86 Dialog Adapter, commands, deployment, or publication work.

## Problem

The convergence implementation passes its focused and full gates, but the gates do not prove the claimed combined-capacity and ownership semantics:

1. `8 queued + 1 active + 4 decoding + 1 listener` consumes all 14 pipe instances. The next client connects the listener, replacement creation hits `ERROR_PIPE_BUSY`, and the endpoint exits.
2. Security integration tests obtain a short-lived initial listener HANDLE and current-user SID through release public API hidden only from documentation.
3. `CommandRouter` feeds `Cancel`/`Cleanup` back as synthetic completion, while `BrokerRuntime` ignores every non-stale effect and compares only phases. Effect ownership and replacement observability are split.
4. Queue saturation uses synchronous `FlushFileBuffers` without a deadline, so the regression test itself can hang indefinitely.

## Considered approaches

### A. Converge ownership and capacity — selected

Model every live pipe instance, keep both an admission/rejection slot and a standby listener, make saturation recoverable, move cleanup feedback into one Runtime effect pump, and remove inspection from the release API. Replace blocking test coordination with bounded clients and typed events.

This changes the fewest responsibility boundaries needed to make the Issue claims true and leaves one clear insertion point for the future Resolver.

### B. Patch only the symptoms — rejected

Increase `MAX_PIPE_INSTANCES`, feature-gate the current inspection methods, and wrap `FlushFileBuffers` in another thread. This is smaller but preserves split effect ownership, a stale-handle-shaped API, and test coordination that cannot actually cancel the blocked Win32 call.

### C. Add an acceptor/dispatcher subsystem — rejected

Separate accept, admission, decode, dispatch, and response into new services. This can model capacity precisely but creates shallow interfaces for a two-command endpoint and is unnecessary for the current scale.

## Accepted design

### Endpoint capacity and recovery

The endpoint derives its Win32 maximum from five independent dimensions:

```text
8 queued commands
1 active command awaiting its reply
4 connections decoding a frame
1 connected admission/rejection instance
1 standby listener
= 15 pipe instances
```

The accept loop still creates the replacement before decoding. With the extra admission instance, a fully occupied endpoint can accept one connection, create the standby listener, return `primary-busy`, and drop the rejected connection without losing availability. An unexpected `ERROR_PIPE_BUSY` during replacement is treated as recoverable saturation: reject/close the connected instance, recreate the listener after capacity is released, and continue. Other replacement failures remain terminal because the endpoint cannot promise availability without a listener.

A real integration test holds one active `PendingCommand`, fills all eight queued slots, occupies all four decoders with partial frames, and sends one more valid command. It must receive `primary-busy`; after releasing/responding to held work, a new command must be accepted.

### Runtime owns effects

`CommandRouter` performs exactly one reducer step for each command or session event and returns the resulting effects. It never executes effects or fabricates completion events.

`BrokerRuntime` owns a small exhaustive effect pump. In this Issue no execution resource exists, so only `Cancel` and `Cleanup` synchronously feed one `CleanupComplete(request_id)` back into the router. Every reducer step is observed independently. Other effect variants have an explicit current-scope action in the single pump; the future Shell/Worker issue replaces those no-resource actions there rather than adding a second runner.

Transitions compare both phase and request identity. Replacing an active request therefore emits `Resolving(old) -> Closing(old)` followed by `Closing(old) -> Resolving(next)`, instead of disappearing as `Resolving -> Resolving`.

### Test-only security inspection

Security tests remain real Win32 handle/ACL tests, but the test module itself creates and owns the real server handle through the private command endpoint constructor. The release `BrokerCommandServer` readiness handshake returns only success, stores no inspection HANDLE, and exposes no inspection method. The crate root no longer exports a SID inspection helper; any helper needed by crate unit tests is at most `pub(crate)`.

### Bounded test coordination

Queue and combined-capacity tests use clients whose connect/read/write operations already have explicit deadlines plus the canonical `CommandQueueFull` event as the readiness proof. Pacing may prevent decoder saturation from obscuring a queue-specific test, but a sleep is never the success assertion. `FlushFileBuffers` is removed.

## Verification

- RED/GREEN combined-capacity test and post-saturation accepted round trip.
- RED/GREEN runtime replacement event sequence and pure-router state/effect tests.
- Real DACL, remote-rejection, handle-inheritance, and current-user round-trip tests with no release inspection symbols.
- Existing contract, slow client, queue, decoder, shutdown, process election, Worker, and protocol suites.
- Focused repetition followed by `tools/test-foundation.ps1`.
- Only `x86_64-pc-windows-msvc` installed/configured; PE remains `8664 machine (x64)`; scoped ARM64 search remains empty.
