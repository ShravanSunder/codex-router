# Execution Receipt: T6b Release Runtime Structural Guardrails

Timestamp: 2026-06-24T15:59:04Z
Base HEAD before commit: 9ac7e96a88a88fff4048822f8a9594652780dd1c

## Scope

This checkpoint adds proof-matrix guardrails that fail the release runtime if
the `codex-router serve` path regresses back to the hand-rolled blocking
transport shape.

The old blocking WebSocket tunnel, blocking local WebSocket handshake helpers,
and manual one-connection server error type are now test-only. The production
runtime keeps the async Hyper/tokio-tungstenite serving path.

## Requirements Addressed

- T6/G-01: release serve path has no production `std::net::TcpListener` or
  `std::net::TcpStream` listener/connection runtime.
- T6/G-02: release HTTP upstream path has no production `reqwest::blocking`.
- T6/G-03: release serve WebSocket path has no blocking Tungstenite
  accept/connect.
- T6/G-04: release server/upstream path has no production `httparse` parsing.
- T6/G-05: release async runtime files do not use blocking `Read` response
  bodies.
- T6/G-07: release runtime has positive Hyper and tokio-tungstenite ownership
  checks.
- T6/G-23: local WebSocket upgrade has no double-handshake escape hatch.
- T6/G-21: the guardrail set is runnable through `scripts/proof-matrix.sh`.

## Changed Files

- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `scripts/check-release-runtime-guardrails.py`
- `scripts/proof-matrix.sh`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-01.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-02.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-03.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-04.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-05.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-07.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-21.json`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-23.json`

## Proof Commands

```text
cargo fmt --all -- --check && cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings
exit: 0
```

```text
cargo test --workspace -- --nocapture
exit: 0
result: 265 passed, 0 failed, 9 ignored. Installed-Codex e2e rows are intentionally ignored in cargo test and run through the smoke script.
```

```text
tests/smoke/installed_codex_mock.sh --transport all
exit: 0
result: 6 passed, 0 failed.
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent
exit: 0
result: 1 passed, 0 failed. Three installed Codex WebSocket clients shared one router PID and overlapped successfully.
```

```text
set -e
for row in G-01 G-02 G-03 G-04 G-05 G-07 G-23 G-21; do
  scripts/proof-matrix.sh "$row"
done
exit: 0
result: all listed release-runtime guardrails passed and wrote fresh JSON receipts.
```

## Explicit Open Gaps

- This is a T6 structural checkpoint, not final PR readiness.
- Remaining T5 rows for revocation/close-family/slow-sink proof still need
  completion or fresh audit.
- T8 five-minute three-installed-Codex soak and cleanup artifact are still
  required before PR-ready.
- Implementation review swarm and PR wrapup are still required before the goal
  can be called complete.

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T6b-release-runtime-structural-guardrails.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/structural/G-21.json`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Continue implementation with remaining T5/T8 proof and then implementation review; release-runtime structural guardrails are now proven.
