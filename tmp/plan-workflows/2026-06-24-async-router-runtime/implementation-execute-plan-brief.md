# Implementation Execute Plan Brief

Date: 2026-06-25
Goal id: `2026-06-24-async-router-runtime`
Current workflow: `shravan-dev-workflow:implementation-execute-plan`

## Current Goal Scope

Resume the reviewed async-router runtime implementation plan. The active goal
remains PR-ready async pure-proxy runtime proof, not merge.

Current confirmed pushed checkpoint:

- `2f9eb73` Move async credential refresh behind auth boundary
- `81b1c2e` Record auth-state boundary guard proof

Confirmed green rows from the latest checkpoint:

- `G-29`: auth/state request-time boundary guard
- `G-21`: structural rollup includes `G-29`

## Current Execution Lane

Lane: `G-24/G-25 HTTP/SSE streaming and bounded affinity`

Allowed implementation scope:

- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/upstream.rs`
- `crates/codex-router-proxy/src/headers.rs` only if needed
- `crates/codex-router-proxy/src/lib.rs` only for focused tests
- `scripts/proof-matrix.sh` only for `G-24`/`G-25`

Expected proof:

- `cargo fmt --all -- --check`
- `cargo check -p codex-router-proxy`
- targeted Rust tests for streaming request forwarding and bounded affinity
- `scripts/proof-matrix.sh G-24`
- `scripts/proof-matrix.sh G-25`

## Separate Future Work

Future quota/runrate work is intentionally not part of the current goal. A
separate draft spec may be written under:

- `tmp/spec-workflows/2026-06-25-quota-runrate-active-load/`

That future spec should cover per-account quota verification state, persisted
quota/count/runrate history, projected burn under active load, active
reservations, TDD algorithm cases, and Codex-safe near-zero quota behavior.

## Controller Notes

- Subagent outputs are candidate evidence until the parent verifies diffs and
  proof commands.
- Do not stage unrelated untracked files.
- If G-24/G-25 require a broader body abstraction redesign than the lane can
  safely complete, report `NEEDS_CONTEXT` and split before editing further.
