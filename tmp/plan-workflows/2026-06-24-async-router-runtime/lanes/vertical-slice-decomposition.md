# Lane: vertical-slice-decomposition

Status: answered
Reasoning effort: high
Security context: applicable

## Candidate Slices

1. Single async production `serve` path.
   - Replace the release entry with one Tokio-owned runtime, loopback listener,
     and explicit shutdown/cancellation ownership.
   - Primary write surface: `crates/codex-router-cli/src/lib.rs`,
     `crates/codex-router-proxy/src/server.rs`, proxy/CLI manifests.
   - Proof: boot/runtime ownership/structural reachability.

2. Async state/auth boundary.
   - Introduce state-owned SQLx async contracts for runtime selection, affinity,
     quota, and credential state.
   - Preserve auth-owned credential refresh logical commit.
   - Split into 2A request-time async repositories and 2B auth refresh commit if
     one slice is too large.

3. Hyper HTTP/SSE proxy cutover.
   - Move local serving and upstream HTTP/SSE transport to Hyper while preserving
     auth, route classification, header sanitation, streaming, affinity, and
     audit semantics.
   - Split local serving from upstream transport only if streaming body
     adaptation forces a broad trait rewrite.

4. WebSocket accept, first-frame routing, and pre-upstream failures.
   - Move local upgrade to Hyper + `tokio-tungstenite`, preserve bounded
     first-frame metadata parsing, tokenless default, explicit token mode, and
     deterministic post-upgrade/pre-upstream closes.
   - Must not include post-upstream pump implementation.

5. WebSocket duplex pumps, session supervision, revocation, and non-blocking
   close paths.
   - Replace response-turn-gated blocking pumps with supervised async
     bidirectional pumps.
   - Keep session registry, close reasons, revocation, cleanup, and pump-side
     side-effect isolation in the same source-owned slice.
   - Split into 5A duplex pumps/cleanup and 5B registry/observability only if
     proof cannot fit one slice.

6. Auth-owned credential refresh commit.
   - Ensure callers never observe half-committed secret material, active
     generation, or quota invalidation.
   - Can start after state/auth contract work; does not require full HTTP/WS
     cutover.

7. Release reachability guardrails and real-client acceptance harness.
   - Structural and behavioral checks prove release `serve` uses the async stack
     only, reproduces and closes the old stuck failure, and proves installed
     Codex concurrency/soak through one shared router PID.
   - Split into 7A structural + real-serve regression and 7B installed-Codex
     e2e/soak if needed.

## Open Planning Decisions

- SQLx cutover shape is unresolved in repo: the plan must avoid leaving two
  production state paths.
- Final proof should spawn the release `codex-router serve` binary directly or a
  compiled helper that still traverses the CLI path; the plan should prefer the
  release binary for clarity.
- Release ownership guardrail must cover reachability across crates, not only
  file-local grep.

## Completion Receipt

Returned source-owned slices with live repo anchors, dependencies, checkpoints,
proof layers, and split triggers.

Confidence: medium-high
