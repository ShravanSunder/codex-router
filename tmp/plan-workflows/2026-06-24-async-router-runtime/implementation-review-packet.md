# Implementation Review Packet: Async Router Runtime

mode: implementation
review_class: plan-backed + risk-triggered
source_backed_verdict_attempted: true
whole-source-trace: required

## Accepted Request

The user required codex-router to stop using a fragile hand-managed WebSocket
server path and prove that multiple real installed Codex clients can use one
router concurrently over WebSockets. PR readiness requires hard gates, review,
checkpoint commits, and proof that the exact multi-client failure class cannot
return silently.

## Source Spec And Plan

- Spec: `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
- Plan: `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
- Goal details: `tmp/workflow-state/2026-06-24-async-router-runtime/details.md`

Material plan rows:
- E-02: five-minute three-runtime WebSocket soak.
- E-03: overlap window timestamps prove concurrent activity.
- E-04: each runtime has repeated frame activity during overlap.
- E-05: one runtime completes a function-call-style continuation while the
  three-client WebSocket overlap is still active.
- E-06: router records active high-water 3 and zero active after completion.
- E-08: live router socket table has no leaked ESTABLISHED or CLOSE_WAIT TCP
  sessions after completion.
- I-17b: slow affinity recording cannot gate WebSocket forwarding.
- G-01/G-02/G-03/G-04/G-05/G-07/G-21/G-23: structural guardrails.

Known remaining gaps:
- Final implementation re-review is running after the current proof refresh.
- PR wrapup is not complete.

## Git Scope

Review commits from `origin/main` through `499667b`, especially:
- `0170054 Harden async router proof gates`
- `b8e1c4a Fix structural proof row dispatch`
- `fa86eba Tighten structural reachability guard`
- `ff689cb Redact structural proof cwd`
- `2256b89 Record structural proof receipts`
- `f5126b5 Add missing integration proof rows`
- `17ead87 Record missing integration proof receipts`
- `a09ee84 Persist smoke proof transcripts`
- `1df684f Record smoke proof transcripts`
- `e191ce6 Correct runtime correlation proof check`
- `071adf1 Persist e2e soak transcript`
- `eb50e11 Record e2e websocket soak proof`
- `da18c3e Refresh async runtime proof receipts`
- `670cbb6 Fix async runtime review blockers`
- `8972874 Tighten async runtime proof fidelity`
- `e16914e Refresh async runtime proof evidence`
- `3515851 Fix JoinSet drain clippy guard`
- `499667b Refresh async runtime proof after JoinSet guard`

Changed implementation surfaces:
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`
- `scripts/check-release-runtime-guardrails.py`
- `scripts/proof-matrix.sh`

Proof artifacts:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-06.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/integration/I-17b.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/*.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T8c-live-websocket-registry-proof.md`

## Current Proof Claims

Final source checkpoint:
- Source commit for runtime code: `3515851b3635c87d0dcdf76d8a5d23101bd6cd32`.
- Evidence refresh commit: `499667bf81c518c2770760aa76be7c79575fb4d9`.

Command gates after `3515851`:
- `cargo fmt --all -- --check`
  exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings`
  exit 0.
- `cargo nextest run --workspace --no-fail-fast --status-level fail --final-status-level slow`
  exit 0; 277 passed, 10 skipped.
- `cargo deny check`
  exit 0; duplicate dependency warnings only.
- `cargo audit`
  exit 0.
- `git diff --check`
  exit 0.
- `rg -n "/Users/|/var/folders/" tmp/plan-workflows/2026-06-24-async-router-runtime/evidence`
  exit 1 with no matches.

Proof-matrix gates after `3515851`:
- `scripts/proof-matrix.sh` rows `G-01` through `G-23`
  all exit 0.
- `scripts/proof-matrix.sh` rows `I-05a I-05b I-17b I-18 I-19 I-20 I-21`
  all exit 0.
- `scripts/proof-matrix.sh` rows `S-01 S-02 S-03 S-04`
  all exit 0.
- `scripts/proof-matrix.sh` rows `E-01` through `E-09`
  all exit 0 against the fresh soak transcript.

Five-minute soak artifact:
- Producer command:
  `tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak`
- Exit 0; 1 passed; 303.34s.
- Raw transient artifact:
  `tmp/smoke/installed-codex-three-websocket-15680-1782340805758.json`.
- Committed transcript:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/three-websocket-soak-transcript.json`.
- transcript git_head=`3515851b3635c87d0dcdf76d8a5d23101bd6cd32`.
- clients.count=3, clients.all_success=true,
  stderr_transport_error_markers=[[],[],[]].
- router_websocket_registry.handled_connections=3,
  high_water_sessions=3, registered_session_ids=[1,2,3],
  closed_session_ids=[3,2,1], active_sessions=0.
- runtime_correlations record observed router_session_id and
  upstream_session_id for each client, with observed booleans true.
- socket_cleanup.established_count=0, close_wait_count=0,
  raw_state_counts=[].

Accepted review findings addressed:
- Upgraded WebSocket errors now propagate out of bounded `serve_protocol_connections`
  instead of being stderr-only.
- Post-completion reset-without-close is accepted as clean teardown only after a
  `response.completed` has already been forwarded; pre-completion failures still
  propagate.
- Registry proof report is now written by the CLI finalizer from an in-memory
  snapshot, not by blocking filesystem writes inside the async tunnel hot path.
- Completion sample history is bounded, and E-04 uses final per-session counts
  so one busy session cannot satisfy the three-session proof.
- Proof rows consume an explicit soak artifact pointer written by the successful
  producing run; they no longer glob the latest artifact.
- WebSocket response lifecycle now marks a response outstanding only for local
  Text/Binary request frames; idle Ping/Pong/Close control frames after
  completion cannot turn a later clean reset into an error.
- Proof rows reject dirty guarded source paths before accepting a fresh artifact,
  so a local implementation/proof change cannot be hidden behind an artifact
  whose git_head already equals HEAD.
- WebSocket duplex forwarding now uses separately supervised local-to-upstream
  and upstream-to-local pump tasks; revocation, serve shutdown, or either pump
  completion aborts and awaits the sibling pump.
- Serve shutdown with a cancellation token now cancels session-level WebSocket
  work and awaits active handlers before returning.
- HTTP/SSE release request preparation now uses async SQLx-backed state and
  async selector/credential contracts instead of sync SQLite inside
  `spawn_blocking`.
- Structural guardrails now scan the release proxy source surface rather than
  only fixed obvious files.

## Security Context

Sensitive boundaries include local loopback routing, authorization header
stripping/injection, upstream OAuth token handling, local auth rejection,
redacted audit/proof artifacts, and subprocess-based installed Codex e2e. Review
must check that proof/report paths do not expose secrets and that debug/proof
flags do not weaken normal serve behavior.

## Review Questions

1. Does the implementation satisfy the accepted async runtime/WebSocket
   requirement without reverting to single-lane or blocking WebSocket behavior?
2. Does the proof genuinely exercise real installed Codex clients over
   WebSockets through one router child?
3. Are the registry report and proof-only CLI flags safe, scoped, and not
   misleading?
4. Are there blocker/important correctness, security, cleanup, or proof gaps
   that must route back to implementation before PR wrapup?
5. Are the two latest re-review fixes sufficient: Ping/Pong post-completion
   lifecycle handling and dirty guarded source-path rejection in
   `scripts/proof-matrix.sh`?
