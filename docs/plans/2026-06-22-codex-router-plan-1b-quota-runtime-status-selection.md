# Plan 1B: Quota Runtime, Selection, Status, And Smoke

Date: 2026-06-22
Parent: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
Depends on: Plan 1A credential/state substrate
Status: executable child plan after Plan 1A gate

## Goal

Implement the user-visible quota runtime behavior after the credential/state substrate is safe: nonblocking startup, immediate background refresh, scheduled refresh, transient failure preservation, next-normal account switching, weekly-aware selection, and SQLite-only quota status UX.

## Non-Goals

- [ ] Do not implement `account login`.
- [ ] Do not alter credential resolver semantics except through Plan 1A-owned interfaces.
- [ ] Do not add mid-stream account switching.
- [ ] Do not add retry policy for 5xx, overload, timeout, DNS failure, reset, cancellation, or post-commit stream failure.
- [ ] Do not run live OAuth/quota proof without explicit approval.
- [ ] Do not defer WebSocket proof. WebSocket remains in Plan 1B scope unless a reviewed replan explicitly changes v1 scope.

## Child Proof Contract

- [ ] Every task block contains actions, red/green expectations for behavior changes, and proof checkboxes.
- [ ] Every executable requirement appears in the proof matrix with a concrete command or exact test filter.
- [ ] No executable row uses placeholder proof text such as `or equivalent`, `named test`, or wrapper-only smoke references.
- [ ] Every spec-required but out-of-scope item appears in a deferred or gated-live table.
- [ ] Live proof uses the exact receipt `not-run: approval required` when approval is absent.
- [ ] Smoke proof names each exact `installed_codex_*` scenario individually.
- [ ] Final closeout reports command, exit code, pass/fail count where available, skipped/not-run reason, stale-proof guard result, and red/green result.

## Preconditions

- [ ] Plan 1A validation gates passed.
- [ ] Plan 1A implementation-review-swarm completed with no unresolved blockers.
- [ ] Unified credential resolver exists for quota refresh, HTTP/SSE, and WebSocket egress.
- [ ] Durable per-window selector source is chosen and available.
- [ ] Current repo state is recorded before Plan 1B starts.

## Write Surfaces

- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-quota/src/*`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-selection/src/*`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-test-support/src/*`
- `tests/smoke/installed_codex_mock.sh`
- `README.md`
- `docs/testing/live-oauth-quota.md`

## Execution Checklist

### Gate 0. Re-Verify Plan 1A Boundary

- [ ] Confirm Plan 1A is complete or intentionally merged with explicit review approval.
- [ ] Record current `git status --short`.
- [ ] Confirm no Plan 2 OAuth/login work enters this child plan.

### T6. Failure Taxonomy Before Immediate Refresh

Actions:

- [ ] Define transient classes: provider timeout, network error, temporary 5xx, malformed/unusable provider body when previous valid state exists, concurrent refresh ambiguity.
- [ ] Define terminal classes: missing secret material, disabled account, unrefreshable expired credentials, provider-confirmed account/quota/auth exhaustion, provider-confirmed permanent auth denial.
- [ ] Preserve selector snapshot/headroom/reset on transient failure.
- [ ] Update stale/failed diagnostics on transient failure.
- [ ] Make only affected account/route bands ineligible on terminal failure.
- [ ] Keep response alias fan-out consistent for `responses`, `models`, `memories_trace_summarize`, and `responses_compact`.

Red/green:

- [ ] Add transient-preservation test first and watch it fail against current failed-zero behavior.
- [ ] Add terminal-scoping test first and watch it fail or prove missing alias/failure behavior.

Proof:

- [ ] Transient failure preserves prior selectable snapshot and shows stale/failed status.
- [ ] Terminal failure zeroes only affected route bands and aliases.
- [ ] Failure injection cannot leave mismatched selector snapshot and status rows.

Checkpoint:

- [ ] `fix: classify quota refresh failures`

### T7. Nonblocking Immediate + Scheduled Refresh

Dependency:

- [ ] T6 transient-preservation semantics are merged before this task.

Actions:

- [ ] Bind and report serve readiness before broad quota I/O.
- [ ] Start one background refresh cycle immediately after startup.
- [ ] Continue scheduled refreshes after the immediate cycle.
- [ ] Keep manual `quota refresh` and background refresh on the same service path.
- [ ] Ensure worker shutdown is bounded while sleeping or timing out.
- [ ] Apply `--max-snapshot-age-seconds` consistently in normal serve.

Red/green:

- [ ] Add a non-zero-interval immediate-refresh integration test and watch it fail on current code.
- [ ] Add a second-cycle scheduled-refresh test.
- [ ] Add a bounded shutdown-while-sleeping test.

Proof:

- [ ] Listener becomes ready before mock quota endpoint responds.
- [ ] Immediate refresh happens once without waiting a full interval.
- [ ] Later scheduled refresh occurs under bounded timing.
- [ ] Dropping/stopping serve exits within timeout with redacted stderr.
- [ ] Request path does not perform broad provider quota polling.

Checkpoint:

- [ ] `feat: refresh quota immediately after startup`

### T8. Weekly/Long-Window-Aware Selection

Actions:

- [ ] Compute selector score in this order: eligibility/freshness, long-window pressure, effective bottleneck headroom, reset urgency as bounded tiebreaker.
- [ ] Preserve process-lifetime weighted selector state across requests.
- [ ] Do not treat unknown/no-snapshot accounts as free capacity when known healthy accounts exist.

Red/green:

- [ ] Add a test where short reset urgency would choose the wrong account unless weekly pressure wins.
- [ ] Add a known-healthy-vs-unknown selector test before refactoring.

Proof:

- [ ] Unit tests prove weekly pressure beats short-reset urgency.
- [ ] Repository-backed selector test uses mixed short and weekly windows.
- [ ] Unknown/no-snapshot account is not selected while a known healthy account exists.

Checkpoint:

- [ ] `feat: weight selection by long-window quota pressure`

### T9. Next-Normal-Path Account Switching

Actions:

- [ ] On request N+1, exclude terminally ineligible accounts for the requested route band.
- [ ] Select another eligible account using shared process-lifetime selector state.
- [ ] Do not retry, rewrite, or switch the account of an in-flight stream.
- [ ] Keep route-band classification consistent between HTTP/SSE and WebSocket paths.

Proof:

- [ ] HTTP/SSE sequence: A selected while eligible, A terminally ineligible for route band X, next request for X selects B.
- [ ] Unaffected route band can still use A when eligible there.
- [ ] Existing WebSocket connection stays pinned to A.
- [ ] Next WebSocket connection selects B after A becomes ineligible.
- [ ] Shared selector state survives separate connections.

Checkpoint:

- [ ] `feat: switch accounts on the next eligible request`

### T10. Quota Status UX

Actions:

- [ ] Keep default table compact and effective-row-first.
- [ ] Keep expanded mode showing effective row plus every provider window.
- [ ] Use semantic window labels (`5h`, `daily`, `weekly`, `monthly`).
- [ ] Pace = actual used percent minus expected used percent at current point in window.
- [ ] Runout = projected time when current burn rate consumes remaining quota, using limiting window.
- [ ] Add notes for stale, failed, terminal, no snapshot, protected-by-weekly, and provider-unavailable cases.
- [ ] Keep `quota status` SQLite-only with zero provider I/O unless explicit refresh command is run.

Proof:

- [ ] Snapshot tests for default table.
- [ ] Snapshot tests for `--all-limits`.
- [ ] Unit tests for pace/runout/reset formatting.
- [ ] Canary redaction tests for status output.
- [ ] Provider mock sees zero requests during status.

Checkpoint:

- [ ] `feat: render quota pace and runout status`

### T11. Docs And Runbooks

Actions:

- [ ] Document current command truth: import exists, login is Plan 2, status is SQLite-only, refresh is explicit/provider-touching.
- [ ] Update runbook for immediate startup refresh.
- [ ] Update runbook for redacted status table capture.
- [ ] Label file-backed `import-codex-auth` as compatibility/dev/recovery or explicit fallback until Plan 2 keyring login exists.
- [ ] Keep live OAuth/quota proof marked approval-gated unless explicitly run.
- [ ] Fix command examples to match current CLI: `--auth-json`, not `--path`, unless a future rename is explicitly planned.

Proof:

- [ ] Docs match `--help` output.
- [ ] No docs/runbook claims live proof that was not run.
- [ ] No docs present plaintext file secrets as normal steady-state onboarding.

Checkpoint:

- [ ] `docs: align quota runtime and account command guidance`

### T12. Validation And Smoke Closeout

Required local gates:

- [ ] `cargo fmt --all --check`
- [ ] `cargo nextest run -p codex-router-auth`
- [ ] `cargo nextest run -p codex-router-secret-store`
- [ ] `cargo nextest run -p codex-router-state`
- [ ] `cargo nextest run -p codex-router-selection`
- [ ] `cargo nextest run -p codex-router-proxy repository_backed_selector`
- [ ] `cargo nextest run -p codex-router-cli account_`
- [ ] `cargo nextest run -p codex-router-cli quota_`
- [ ] `cargo nextest run -p codex-router-cli serve_`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo nextest run --workspace`
- [ ] `cargo deny check`
- [ ] `cargo audit`
- [ ] `tests/smoke/installed_codex_mock.sh`
- [ ] `git diff --check`

Named smoke cases:

- [ ] `installed_codex_router_profile_against_mock_upstream`
- [ ] `installed_codex_startup_not_quota_blocked_when_quota_delayed`
- [ ] `installed_codex_quota_status_redacted_after_background_refresh`
- [ ] `installed_codex_hostile_request_without_router_token_rejected`
- [ ] `installed_codex_http_sse_strips_local_token_injects_upstream_auth`
- [ ] `installed_codex_websocket_first_frame_routing_pins_connection`

Gated live proof:

- [ ] Not run unless explicitly approved.
- [ ] If not run, record `not-run: approval required`.
- [ ] If run, redact account labels, tokens, bodies, prompts, memory traces, and tool arguments.

Checkpoint:

- [ ] `test: prove quota runtime and status behavior`

## Plan 1B Proof Matrix

| Done | ID | Requirement | Source | Task | Layer | Fixture/mock | Command | Expected observation | Stale-proof guard | Red/green |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [ ] | 1B-01 | Startup does not block on broad quota refresh | spec Account/Quota, Smoke | T7 | integration/smoke | delayed quota mock | `cargo nextest run -p codex-router-cli serve_` and smoke scenario `installed_codex_startup_not_quota_blocked_when_quota_delayed` | listener ready before quota endpoint responds | assert ordering, not elapsed sleep only | yes |
| [ ] | 1B-02 | Last-known quota snapshot is usable at startup | spec Account/Quota | T7/T8 | integration | preseeded SQLite | `cargo nextest run -p codex-router-cli serve_` | request routes from preseeded snapshot before refresh | fixture DB recreated in test | yes |
| [ ] | 1B-03 | Unknown/no-snapshot is not free capacity while known healthy account exists | spec Account/Quota | T8 | unit/integration | two accounts, one fresh, one unknown | `cargo nextest run -p codex-router-proxy repository_backed_selector` | unknown account is not selected | fixed clock and deterministic selector state | yes |
| [ ] | 1B-04 | Transient failure preserves selector snapshot | spec Account/Quota | T6 | integration | preseeded DB + failing provider | `cargo nextest run -p codex-router-cli quota_` | prior headroom/reset remains; status stale/failed redacted | assert no failed-zero overwrite | yes |
| [ ] | 1B-05 | Terminal failure scopes ineligibility to affected account/route band | spec Account/Quota | T6/T9 | integration/protocol | two accounts, terminal failure for A | `cargo nextest run -p codex-router-cli quota_` and proxy selector test | A route band ineligible; B unaffected; aliases fan out | response aliases asserted together | yes |
| [ ] | 1B-06 | Immediate refresh runs after startup on non-zero interval | spec Account/Quota | T7 | integration | non-zero interval mock | `cargo nextest run -p codex-router-cli serve_` named immediate test | first refresh before interval elapses, after bind | no `interval=0` shortcut | yes |
| [ ] | 1B-07 | Scheduled refresh continues after immediate cycle | spec Account/Quota | T7 | integration | controllable/mock clock or bounded interval | `cargo nextest run -p codex-router-cli serve_` named scheduled test | second refresh cycle observed | no unbounded sleep | yes |
| [ ] | 1B-08 | Worker shutdown is bounded and redacted | spec Security/Proof | T7 | integration | sleeping/in-flight worker | `cargo nextest run -p codex-router-cli serve_` named shutdown test | stop/join completes in timeout; stderr redacted | token canary included | yes |
| [ ] | 1B-09 | Weekly pressure beats short-reset urgency | spec Account/Quota | T8 | unit/integration | mixed 5h + weekly windows | `cargo nextest run -p codex-router-selection` and proxy selector test | low-weekly account not preferred only due short reset | fixed windows and clock | yes |
| [ ] | 1B-10 | Next HTTP/SSE request switches to another eligible account | spec Account/Quota/Protocol | T9 | protocol | two accounts + terminal ineligible A | `cargo nextest run -p codex-router-proxy repository_backed_selector` | next request selects B; no inline broad refresh | route band explicit | yes |
| [ ] | 1B-11 | WebSocket existing connection stays pinned; next connection can switch | spec Routing/WebSocket | T9 | protocol | two WS connections | `cargo nextest run -p codex-router-proxy websocket_` | old WS stays A; next WS selects B | frame preservation asserted | yes |
| [ ] | 1B-12 | Status command is SQLite-only and readable | spec Account/Quota | T10 | integration | persisted status rows | `cargo nextest run -p codex-router-cli quota_status_` | table has account, route, status, headroom, window, reset, pace, runout, notes; zero provider calls | no provider listener required | yes |
| [ ] | 1B-13 | Expanded status shows effective plus provider windows | spec Account/Quota | T10 | integration | multi-window rows | `cargo nextest run -p codex-router-cli quota_status_` | effective row remains visible with all windows | deterministic ordering | yes |
| [ ] | 1B-14 | Pace/runout math matches research | spec Account/Quota, research ledger | T10 | unit | fixed clock/window rows | `cargo nextest run -p codex-router-cli quota_pace_runout_math` | expected-vs-actual pace; burn-rate runout | fixed now, elapsed, used, remaining | yes |
| [ ] | 1B-15 | Docs and help state current command truth | spec Activation/Secret Storage | T11 | docs/manual | `--help` output | `cargo run -p codex-router-cli -- --help` and subcommand help | docs use `--auth-json`; login marked Plan 2; file import not normal steady-state | compare against current help | no |
| [ ] | 1B-16 | Installed smoke covers exact scenarios | spec Smoke | T12 | smoke | installed Codex mock | `tests/smoke/installed_codex_mock.sh`; scenarios: `installed_codex_router_profile_against_mock_upstream`, `installed_codex_startup_not_quota_blocked_when_quota_delayed`, `installed_codex_quota_status_redacted_after_background_refresh`, `installed_codex_hostile_request_without_router_token_rejected`, `installed_codex_http_sse_strips_local_token_injects_upstream_auth`, `installed_codex_websocket_first_frame_routing_pins_connection` | startup/status/hostile/HTTP/WS cases pass | output lists each `installed_codex_*` scenario/count | yes |
| [ ] | 1B-17 | Live OAuth/quota proof is gated | spec Gated live | T12 | gated live | real accounts only with approval | runbook commands only after approval | redacted proof or `not-run: approval required` | explicit approval captured | no unless approved |

## Review Gate

- [ ] Run `implementation-review-swarm` with quota runtime, selector, status UX, smoke, and docs lanes.
- [ ] Do not claim Plan 1B complete until all required proof rows are checked or explicitly deferred with user approval.

## Replan Triggers

- [ ] Immediate refresh cannot be tested without unbounded sleeps.
- [ ] Smoke harness cannot enumerate required named scenarios.
- [ ] Weekly-aware selection cannot consume the Plan 1A durable selector source cleanly.
- [ ] Implementation reveals the spec is wrong or contradictory.
