# Account Selection Burn-Rate Implementation Plan

Date: 2026-06-28
Status: draft for one plan-review cycle
Goal id: 2026-06-28-quota-burn-rate-selection

## Source Coverage

Primary specs:

- `docs/specs/2026-06-28-account-selection-tdd-scenario-spec.md`
  - line count checked after review/address: 815
  - full chunks read by parent across spec/review/address: 1-190, 210-360,
    360-742, with final 742-815 expansion sanity checked after patch
- `docs/specs/2026-06-27-account-quota-burn-rate-selection.md`
  - line count checked: 616
  - parent read requirements, SQLx domains, selector contract, proof
    expectations, and post-v1 tuning sections

Current repo anchors:

- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-state/src/selection_projection.rs`
- `crates/codex-router-state/src/lib.rs`
- `crates/codex-router-proxy/src/lib.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-cli/src/lib.rs`

Review cycle budget:

- Spec review/address was capped at two cycles and is spent.
- Plan review/address is capped at one cycle.
- After one plan review/address pass, proceed to implementation unless a
  material blocker prevents meaningful progress.

## Goal

Make account selection deterministic, burn-rate aware, active-session aware, and
Codex-safe under account exhaustion. The router should maximize usable weekly
quota across configured OAuth accounts while minimizing downtime for long-running
Codex work.

## Non-Goals

- No WebSocket-vs-HTTP quota cost.
- No synthetic headroom cost.
- No smooth weighted fairness.
- No minimum-score fallback.
- No broad payload validation.
- No production router restart or kill during validation.
- No storage backend other than SQLx.
- No Codex CLI changes.

## Execution DAG

```text
gate 0: repo/source re-anchor
  |
  v
gate 1: executable fixture appendix/source for required scenarios
  |
  v
slice 1: selector executable scenario harness + strict selection math
  |
  v
slice 2: SQLx projection parity + active-session rollup proof
  |
  v
slice 3: proxy exhaustion containment and reconnect safety
  |
  v
slice 4: CLI/status explanation + installed/debug smoke proof
  |
  v
gate 5: cargo fmt, clippy, targeted crate tests, workspace proof as feasible
  |
  v
implementation-review-swarm
  |
  v
implementation-pr-wrapup
```

The plan is mostly serial because each slice feeds the next proof layer:
selector fixtures define expected behavior, SQLx must project the same input,
proxy must use that selector/projection behavior, and CLI/smoke proof must
explain the same runtime decision.

## Vertical Slice 1: Selector Scenario Harness And Math

Source anchors:

- 2026-06-28 spec R1-R5 and `Test Fixture Shape`
- 2026-06-27 spec R0-R6 and `Selector Algorithm Contract`

Behavior:

- Add a pure selector scenario harness in `codex-router-selection`.
- Every account-selection scenario includes:
  - `now_unix_seconds`
  - route band
  - full 5h and weekly windows
  - current active sessions
  - policy constants
  - projection mode or explicit per-start projection trace
  - exact selected sequence and final active sessions
  - per-account state, reason codes, and pool roles
- Remove selector dependence on:
  - `active_load_pressure`
  - `headroom_cost`
  - transport-specific pressure
  - score/weight fallback that keeps weak accounts selectable

Likely touched files:

- `crates/codex-router-selection/src/burn_down.rs`
- optional new test helper module under `crates/codex-router-selection/src/`

TDD gate:

- Pre-slice fixture gate before selector implementation:
  - expand S1, S2, S3e-S3m, and S5 to executable fixture quality
  - include full policy, `now_unix_seconds`, route band, all account windows,
    projection mode or explicit per-start projection vector,
    `selected_sequence`, final active sessions, account states, reason codes,
    and pool roles
  - include controlled-drain long-running placement boundaries at 6h, 10h,
    and 15h
- First red behavior test after the fixture gate: S4 real low-weekly case
  selects `B, B, B, C, C` with replayed active-session mutation and projection
  trace.
- Then add S1, S2, S3a-S3n, S5, and no-cost canaries as executable fixtures.

Proof:

- Unit:
  - pure selector scenario harness
  - mutating multi-start scenarios
  - same-pool active balancing
  - drain-pool before far-reset reserve
  - projection-driven reserve entry
  - controlled-drain 6h, 10h, and 15h boundaries
  - usage-limit hard block
  - no `score 1`, active pressure, headroom, or transport-cost influence
- Red/green required for the first failing scenario and any current behavior
  that is known wrong.

Split/replan trigger:

- If the selector cannot support replayable projection without changing public
  input structures substantially, split a data-shape subplan before touching
  proxy or CLI.

## Vertical Slice 2: SQLx Projection Parity

Source anchors:

- 2026-06-28 spec R2/R3 and proof expectations
- 2026-06-27 spec R5/R9 and data proof expectations

Behavior:

- Ensure SQLx projections can create the exact pure selector input shape.
- Runtime/proxy remains authoritative for live active sessions.
- SQLx remains the durable mirror/history source:
  - current active-session mirror
  - active-session events
  - active-session rollups
  - quota history by reset segment
  - route-band usage-limit state
- Projection must use active-session overrides for runtime-owned current truth.

Likely touched files:

- `crates/codex-router-state/src/selection_projection.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/lib.rs`
- migration files if schema changes are needed

TDD gate:

- Red test: SQLx fixture for S4 projects the same account sequence as the pure
  selector when active-session overrides mutate between starts.

Proof:

- Integration:
  - quota history and active-session rollups produce expected burn estimates
  - active-session events retain completed sessions after current leases are
    released
  - overlapping sessions add session-seconds
  - partial buckets are clipped to the exact quota-observation interval
  - stale-purged sessions contribute until purge time
  - re-reserved sessions remain one continuous interval unless terminally
    released, retired, or stale-purged
  - retention keeps week-long quota history and active-session rollups long
    enough for run-rate calculation, then purges deterministically
  - reset-boundary history does not create fake burn
  - zero/partial active-session history falls back without divide-by-zero
  - usage-limit state excludes accounts before selection
  - migration preserves current mirror state without synthetic backfill
  - migration/schema proof creates `active_session_events` and
    `active_session_rollups` SQLx domains without using legacy active pressure
    as selector input

Split/replan trigger:

- If schema changes are larger than the existing migration model supports, stop
  after selector proof and write a smaller SQLx migration plan.

## Vertical Slice 3: Proxy Exhaustion Containment

Source anchors:

- 2026-06-28 spec R6/S6 and proxy proof expectations
- 2026-06-27 spec R8

Behavior:

- Keep traffic pass-through except account routing/auth/quota safety.
- Preserve affinity and existing-work behavior:
  - usable previous-response affinity and same-turn continuations stay on the
    owning account
  - new work may select a different account when the affinity owner is retiring
    but not hard-blocked
  - hard-blocked affinity owners use the Codex-safe retry/reconnect path or a
    router-level safety error
- For WebSocket quota exhaustion:
  - detect only complete Responses provider error envelopes
  - mark exhausted account state
  - retire/release active reservation
  - verify an alternative account can serve
  - send `websocket_connection_limit_reached` only when an alternative exists
  - send router all-accounts-exhausted only when all accounts are exhausted
  - send router quota-state-unavailable if marking or alternative verification
    fails
  - close the old socket before forwarding more client work to the exhausted
    account
- For HTTP/SSE precommit quota exhaustion, preserve existing retry behavior and
  extend proof only where selector/state changes require it.

Likely touched files:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/provider_error.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/lib.rs`

TDD gate:

- Red test: WebSocket account exhaustion with A selected and B available emits
  reconnect signal, excludes A on reconnect, and mock upstream A receives no
  later client data frame.
- Red test: six concurrent runtime selection attempts across three accounts
  produce the expected selected timeline, final active reservations, and SQLx
  mirror state.

Proof:

- Proxy/integration:
  - C1 usable affinity continuation stays on A while new work goes B
  - C2 hard-blocked affinity owner is not reused and uses a safe retry/reconnect
    path
  - at least six concurrent selection attempts across three accounts assert
    selected timeline, final active reservations, and SQLx mirror state
  - genuine quota envelope triggers containment
  - non-error JSON containing quota words passes through
  - binary frame passes through
  - malformed JSON passes through
  - all accounts exhausted yields scrubbed router-level exhausted signal
  - state marking failure yields quota-state-unavailable
  - client-visible payload does not leak provider quota body, account labels,
    tokens, prompts, or filesystem paths

Split/replan trigger:

- If Codex reconnect semantics differ from the researched
  `websocket_connection_limit_reached` behavior during installed smoke, stop and
  return to design before inventing another signal.

## Vertical Slice 4: CLI Explanation And Smoke/E2E Proof

Source anchors:

- 2026-06-28 spec CLI and smoke/e2e proof expectations
- 2026-06-27 spec R10

Behavior:

- `codex-router quota` explains the same selected account and reason as the
  selector.
- Human output shows active sessions, burn, reset, runout, and reason codes.
- JSON output exposes stable selected account/reason/active/freshness fields.
- Output does not show fake score, active pressure cost, headroom cost, or
  transport cost.
- Installed binary proof reports version/current command path.

Likely touched files:

- `crates/codex-router-cli/src/lib.rs`
- CLI tests under the same crate
- docs/testing runbook only if existing smoke instructions need a pointer

TDD gate:

- Red test: fixture CLI output for the S4 state shows B as selected initially,
  then updated state after refresh/projection, with no score/cost fields.

Proof:

- CLI:
  - table output and JSON output match selector
  - stale/unavailable active-session mirror is labeled as not live-load exact
  - installed `codex-router quota` path reports expected behavior after install
- Smoke/e2e:
  - use debug router port/state only, not production port 8787
  - installed Codex smoke for reconnect path if live auth/quota state is
    available
  - if live auth/quota state is unavailable, record blocker and do not claim
    end-to-end readiness
- OTEL/Victoria:
  - query account selections, rejections, active sessions, quota refresh
    outcomes, and usage-limit containment using scrubbed dimensions when the
    local observability stack is available
  - if the stack is unavailable, record a stack-unavailable blocker for the
    telemetry proof row without weakening lower-layer proof
  - negative canaries prove telemetry does not contain tokens, prompts, raw
    account ids, account labels, reservation ids, provider bodies, or
    filesystem paths

Split/replan trigger:

- If installed smoke would require touching production router or real account
  state destructively, stop at lower-layer proof and report the e2e blocker.

## Requirements/Proof Matrix

```text
R1 full-matrix selector scenarios
  owning slice: 1
  proof source: codex-router-selection unit tests
  evidence source: cargo test output and fixture source
  freshness guard: run after final selector diff
  red/green: required

R2 mutating multi-start active sessions
  owning slice: 1 and 2
  proof source: pure selector scenario tests and SQLx projection tests
  evidence source: selected_sequence plus final_active_sessions assertions
  freshness guard: mutation must happen inside test loop
  red/green: required

R3 burn-rate/reset-aware selection
  owning slice: 1 and 2
  proof source: selector unit tests and SQLx run-rate integration tests
  evidence source: projection trace, burn basis-points/hour, reset/runout rows
  freshness guard: basis-point math, no display rounding in comparisons
  red/green: required

R4 active-session balancing per account
  owning slice: 1 and 2
  proof source: same-pool scenarios and SQLx active-session rollups
  evidence source: active counts, session-seconds, selected sequence
  freshness guard: current runtime overrides beat stale SQLx mirror
  red/green: required

R5 usage-limit containment
  owning slice: 3
  proof source: proxy WebSocket and HTTP/SSE tests
  evidence source: mock upstream capture, client-visible payload assertions,
    route-band state inspection
  freshness guard: no raw provider body or account labels in client output
  red/green: required

R5b affinity and existing-work behavior
  owning slice: 3
  proof source: proxy/runtime tests for C1/C2
  evidence source: selected account timeline, affinity owner state, hard-block
    exclusion, reconnect/retry behavior
  freshness guard: same diff as proxy containment changes
  red/green: required

R5c concurrent runtime selection
  owning slice: 3
  proof source: proxy/runtime integration with at least six concurrent
    selection attempts across three accounts
  evidence source: selected timeline, final active reservations, SQLx mirror
  freshness guard: debug/temp state only
  red/green: required

R6 CLI explanation matches selector
  owning slice: 4
  proof source: CLI table/JSON tests and installed command smoke
  evidence source: command output, version/path proof, JSON fields
  freshness guard: installed binary version checked before user-facing claim
  red/green: required for tests; installed smoke may be blocked by live auth

R7 no production router disruption
  owning slice: all
  proof source: commands bind debug ports/temp state only
  evidence source: command lines and runtime config
  freshness guard: do not kill/restart production router
  red/green: not applicable; operational guard

R8 SQLx active-session domain edges
  owning slice: 2
  proof source: SQLx integration tests for clipping, overlap, stale purge,
    re-reservation continuity, retention, migration, and no legacy pressure use
  evidence source: event rows, rollup rows, projected selector inputs
  freshness guard: run after schema/projection diff
  red/green: required

R9 telemetry and redaction proof
  owning slice: 4
  proof source: OTEL/Victoria query proof or explicit stack-unavailable blocker
  evidence source: scrubbed metric/log/trace query output and negative canaries
  freshness guard: local observability stack status captured in same run
  red/green: required when stack available; otherwise blocker is explicit
```

## Validation Gates

Initial targeted gates:

```text
cargo test -p codex-router-selection <scenario filters>
cargo test -p codex-router-state <projection/rollup filters>
cargo test -p codex-router-proxy <quota exhaustion filters>
cargo test -p codex-router-cli <quota output filters>
```

Quality gates:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Broader gates:

```text
cargo test -p codex-router-selection
cargo test -p codex-router-state
cargo test -p codex-router-proxy
cargo test -p codex-router-cli
cargo test --workspace
```

Smoke/e2e gates:

```text
codex-router --version
codex-router quota --format json
codex-router serve --bind 127.0.0.1:<debug-port> --state <temp-state>
codex --profile <debug-router-profile> ...  # only with debug port/state
```

The smoke command shape is finalized during implementation based on available
auth and installed profiles. It must not use the production router process or
port 8787.

## Security And Reliability Constraints

- OAuth credentials and provider bodies stay redacted.
- Account labels/ids, reservation ids, tokens, prompts, payloads, and
  filesystem paths do not enter telemetry or client-facing quota safety errors.
- Quota parsing is bounded to Responses provider error envelopes only.
- Binary frames and non-error JSON are pass-through for quota purposes.
- SQLx errors during exhaustion marking or alternative verification produce a
  router quota-state-unavailable safety response.
- Runtime controlled drain is gated on proxy reconnect-containment proof.
- Active-session rollups are additive for overlapping sessions and never
  converted into fixed transport costs.

## Plan Review Scope

Run one `plan-review-swarm` cycle only. Review should answer:

```text
1. Do the four slices preserve vertical behavior/proof ownership?
2. Does the plan avoid fake cost/score/headroom/smooth-fairness behavior?
3. Are TDD red/green gates concrete enough to start implementation?
4. Are SQLx-only and pass-through boundaries preserved?
5. Does smoke/e2e proof avoid touching production router port 8787?
```

Accepted findings from that single cycle may be patched once. Then proceed to
`implementation-execute-plan`.

## Recommended Next Workflow

`shravan-dev-workflow:plan-review-swarm`

phase_result: complete
evidence: this plan plus source specs listed above
recommended_next_workflow: shravan-dev-workflow:plan-review-swarm
recommended_transition_reason: Plan maps the reviewed specs into vertical
slices with TDD proof gates and one remaining review/address cycle.
