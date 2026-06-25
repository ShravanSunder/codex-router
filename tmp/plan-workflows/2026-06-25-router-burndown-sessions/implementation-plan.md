# Implementation Plan: Router Burndown, Quota Safety, Sessions

Date: 2026-06-25
Status: draft plan, pending plan-review-swarm
Source spec: tmp/spec-workflows/2026-06-25-router-burndown-sessions/router-burndown-quota-safety-sessions-spec.md

## Goal

Implement historical quota burndown, Codex-safe quota exhaustion, and router sessions UX without expanding router behavior beyond account routing, auth, quota safety, and pass-through compatibility.

## Tooling Decisions

- Runtime/proxy: keep Tokio + Hyper + tokio-tungstenite direction.
- State: use SQLx SQLite for new async quota history surfaces.
- CLI: add `clap`; migrate command contract deliberately rather than adding more hand parser branches.
- Sessions picker: use `inquire`.
- Tables: keep `comfy-table`.
- JSON: keep `serde_json`.
- Do not add `ratatui`, `dialoguer`, direct `crossterm`, or `indicatif` for V1 sessions unless a later measured issue justifies it.

## Execution DAG

```text
gate 0: re-anchor repo state and run baseline tests
  |
  +-- slice A: quota history schema + repositories
  |
  +-- slice B: estimator + projected burn scorer
  |
  +-- slice C: active reservation/load accounting
  |
integration gate 1: selector uses history + active load
  |
  +-- slice D: Codex-safe quota exhaustion and WS retirement proof
  |
  +-- slice E: quota status UX and live quota E2E harness
  |
  +-- slice F: sessions command with clap + inquire
  |
integration gate 2: full router smoke + deterministic E2E
  |
implementation-review-swarm
  |
PR-ready gate
```

## Slice A: Quota History Schema And Repositories

Write scope:
- `crates/codex-router-state`
- tests in state crate

Work:
- Add SQLx migration/schema for append-only quota observations.
- Preserve current latest-row tables as materialized current state.
- Add one-week retention purge.
- Add query APIs by account, route band, window seconds, and time range.
- Store reset credits with observation metadata when reported.

Proof:
- red/green SQLite integration tests for append, latest read, seven-day retention, reset-credit persistence, reset boundary records, stale/failure records.

## Slice B: Estimator And Projected Burn Scorer

Write scope:
- `crates/codex-router-selection`

Work:
- Add run-rate estimator from quota history.
- Segment history across reset boundaries.
- Produce confidence state: none, insufficient, low, normal.
- Combine current 5h/weekly quota, reset time, run-rate, and projected exhaustion.
- Keep weekly as hard limiter.
- Keep reset credits display-only unless a later reviewed requirement changes that.

Proof:
- TDD unit matrix for no history, one sample, sparse/dense samples, reset crossed, quota increase anomaly, accelerating burn, decelerating burn, 5h high weekly low, 5h low weekly high, near reset, all exhausted.

## Slice C: Active Reservation And Load Accounting

Write scope:
- `crates/codex-router-selection`
- `crates/codex-router-proxy`

Work:
- Wire reservation/load accounting into repo-backed selection.
- Reserve on selected WebSocket/request start.
- Release on response complete, close, error, cancellation/drop.
- Add stale reservation cleanup and observability.
- Separate account pinning/cooldown from active load projection.

Proof:
- unit tests for reservation lifecycle.
- proxy integration tests for concurrent sessions projecting different burn against accounts.
- leak cleanup tests using bounded virtual time where available.

## Slice D: Codex-Safe Quota Exhaustion And WebSocket Retirement

Write scope:
- `crates/codex-router-proxy`
- `crates/codex-router-auth` if error classification belongs there
- test support

Work:
- Classify upstream `websocket_connection_limit_reached` as transport reconnect, not quota exhaustion.
- Classify `usage_limit_reached`/quota failures as account exhaustion/quarantine.
- Avoid exhausted accounts before upstream open.
- Add controlled retirement for accounts projected near zero only at safe connection boundaries.
- Do not implement mid-stream account hot-swap.
- If in-flight quota error cannot be hidden safely, mark it as post-commit failure and rely on earlier projection.

Proof:
- deterministic router+fake-Codex WS tests:
  - account A exhausted before next request, reconnect selects B, Codex turn completes
  - all accounts exhausted surfaces quota error
  - connection-limit error reconnects without quota quarantine
  - usage-limit error quarantines account
  - three concurrent Codex-shaped WS clients remain isolated
- installed-Codex smoke with bounded turns and fake upstream, asserting no sticky HTTP fallback for controlled retirement.

## Slice E: Quota Status UX And Live Quota E2E

Write scope:
- `crates/codex-router-cli/src/quota.rs`
- docs/testing
- test support

Work:
- Update quota status to show one compact account row with multiline 5h/weekly cells.
- Show bars, percentages, reset times, reset credits, routing decision, projected runout, and confidence.
- Make unknown/stale/blocked/all-out explicit.
- Add deterministic E2E fixture for Codex-account-shaped rate-limit events.
- Add live-gated command for real logged-in Codex account quota proof.

Proof:
- table snapshot tests.
- JSON schema/field tests.
- deterministic E2E: refresh -> DB history/current -> status -> selection.
- live-gated E2E run when credentials exist, with sanitized output only.

## Slice F: Sessions Command

Write scope:
- `crates/codex-router-cli`
- possibly a small `sessions` module/crate under CLI ownership

Work:
- Add `clap` command model.
- Add `codex-router sessions` with `scope`, `provider`, `source`, `sort`, `--list`, `--format`, `--last`.
- Read Codex `state_5.sqlite` metadata read-only.
- Resolve worktree root; outside git, fall back to cwd with note.
- Use `inquire` for interactive picker.
- Launch `codex --profile codex-router resume <SESSION_ID>` through injectable command runner.

Proof:
- CLI parse tests for defaults and invalid args.
- SQLite fixture tests for cwd/worktree/any, provider any/current/id, source interactive/all/subagents.
- table/json output tests with no transcript body.
- fake `codex` binary/runner tests for picker selection and `--last`.
- dependency guard test or review checklist confirms no disallowed TUI crates.

## Requirements / Proof Matrix

| Requirement | Owning slice | Proof layer | Evidence |
| --- | --- | --- | --- |
| Persist one-week quota history | A | integration | SQLx SQLite tests for append/query/retention |
| Compute run-rate and projected exhaustion | B | unit | estimator/scorer TDD matrix |
| Include active load in next account choice | C | unit + integration | reservation lifecycle and proxy selection tests |
| Avoid exhausted accounts before upstream open | D | integration + E2E | fake upstream quota exhaustion tests |
| Codex sees quota only when all accounts exhausted | D | E2E | router+Codex-shaped WS test and installed-Codex smoke |
| Distinguish connection limit from quota exhaustion | D | integration | websocket_connection_limit_reached test |
| Show reset data and reset credits | E | CLI + E2E | status table/json and live-gated quota proof |
| Sessions picker defaults and filters | F | CLI + integration | state DB fixture tests |
| Sessions launch path | F | integration | injected runner/fake codex command proof |

## Baseline And Final Validation Commands

Baseline before implementation:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

Targeted validation during implementation:

```text
cargo test -p codex-router-selection burn
cargo test -p codex-router-selection reservation
cargo test -p codex-router-state quota
cargo test -p codex-router-proxy websocket
cargo test -p codex-router-cli quota
cargo test -p codex-router-cli sessions
```

Smoke/E2E validation:

```text
cargo run -p codex-router-cli -- quota refresh
cargo run -p codex-router-cli -- quota status --format table
cargo run -p codex-router-cli -- quota status --format json
cargo run -p codex-router-cli -- sessions --list --format table
cargo run -p codex-router-cli -- sessions --last --dry-run
```

Live-gated quota proof must be a separate documented command that refuses to print secrets and exits clearly when no logged-in Codex account is available.

## Split / Replan Triggers

- If proactive WS retirement makes Codex enter sticky HTTP fallback, stop and redesign retirement boundaries.
- If in-flight `usage_limit_reached` cannot be retried invisibly, mark it as unavoidable post-commit failure and tighten projection thresholds.
- If clap migration becomes too large, split into CLI parser migration first, then sessions command.
- If Codex state DB schema differs from observed `state_5.sqlite`, split a schema-discovery compatibility task.
- If live quota proof would consume meaningful user quota, require explicit manual run confirmation.

## Review Gate

Before implementation starts, run `spec-review-swarm` or `plan-review-swarm` on this spec and plan. The review packet must include the Codex lifecycle finding that normal usage-limit errors are terminal and that physical WebSockets can span logical turns.

