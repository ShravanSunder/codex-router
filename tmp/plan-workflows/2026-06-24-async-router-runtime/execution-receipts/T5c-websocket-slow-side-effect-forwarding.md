# Execution Receipt: T5c WebSocket Slow Side-Effect Forwarding

Timestamp: 2026-06-24T16:06:06Z
Base HEAD before commit: 6140ab5668b3cbac12fe79cc301eaf0e78dea558

## Scope

This checkpoint fixes and proves the WebSocket pump invariant that durable
side effects cannot gate frame forwarding. In the async WebSocket path,
previous-response affinity owner persistence is now captured from the upstream
message, the upstream message is forwarded to the local client, and the owner
write is spawned afterward on the blocking pool.

The test-only blocking tunnel keeps its synchronous helper for legacy unit
coverage, but the release async tunnel no longer records affinity owners before
forwarding the frame.

## Requirements Addressed

- T5/I-17b: a slow affinity owner recorder cannot delay WebSocket frame
  forwarding.
- T5/G-12 partial: WebSocket pump-side persistence is not awaited from the
  frame-forwarding path.
- T5/G-20 partial: permanent regression coverage now includes a slow
  pump-side side-effect fixture.

## Changed Files

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/lib.rs`
- `scripts/proof-matrix.sh`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/integration/I-17b.json`

## Proof Commands

```text
scripts/proof-matrix.sh I-17b
exit: 0
result: focused async WebSocket slow-recorder integration test passed.
```

```text
cargo fmt --all -- --check && cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings
exit: 0
```

```text
cargo test --workspace -- --nocapture
exit: 0
result: 266 passed, 0 failed, 9 ignored. Installed-Codex e2e rows remain ignored in cargo test and are run through smoke scripts.
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent
exit: 0
result: 1 passed, 0 failed. Three installed Codex WebSocket clients shared one router PID and overlapped successfully.
```

## Explicit Open Gaps

- This checkpoint does not complete all T5 close-family rows. Remaining
  close/revocation/blocked-write coverage still needs final audit against the
  matrix.
- T8 five-minute three-installed-Codex soak and cleanup artifact remain open.
- Implementation review swarm and PR wrapup remain required before the goal can
  be called PR-ready.

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T5c-websocket-slow-side-effect-forwarding.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/integration/I-17b.json`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Continue implementation with remaining close-family audit and T8 soak; slow WebSocket side-effect forwarding is now proven.
