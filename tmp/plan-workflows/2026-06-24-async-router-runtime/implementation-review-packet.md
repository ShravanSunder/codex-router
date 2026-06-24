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
- Final implementation re-review and PR wrapup are not complete.

## Git Scope

Review commits from `7749909` through `8478fa8`, especially:
- `7749909 feat: use hyper tungstenite serve runtime`
- `9ac7e96 feat: stream http upstream with hyper`
- `6140ab5 test: add release runtime guardrails`
- `44ef5ed fix: keep websocket side effects off forwarding path`
- `97a5c8c test: add installed codex websocket soak`
- `188ff43 test: prove websocket registry drains`
- `e62e50f fix: keep websocket sessions alive through continuations`
- `e2f959b test: align websocket lifecycle tests with continuations`
- `3e9ef44 fix: harden websocket proof gates`
- `46da5b6 test: record hardened websocket proof evidence`
- `9e02458 test: allow evidence-only proof commits`
- `16c8a1f test: refresh async runtime proof evidence`
- `fb4fb13 test: add non-mutating proof verification`
- `f8d3ec5 fix: close websocket review proof gaps`
- `ee32a2d test: stabilize websocket overlap proof timing`
- `8478fa8 fix: guard websocket proof against dirty sources`

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

- `cargo fmt --all -- --check`
  exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings`
  exit 0.
- `tests/smoke/installed_codex_mock.sh --transport all`
  exit 0, 6 passed.
- `tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent`
  exit 0, 1 passed.
- `tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak`
  exit 0, 1 passed, 303.68s.
- `cargo test --workspace -- --nocapture`
  exit 0 after `8478fa8`; 270 passed, 0 failed, 10 ignored.
- `cargo test -p codex-router-proxy -- --nocapture`
  exit 0 after `8478fa8`; 111 passed, 0 failed.
- `scripts/proof-matrix.sh` rows E-02/E-03/E-04/E-05/E-06/E-08
  exit 0.
- `scripts/proof-matrix.sh` rows G-01/G-02/G-03/G-04/G-05/G-07/G-21/G-23/I-17b
  exit 0.

Latest implementation-review finding fixes after `cbe736a`:
- `cargo fmt --all -- --check`
  exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings`
  exit 0.
- `cargo test -p codex-router-proxy -- --nocapture`
  exit 0, 113 passed.
- `cargo test --workspace -- --nocapture`
  exit 0, 270 passed, 0 failed, 10 ignored.
- `tests/smoke/installed_codex_mock.sh --transport all`
  exit 0, 6 passed.
- `scripts/proof-matrix.sh I-19`
  exit 0; pump cleanup/shutdown row now has a permanent harness.
- `scripts/proof-matrix.sh I-20`
  exit 0; exact first-frame forwarding row now has a permanent harness.
- `scripts/proof-matrix.sh I-21`
  exit 0; release HTTP/SSE request prep uses async state/selector contracts.
- `scripts/proof-matrix.sh` rows G-01/G-02/G-03/G-04/G-05/G-07/G-21/G-23
  exit 0 with guardrails scanning the release `codex-router-proxy/src/*.rs`
  surface after stripping `#[cfg(test)]` items.

Fresh post-review long-run proof:
- `tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak`
  exit 0 at `c60fb47d2f383444b9060ef7e955343cc1ea19d3`; 1 passed; 303.81s.
- Fresh artifact:
  `tmp/smoke/installed-codex-three-websocket-84866-1782332221488.json`.
- `scripts/proof-matrix.sh` rows E-02/E-03/E-04/E-05/E-06/E-08
  exit 0 against that artifact.

Five-minute soak artifact:
- `tmp/smoke/installed-codex-three-websocket-84866-1782332221488.json`
- git_head=c60fb47d2f383444b9060ef7e955343cc1ea19d3.
- clients.all_success=true, count=3.
- upstream.active_high_water=3, completed_sessions=3,
  final_active_sessions=0, real_overlap_duration_ms=301022,
  in_overlap_session_event_counts=[13,13,11],
  normal_close_sessions=3, abnormal_close_sessions=0,
  session_close_outcomes=[normal,normal,normal].
- upstream.multi_step_interleave_completed=true,
  multi_step_followup_frame_count=1,
  multi_step_followup_active_session_count=3,
  multi_step_completed_before_overlap_end=true.
- router_websocket_registry.active_sessions=0, high_water_sessions=3,
  registered_sessions=3, closed_sessions=3,
  completed_response_sessions=7,
  forwarded_upstream_messages=51,
  final_session_forwarded_upstream_message_counts=[16,19,16],
  handled_connections=3.
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
