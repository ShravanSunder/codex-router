# Implementation Plan: Router Burndown, Quota Safety, Sessions

Date: 2026-06-25
Status: reviewed once; accepted plan-review-cycle-1 findings applied
Source spec: tmp/spec-workflows/2026-06-25-router-burndown-sessions/router-burndown-quota-safety-sessions-spec.md

## Goal

Implement historical quota burndown, Codex-safe quota exhaustion, and router sessions UX without expanding router behavior beyond account routing, auth, quota safety, and pass-through compatibility.

## Tooling Decisions

- Runtime/proxy: keep Tokio + Hyper + tokio-tungstenite direction.
- State: use SQLx SQLite for new async quota history surfaces.
- SQL boundary: all new or extended SQL access in this implementation uses SQLx only; do not add or extend rusqlite queries, repository traits, migrations, session readers, or test helpers.
- CLI: add `clap`; migrate command contract deliberately rather than adding more hand parser branches.
- Sessions picker: use `inquire`.
- Tables: keep `comfy-table`.
- JSON: keep `serde_json`.
- Do not add `ratatui`, `dialoguer`, direct `crossterm`, or `indicatif` for V1 sessions unless a later measured issue justifies it.

## Security Context

Assets:
- OAuth/account credentials and account labels.
- Router quota history and current selector state.
- Codex `state_5.sqlite` session metadata.
- WebSocket and HTTP/SSE upstream payloads.
- Local `codex` subprocess launch path.

Entry points and untrusted inputs:
- provider quota/rate-limit responses
- HTTP/SSE and WebSocket upstream control/error envelopes
- Codex session metadata rows from `state_5.sqlite`
- CLI flags and subprocess arguments

Invariants:
- no secrets, auth headers, cookies, raw prompt bodies, or transcript bodies in output, logs, JSON, tests, or live proof artifacts
- new SQL is SQLx-only
- sessions reader opens Codex state read-only
- subprocess launch uses fixed argv and no shell interpolation
- ambiguous quota-looking model text is pass-through and must not quarantine an account
- live account proof requires explicit opt-in and defaults to no-generation dry-run

Required security proof:
- prompt canary tests for sessions output/search
- read-only SQLx DB fixture tests
- diff-aware no-new-rusqlite guard
- parser ambiguity pass-through tests for HTTP/SSE and WebSocket
- sanitized live-output canary tests

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
- Include required dimensions: account id, safe account label, route band, limit window seconds, observed timestamp, remaining headroom percent, reset timestamp, window status, effective flag, refresh source, refresh success/failure status, reset credits when reported.
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
- Produce confidence state: unknown, insufficient, low, normal, stale.
- Implement deterministic thresholds from the spec: one sample insufficient; two samples low; three or more same-segment samples spanning at least fifteen minutes normal; stale samples excluded from normal projection.
- Combine current 5h/weekly quota, reset time, run-rate, and projected exhaustion.
- Preserve unknown fallback policy: unknown/no-history accounts are not healthy; known usable pool wins, unknown fallback is allowed only by explicit policy when no known usable/reserve account exists, and background probe/verify is scheduled or surfaced.
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
- Use deterministic reservation units as estimated percentage-point burn-rate contribution per active stream and route band.
- Define separate deterministic HTTP/SSE and WebSocket reservation weights and test both.
- Release on response complete, close, error, cancellation/drop.
- Add stale reservation cleanup and observability.
- Separate account pinning/cooldown from active load projection.

Proof:
- unit tests for reservation lifecycle.
- proxy integration tests for concurrent sessions projecting different burn against accounts.
- unit and proxy integration tests where two accounts have the same current quota/history, one has active stream load, selector picks the other due to earlier projected runout, and release restores projection.
- tests that history/load scoring composes with previous-response affinity ownership and does not break an existing owner unless quota safety requires it.
- leak cleanup tests using bounded virtual time where available.

## Slice D: Codex-Safe Quota Exhaustion And WebSocket Retirement

Write scope:
- `crates/codex-router-proxy`
- `crates/codex-router-auth` if error classification belongs there
- test support

Work:
- Classify upstream `websocket_connection_limit_reached` as transport reconnect, not quota exhaustion.
- Classify `usage_limit_reached`/quota failures as account exhaustion/quarantine.
- Inspect only recognized provider control/error envelopes for HTTP/SSE and WebSocket; ambiguous payload text remains pass-through and must not quarantine accounts.
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
  - ambiguous WebSocket error envelope or model payload text containing quota-looking strings is forwarded unchanged and does not quarantine
  - three concurrent Codex-shaped WS clients remain isolated
- deterministic HTTP/SSE proxy tests:
  - explicit usage/quota envelope quarantines account
  - ambiguous SSE/model text is forwarded unchanged and does not quarantine
  - normal streamed model content is not mutated
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
- Live command defaults to refresh/status/selection dry-run and refuses network/account use without explicit opt-in flag such as `--approve-network-account-use`.
- Live command exits clearly when credentials are unavailable.
- Any live WebSocket or model-generating step requires a second explicit confirmation flag such as `--approve-live-generation`.
- Live output prints only sanitized account label, route band, 5h/weekly remaining and reset, reset credits, refresh source, and selected routing outcome.

Proof:
- table snapshot tests.
- JSON schema/field tests.
- deterministic E2E: refresh -> DB history/current -> status -> selection.
- CLI tests for live proof refusal without opt-in, credential-unavailable exit, no-generation dry-run, sanitized stdout/stderr canaries.
- deterministic fixture smoke with temp router root and fake provider; real-account live-gated run only with explicit opt-in.

## Slice F: Sessions Command

Write scope:
- `crates/codex-router-cli`
- possibly a small `sessions` module/crate under CLI ownership

Work:
- Add `clap` command model.
- Add `codex-router sessions` with `scope`, `provider`, `source`, `sort`, `--list`, `--format`, `--last`.
- Read Codex `state_5.sqlite` metadata read-only.
- Resolve `provider=current` from the configured Codex provider used by `codex --profile codex-router`; fail with an actionable error when unavailable.
- Resolve worktree root; outside git, fall back to cwd with note.
- Use `inquire` for interactive picker.
- Launch `codex --profile codex-router resume <SESSION_ID>` through injectable command runner.

Proof:
- CLI parse tests for defaults and invalid args.
- SQLite fixture tests for cwd/worktree/any, provider any/current/id, source interactive/all/subagents.
- table/json output tests with no transcript body.
- prompt-canary tests prove title/preview are truncated human labels only and absent from JSON/default search.
- fake `codex` binary/runner tests for picker selection and `--last`.
- dependency guard test or review checklist confirms no disallowed TUI crates.
- SQL boundary proof confirms sessions reader uses SQLx and adds no rusqlite access.

## Requirements / Proof Matrix

| Requirement | Owning slice | Proof layer | Evidence |
| --- | --- | --- | --- |
| Persist one-week quota history | A | integration | SQLx SQLite tests for append/query/retention |
| Enforce SQLx-only SQL boundary | A/F | review + automated guard | diff/rg or boundary test proves no new/extended rusqlite access |
| Compute run-rate and projected exhaustion | B | unit | estimator/scorer TDD matrix |
| Include active load in next account choice | C | unit + integration | reservation lifecycle and proxy selection tests |
| Active load changes projected account selection | C | unit + integration | equal-history accounts choose lower projected-runout account only when active load applies, release restores |
| Preserve unknown fallback policy | B/D | unit + integration | known usable wins, unknown fallback only under policy, background probe/verify surfaced |
| Avoid exhausted accounts before upstream open | D | integration + E2E | fake upstream quota exhaustion tests |
| Codex sees quota only when all accounts exhausted | D | E2E | router+Codex-shaped WS test and installed-Codex smoke |
| Distinguish connection limit from quota exhaustion | D | integration | websocket_connection_limit_reached test |
| Ambiguous quota-looking payloads are pass-through | D | integration | HTTP/SSE and WebSocket ambiguity tests prove no quarantine/mutation |
| Show reset data and reset credits | E | CLI + E2E | status table/json and live-gated quota proof |
| Sessions picker defaults and filters | F | CLI + integration | SQLx state DB fixture tests |
| Sessions provider=current resolution | F | CLI + integration | config fixture resolves provider or returns actionable error |
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
cargo run -p codex-router-cli -- quota refresh --router-root <temp-fixture-root>
cargo run -p codex-router-cli -- quota status --router-root <temp-fixture-root> --format table
cargo run -p codex-router-cli -- quota status --router-root <temp-fixture-root> --format json
cargo run -p codex-router-cli -- sessions --list --format table
cargo run -p codex-router-cli -- sessions --last --dry-run
```

Live-gated quota proof must be a separate documented command that refuses to print secrets, requires explicit network/account opt-in, defaults to no-generation dry-run, and exits clearly when no logged-in Codex account is available.

## Split / Replan Triggers

- If proactive WS retirement makes Codex enter sticky HTTP fallback, stop and redesign retirement boundaries.
- If in-flight `usage_limit_reached` cannot be retried invisibly, mark it as unavoidable post-commit failure and tighten projection thresholds.
- If clap migration becomes too large, split into CLI parser migration first, then sessions command.
- If Codex state DB schema differs from observed `state_5.sqlite`, split a schema-discovery compatibility task.
- If live quota proof would consume meaningful user quota, require explicit manual run confirmation.

## Plan Review Cycle 1 Findings Addressed

- Added explicit live-gated opt-in, dry-run, no-credential, and sanitized-output proof.
- Added unknown quota fallback/probe policy to plan ownership.
- Added HTTP/SSE quota parser and ambiguous payload pass-through proof.
- Added deterministic HTTP and WebSocket reservation weight proof.
- Added affinity/previous-response composition proof.
- Added explicit security context and proof.
- Added provider=current resolver home and proof.
- Added quota-history dimension checklist.

## Review Gate

Spec review cycle 1 is complete in `spec-review-cycle-1/review-report.md`, and plan review cycle 1 is complete in `plan-review-cycle-1/review-report.md`. Accepted findings from both cycles have been applied. Any material change to the spec or plan after this point requires another matching review cycle before implementation continues against the changed contract.
