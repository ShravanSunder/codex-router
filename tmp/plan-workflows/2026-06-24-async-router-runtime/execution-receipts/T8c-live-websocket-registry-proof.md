# Execution Receipt: T8c Live WebSocket Registry Proof

Timestamp: 2026-06-24T16:50:51Z
Base HEAD before commit: 97a5c8ca57e65860d4244264806af76e76de5f5c

## Scope

This checkpoint upgrades the five-minute installed-Codex WebSocket soak with
router-owned registry evidence from the live `codex-router serve` child process.
The registry report is written by the shared WebSocket registry when sessions
open, forward `response.completed`, and close. The soak harness waits for the
registry to drain before stopping the child process.

## Requirements Addressed

- E-02: five-minute three-runtime installed-Codex WebSocket soak passed.
- E-03: artifact proves a single overlap window across three clients.
- E-04: each runtime received repeated frames during the overlap.
- E-06: router-owned registry records high-water 3 and active 0 after
  completion.
- E-08 partial: registry-level active sessions drain before process stop, and
  upstream final active sessions are 0. Separate OS socket-table proof is not
  captured in this checkpoint.

## Changed Files

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-06.json`

## Proof Commands

```text
CODEX_ROUTER_SOAK_SECONDS=3 tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
exit: 0
result: 1 passed, 0 failed, finished in 5.78s after live registry report assertions.
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
exit: 0
result: 1 passed, 0 failed, finished in 302.57s.
```

Five-minute artifact summary:

```text
artifact=tmp/smoke/installed-codex-three-websocket-18227-1782319827822.json
mode=three-websocket-soak
clients.all_success=true
shared_router_pid=18252
upstream.active_high_water=3
upstream.completed_sessions=3
upstream.final_active_sessions=0
upstream.hold_duration_ms=300000
upstream.overlap_duration_ms=300017
router_websocket_registry.active_sessions=0
router_websocket_registry.high_water_sessions=3
router_websocket_registry.registered_sessions=6
router_websocket_registry.closed_sessions=6
router_websocket_registry.completed_response_sessions=6
```

## Open Gaps

- E-05 tool-call-style interleave remains separate from this soak proof.
- E-08 OS socket-table proof remains separate from registry/upstream cleanup
  proof.
- Final full-suite validation, implementation review swarm, and PR wrapup remain
  required before PR-ready.

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T8c-live-websocket-registry-proof.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-06.json`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Continue implementation execution for remaining E-05/E-08 proof decisions, then run implementation-review-swarm and PR wrapup.
