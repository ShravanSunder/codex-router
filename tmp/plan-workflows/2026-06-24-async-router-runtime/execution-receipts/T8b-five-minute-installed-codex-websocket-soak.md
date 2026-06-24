# Execution Receipt: T8b Five-Minute Installed-Codex WebSocket Soak

Timestamp: 2026-06-24T16:18:59Z
Base HEAD before commit: 44ef5ed844a492050d5b2c2d0aa156538f3c8e19

## Scope

This checkpoint adds and proves the long-running installed-Codex WebSocket soak
gate. The soak harness starts one real `codex-router serve` child process, runs
three installed `codex --profile codex-router exec` children against it, holds
all three upstream WebSocket sessions in the same overlap window, sends
heartbeat frames during the hold, then completes all three children.

The normal smoke path remains fast. The five-minute soak is an ignored/manual
gate exposed through:

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
```

`CODEX_ROUTER_SOAK_SECONDS` can shorten the hold for local harness debugging;
the PR-ready proof below used the default 300-second hold.

## Requirements Addressed

- T8/E-02: five-minute three-runtime WebSocket soak passed.
- T8/E-03: artifact proves one overlap window with active high-water 3.
- T8/E-04: each runtime has multiple upstream frame exchanges during the
  overlap window.
- T8/E-06 partial: upstream-side active high-water is 3 and final active count
  is 0 after completion.
- T8/E-07 partial: artifact correlates all children to one shared router PID
  and successful installed Codex outputs.
- T8/E-09 partial: redacted transcript writer and negative canaries passed for
  the soak artifact.

## Changed Files

- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`

## Proof Commands

```text
CODEX_ROUTER_SOAK_SECONDS=3 tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
exit: 0
result: quick harness shakeout passed with artifact assertions enabled.
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
exit: 0
result: 1 passed, 0 failed, finished in 302.60s.
```

Five-minute artifact summary:

```text
mode=three-websocket-soak
clients.all_success=true
shared_router_pid=42375
spawned_real_serve_child=true
active_high_water=3
final_active_sessions=0
hold_duration_ms=300000
overlap_duration_ms=300042
session_event_counts=[12, 12, 12]
session_frame_counts=[1, 1, 1]
```

```text
cargo fmt --all -- --check && cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings
exit: 0
```

```text
tests/smoke/installed_codex_mock.sh --transport all
exit: 0
result: 6 passed, 0 failed.
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent
exit: 0
result: 1 passed, 0 failed.
```

```text
cargo test --workspace -- --nocapture
exit: 0
result: 266 passed, 0 failed, 10 ignored.
```

## Explicit Open Gaps

- E-06 is still partial for router-internal registry export from the child
  process; the soak proves upstream high-water/final-active evidence.
- E-08 socket cleanup by OS socket inspection is still not separately captured;
  the soak artifact proves upstream final active 0 and successful child
  completion.
- E-05 tool-call-style interleave is not separately implemented; the soak
  proves multi-frame streaming activity but not a real tool-call transcript.
- Implementation review swarm and PR wrapup remain required before the goal can
  be called PR-ready.

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T8b-five-minute-installed-codex-websocket-soak.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Continue implementation/review with remaining router-registry export, socket-cleanup/tool-call proof decisions, implementation review, and PR wrapup; the required five-minute installed-Codex WebSocket soak is now proven.
