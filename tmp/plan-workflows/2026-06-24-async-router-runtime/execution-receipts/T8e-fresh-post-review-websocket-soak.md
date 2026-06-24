# T8e Fresh Post-Review WebSocket Soak

timestamp_utc: 2026-06-24T20:21:00Z
git_head: c60fb47d2f383444b9060ef7e955343cc1ea19d3

## Command

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak
```

Result:

```text
test installed_codex::tests::three_codex_websocket_soak_holds_overlap_and_records_activity ... ok
test result: ok. 1 passed; 0 failed; finished in 303.81s
```

Artifact:

```text
tmp/smoke/installed-codex-three-websocket-84866-1782332221488.json
```

## Observed Proof

- clients: count=3, all_success=true.
- upstream: active_high_water=3, final_active_sessions=0.
- overlap: hold_duration_ms=300000, real_overlap_duration_ms=301022.
- per-session overlap events: [13, 13, 11].
- multi-step interleave: completed=true, followup_frame_count=1,
  followup_active_session_count=3,
  completed_before_overlap_end=true.
- close behavior: normal_close_sessions=3, abnormal_close_sessions=0,
  session_close_outcomes=[normal, normal, normal].
- router registry: active_sessions=0, high_water_sessions=3,
  registered_sessions=3, closed_sessions=3, completed_response_sessions=7,
  forwarded_upstream_messages=51,
  final_session_forwarded_upstream_message_counts=[16, 19, 16],
  handled_connections=3.
- socket cleanup: established_count=0, close_wait_count=0,
  raw_state_counts=[].

## Matrix Rows Refreshed

```text
scripts/proof-matrix.sh E-02
scripts/proof-matrix.sh E-03
scripts/proof-matrix.sh E-04
scripts/proof-matrix.sh E-05
scripts/proof-matrix.sh E-06
scripts/proof-matrix.sh E-08
```

All six rows passed against the fresh artifact.

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T8e-fresh-post-review-websocket-soak.md`, `tmp/smoke/installed-codex-three-websocket-84866-1782332221488.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-02.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-03.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-04.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-05.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-06.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/e2e/E-08.json`
recommended_next_workflow: shravan-dev-workflow:implementation-review-swarm
recommended_transition_reason: The post-review implementation fixes now have fresh long-run installed-Codex WebSocket proof and should receive final implementation review before PR wrapup.
