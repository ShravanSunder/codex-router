# Execution Receipt: T8c Live WebSocket Registry Proof

Timestamp: 2026-06-24T18:20:50Z
Proof HEAD: 3e9ef44e5a5ada9462a97559efb1bcd64d85e40e

## Scope

This checkpoint upgrades the five-minute installed-Codex WebSocket soak with
router-owned registry evidence from the live `codex-router serve` child process.
The registry report is written by the shared WebSocket registry when sessions
open, complete, and close. Forwarded-frame counters stay in memory on the hot
path and are flushed by completion/close updates. The soak harness waits for the
registry to drain before stopping the child process.

## Requirements Addressed

- E-02: five-minute three-runtime installed-Codex WebSocket soak passed through
  `scripts/proof-matrix.sh E-02`.
- E-03: overlap timestamps passed through `scripts/proof-matrix.sh E-03`.
- E-04: router-side successful upstream-to-local writes passed through
  `scripts/proof-matrix.sh E-04`.
- E-05: function-call-style multi-step WebSocket continuation completed during
  the three-client overlap and passed through `scripts/proof-matrix.sh E-05`.
- E-06: router-owned registry high-water/drain proof passed through
  `scripts/proof-matrix.sh E-06`.
- E-08: live router PID socket table showed no ESTABLISHED or CLOSE_WAIT TCP
  sessions after completion and passed through `scripts/proof-matrix.sh E-08`.

## Changed Files

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`
- `scripts/proof-matrix.sh`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-03.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-04.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-05.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-06.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-08.json`

## Proof Commands

```text
CODEX_ROUTER_SOAK_SECONDS=3 tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
exit: 0
result: 1 passed, 0 failed, finished in 6.17s after live registry report assertions.
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
exit: 0
result: 1 passed, 0 failed, finished in 302.69s.
```

```text
for row in E-02 E-03 E-04 E-05 E-06 E-08; do scripts/proof-matrix.sh "$row"; done
exit: 0
result: all six E rows passed and wrote row-local artifacts.
```

Five-minute artifact summary:

```text
artifact=tmp/smoke/installed-codex-three-websocket-24532-1782325226459.json
git_head=3e9ef44e5a5ada9462a97559efb1bcd64d85e40e
mode=three-websocket-soak
clients.all_success=true
shared_router_pid=<recorded in artifact>
upstream.active_high_water=3
upstream.completed_sessions=3
upstream.final_active_sessions=0
upstream.hold_duration_ms=300000
upstream.overlap_duration_ms=300026
upstream.real_overlap_duration_ms=300024
upstream.session_frame_counts=[2, 3, 2]
upstream.session_event_counts=[12, 15, 12]
upstream.multi_step_interleave_completed=true
upstream.multi_step_followup_frame_count=1
upstream.multi_step_followup_active_session_count=3
upstream.multi_step_completed_before_overlap_end=true
router_websocket_registry.active_sessions=0
router_websocket_registry.high_water_sessions=3
router_websocket_registry.registered_sessions=3
router_websocket_registry.closed_sessions=3
router_websocket_registry.completed_response_sessions=7
router_websocket_registry.forwarded_upstream_messages=45
router_websocket_registry.handled_connections=3
router_websocket_registry.completed_session_forwarded_upstream_message_counts=[2, 2, 2, 5, 14, 17, 14]
router_websocket_registry.final_session_forwarded_upstream_message_counts=[14, 17, 14]
socket_cleanup.established_count=0
socket_cleanup.close_wait_count=0
socket_cleanup.raw_state_counts=[]
```

## Open Gaps

- Final full-suite validation, implementation review swarm, and PR wrapup remain
  required before PR-ready.

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T8c-live-websocket-registry-proof.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-06.json`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Continue implementation execution for remaining E-05/E-08 proof decisions, then run implementation-review-swarm and PR wrapup.
