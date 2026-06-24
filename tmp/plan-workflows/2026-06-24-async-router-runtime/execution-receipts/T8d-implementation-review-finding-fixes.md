# T8d Implementation Review Finding Fixes

timestamp_utc: 2026-06-24T20:10:55Z
base_git_head: cbe736a787f4024d6efd938a5ea6e9c9c0629c8e

## Accepted Findings Addressed

- WebSocket forwarding no longer uses one parent task that can be pinned behind
  a single awaited send. The duplex path now splits local-to-upstream and
  upstream-to-local pumps into sibling Tokio tasks and aborts the sibling on
  completion, revocation, or serve shutdown.
- Serve shutdown no longer only stops the accept loop. When a shutdown token is
  supplied, active connection handlers are tracked, a per-session cancellation
  token is cancelled, and active WebSocket handlers are awaited before the serve
  future returns.
- Release HTTP/SSE request preparation no longer bridges through the sync
  SQLite selector in `spawn_blocking`. The serve path now uses async state open,
  async selector, and async credential resolver contracts.
- Structural guardrails now scan every release `codex-router-proxy/src/*.rs`
  source file after stripping `#[cfg(test)]` items, instead of checking only
  hand-picked files.
- Proof rows `I-19` and `I-20` are wired to permanent focused tests for pump
  cleanup/shutdown and exact first-frame forwarding.

## Proof Run

```text
cargo fmt --all -- --check
exit 0

cargo clippy --workspace --all-targets -- -D warnings
exit 0

cargo test -p codex-router-proxy -- --nocapture
exit 0; 113 passed; 0 failed

cargo test --workspace -- --nocapture
exit 0; 270 passed; 0 failed; 10 ignored

scripts/proof-matrix.sh I-19
exit 0

scripts/proof-matrix.sh I-20
exit 0

scripts/proof-matrix.sh I-21
exit 0

scripts/proof-matrix.sh G-01 && scripts/proof-matrix.sh G-02 &&
scripts/proof-matrix.sh G-03 && scripts/proof-matrix.sh G-04 &&
scripts/proof-matrix.sh G-05 && scripts/proof-matrix.sh G-07 &&
scripts/proof-matrix.sh G-23 && scripts/proof-matrix.sh G-21
exit 0

tests/smoke/installed_codex_mock.sh --transport all
exit 0; 6 passed
```

## Remaining Gate Before PR-Ready

- Rerun the five-minute installed-Codex three-WebSocket soak after this review
  fix is committed, then rerun rows `E-02`, `E-03`, `E-04`, `E-05`, `E-06`,
  and `E-08` against the new artifact. The previous soak artifact was produced
  at `8478fa8791597d8e1115e54c52e6a57f7c105ecf`; these source changes make that
  artifact stale for final PR readiness.

phase_result: needs_revision
evidence: `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T8d-implementation-review-finding-fixes.md`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/integration/I-19.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/integration/I-20.json`, `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/integration/I-21.json`
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Accepted implementation review findings are patched and locally proven; final installed-Codex soak must be refreshed after the checkpoint commit before PR wrapup.
