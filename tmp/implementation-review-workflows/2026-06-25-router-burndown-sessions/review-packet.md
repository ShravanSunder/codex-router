# Implementation Review Packet: Router Burndown, Quota Safety, Sessions

Date: 2026-06-25
Mode: implementation
review_class: plan-backed and risk-triggered
source_backed_verdict_attempted: true
whole-source-trace: required

## Accepted Request

Implement the follow-up goal in a new worktree:

- historical burndown quota safety using SQLx-only new SQL
- projected burn under active load and reservation-aware selection
- Codex-safe quota exhaustion / connection retirement behavior
- router-owned sessions picker/list/last command
- live quota/reset/reset-credit proof gates with no secret leakage
- no behavior beyond account routing, auth, quota safety, and Codex pass-through compatibility

Important standing constraints:

- Reviewers must use gpt-5.5, not gpt-5.4.
- New or extended SQL for this implementation must use SQLx only.
- Do not add or extend rusqlite queries, repositories, migrations, session readers, or test helpers.
- No direct ratatui, dialoguer, or crossterm for sessions V1.
- No mid-stream WebSocket hot-swap as a hidden protocol invention.

## Source Artifacts

- Spec: `tmp/spec-workflows/2026-06-25-router-burndown-sessions/router-burndown-quota-safety-sessions-spec.md`
- Spec review report: `tmp/spec-workflows/2026-06-25-router-burndown-sessions/spec-review-cycle-1/review-report.md`
- Plan: `tmp/plan-workflows/2026-06-25-router-burndown-sessions/implementation-plan.md`
- Plan review report: `tmp/plan-workflows/2026-06-25-router-burndown-sessions/plan-review-cycle-1/review-report.md`
- Workflow state: `tmp/workflow-state/2026-06-25-router-burndown-sessions-implementation/details.md`
- Research ledger: `tmp/research-workflows/2026-06-25-router-burndown-sessions/research-ledger.md`

Review-cycle evidence:

- Spec line 4: `Status: reviewed once; accepted spec-review-cycle-1 findings applied`
- Plan line 4: `Status: reviewed once; accepted plan-review-cycle-1 findings applied`

## Git Scope

- Worktree: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.impl-burndown-sessions`
- Branch: `impl-burndown-sessions`
- Head: `248989462b98b53b8637ca65ff42a1a24426ea8d`
- Base: `origin/main`
- Review diff: `origin/main...HEAD`

Changed paths:

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/live.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/sessions.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/lib.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/provider_error.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-selection/src/lib.rs`
- `crates/codex-router-selection/src/reservation.rs`
- `crates/codex-router-selection/src/run_rate.rs`
- `crates/codex-router-state/src/lib.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `docs/testing/live-oauth-quota.md`
- spec/plan/research/workflow artifacts under `tmp/`

## Proof Claims

Current checkpoint proof, all run from the worktree after commit `2489894`:

- `cargo fmt --all -- --check` -> exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` -> exit 0
- `cargo nextest run --workspace` -> 322 tests passed, 10 skipped, exit 0
- `cargo build -p codex-router-cli --bin codex-router` -> exit 0
- `tests/smoke/quota_status_fixture.sh` -> exit 0
- `tests/smoke/installed_codex_mock.sh --transport all --scenario serial` -> 6 passed, exit 0
- `tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent` -> 1 passed, exit 0
- `CODEX_ROUTER_SOAK_SECONDS=10 tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak` -> 1 passed, exit 0
- Live quota gate smoke using `./target/debug/codex-router live quota --profiles-root <tmp> --dry-run` and without approval -> dry-run succeeds without secret output; no-approval path fails with approval-required message; exit 0 for smoke wrapper.

Artifacts from latest smoke:

- `tmp/smoke/installed-codex-mock-30012-1782399443016.json`
- `tmp/smoke/installed-codex-mock-30012-1782399443992.json`
- `tmp/smoke/installed-codex-mock-30012-1782399444519.json`
- `tmp/smoke/installed-codex-three-websocket-30436-1782399449115.json`
- `tmp/smoke/installed-codex-three-websocket-31069-1782399462965.json`

Known proof limitation:

- Real live account provider call with `--approve-network-account-use` was not run in this checkpoint. The no-network default and dry-run/refusal safety were proven; real-account proof remains explicit opt-in.

## Recent Checkpoint Fixes To Review Closely

- `live quota` now requires `--approve-network-account-use` or `--dry-run`.
- `live quota --dry-run` discovers profile labels without provider I/O or token printing.
- HTTP/SSE installed-Codex harness now accepts streaming proof via request line, `Accept: text/event-stream`, or top-level JSON `stream: true`, instead of only brittle body substring.
- Mock HTTP reader now decodes complete chunked bodies.
- Local auth fallback scanner avoids slicing inside UTF-8 chars.
- Three-Codex concurrent WebSocket smoke now holds mocked sessions long enough to prove overlap.
- CLI serve test retry helpers now use a wall-clock deadline instead of a tight scheduler-dependent loop.

## Non-goals / Boundaries

- Do not require real-account quota call without explicit approval.
- Do not require mid-turn WebSocket account switching.
- Do not add transcript search to sessions V1.
- Do not broaden the router into a payload-transforming proxy.
- Do not introduce rusqlite for new SQL surfaces.

## Required Reviewer Output

Each lane is read-only. Do not edit files.

Return candidate findings only, using:

- severity: blocker | important | follow-up | nit
- title
- evidence: exact file:line, symbol, command, or source artifact line
- scenario
- smallest_fix
- proof
- confidence

Also include:

- lane confidence
- remaining uncertainty
- completion receipt with source anchors inspected
