# Plan 1B: Quota Runtime, Selection, Status, And Smoke

Date: 2026-06-22
Parent: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
Depends on: Plan 1A credential/state substrate
Status: executable child plan after Plan 1A gate; revised after plan-review `needs_revision`

## Goal

Implement the user-visible quota runtime behavior after the credential/state substrate is safe: nonblocking startup, immediate background refresh, scheduled refresh, transient failure preservation, next-normal account switching, weekly-aware selection, and SQLite-only quota status UX.

This plan also owns same-turn and previous-response affinity proof, local
bearer-token lifecycle receipts, route support/fail-closed protocol proof, and
installed-Codex smoke expansion. It still does not implement Plan 2
OAuth/device-code/keyring onboarding.

## Non-Goals

- [ ] Do not implement `account login`.
- [ ] Do not alter credential resolver semantics except through Plan 1A-owned interfaces.
- [ ] Do not add mid-stream account switching.
- [ ] Do not add retry policy for 5xx, overload, timeout, DNS failure, reset, cancellation, or post-commit stream failure.
- [ ] Do not run live OAuth/quota proof without explicit approval.
- [ ] Do not defer WebSocket proof. WebSocket remains in Plan 1B scope unless a reviewed replan explicitly changes v1 scope.

## Child Proof Contract

- [ ] Every task block contains actions, red/green expectations for behavior changes, and proof checkboxes.
- [ ] Every executable requirement appears in the proof matrix with proof owner,
      exact preflight list command, exact execution command, expected
      observation, and stale-proof guard.
- [ ] No executable row uses vague substitute wording, broad prefix filters, or
      wrapper-only smoke references.
- [ ] Every spec-required but out-of-scope item appears in a deferred or gated-live table.
- [ ] Live proof uses the exact receipt `not-run: approval required` when approval is absent.
- [ ] Smoke proof names each exact `installed_codex_*` scenario individually.
- [ ] Final closeout reports command, exit code, pass/fail count where available, skipped/not-run reason, stale-proof guard result, and red/green result.

## Preconditions

- [ ] Plan 1A validation gates passed.
- [ ] Plan 1A implementation-review-swarm completed with no unresolved blockers.
- [ ] Plan 1A completion receipt commit exists before any Plan 1B checkpoint
      commit, even in a single PR stack.
- [ ] Unified credential resolver exists for quota refresh, HTTP/SSE, and WebSocket egress.
- [ ] Durable per-window selector source is chosen and available.
- [ ] Current repo state is recorded before Plan 1B starts.
- [ ] Dirty-tree isolation receipt from Plan 1A is still valid or refreshed for
      Plan 1B write surfaces.

## Write Surfaces

- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-quota/src/*`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-selection/src/*`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-test-support/src/*`
- `tests/smoke/installed_codex_mock.sh`
- `README.md`
- `docs/testing/live-oauth-quota.md`

Closed unless task-local amendment is approved:

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-state/Cargo.toml`

## Task-Local Write Ownership

Default execution is serial.

- T6 owns quota refresh failure taxonomy and response-backed alias fan-out.
- T7 owns serve startup/background/manual refresh convergence and
  cross-process one-writer behavior.
- T8 owns selector scoring and durable selector projection consumption.
- T9 owns next-normal account switching, HTTP/SSE affinity, WebSocket
  first-frame affinity, route support proof, and local bearer lifecycle receipt.
- T10 owns status rendering/math from SQLite only.
- T11 owns docs/runbook/help alignment after T10 behavior is final.
- T12 owns exact installed-smoke test expansion and final validation receipts.

## Execution Checklist

### Gate 0. Re-Verify Plan 1A Boundary

- [ ] Confirm Plan 1A is complete with validation and implementation-review
      evidence. Do not use a user-approved exception to start Plan 1B early.
- [ ] Record current `git status --short`.
- [ ] Prefer a fresh execution worktree from the Plan 1A receipt commit.
- [ ] If executing in this worktree, refresh the dirty-path manifest and save
      hunk fingerprints for every dirty path overlapping Plan 1B write surfaces.
- [ ] Confirm no Plan 2 OAuth/login work enters this child plan.

### T6. Failure Taxonomy Before Immediate Refresh

Actions:

- [ ] Define transient classes: provider timeout, network error, temporary 5xx, malformed/unusable provider body when previous valid state exists, concurrent refresh ambiguity.
- [ ] Define terminal classes: missing secret material, disabled account, unrefreshable expired credentials, provider-confirmed account/quota/auth exhaustion, provider-confirmed permanent auth denial.
- [ ] Preserve selector snapshot/headroom/reset on transient failure.
- [ ] Update stale/failed diagnostics on transient failure.
- [ ] Make only affected account/route bands ineligible on terminal failure.
- [ ] Keep response alias fan-out consistent for `responses`, `models`, `memories_trace_summarize`, and `responses_compact`.
- [ ] Keep `code_review` as status/quota state only unless a future spec change
      promotes it to routed selector input.

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
- [ ] Define a cross-process per-account quota-refresh one-writer rule, using
      a persisted cycle-generation fence or SQLite lease rather than only an
      in-memory mutex.
- [ ] Make manual `quota refresh`, startup-immediate refresh, and scheduled
      refresh converge on the same service path and one-writer rule.
- [ ] Use a whole-account refresh cycle as the visibility unit: a losing or
      stale cycle must not overwrite any selector/status rows from the winning
      cycle afterward.

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
- [ ] Manual/background overlap test proves only one cycle's selector/status
      view is visible.

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

### T9. Next-Normal-Path Account Switching And Affinity

Actions:

- [ ] On request N+1, exclude terminally ineligible accounts for the requested route band.
- [ ] Select another eligible account using shared process-lifetime selector state.
- [ ] Do not retry, rewrite, or switch the account of an in-flight stream.
- [ ] Keep route-band classification consistent between HTTP/SSE and WebSocket paths.
- [ ] Resolve `x-codex-turn-state` after local auth and before weighted quota
      selection on HTTP/SSE.
- [ ] Decode the router-owned turn-state envelope, use the router account pin
      locally, and forward only the upstream token/value upstream when needed.
- [ ] Resolve `previous_response_id` ownership before weighted quota selection.
- [ ] On disabled/unauthenticated owner, fail clearly before selecting a
      different account.
- [ ] Extract bounded affinity metadata from the first WebSocket
      `response.create` frame before upstream open.
- [ ] Persist previous-response ownership only after successful response commit.
- [ ] Preserve local bearer-token lifecycle proof: old-token HTTP rejection
      before account selection, missing/old-token WebSocket rejection before
      upstream open, and rotation closing old-generation WebSockets with a
      redacted local close reason.

Proof:

- [ ] HTTP/SSE sequence: A selected while eligible, A terminally ineligible for route band X, next request for X selects B.
- [ ] Unaffected route band can still use A when eligible there.
- [ ] Existing WebSocket connection stays pinned to A.
- [ ] Next WebSocket connection selects B after A becomes ineligible.
- [ ] Shared selector state survives separate connections.
- [ ] Same-turn and previous-response HTTP/SSE continuations stay on owner.
- [ ] Invalid/replayed turn-state envelopes fail locally.
- [ ] WebSocket continuation metadata routes to owner before upstream open.
- [ ] Existing local-token lifecycle tests are attached as receipts or rerun
      with exact commands.

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
- [ ] State that Plan 1 is not onboarding-complete without reviewed Plan 2
      OAuth/device-code/keyring work.
- [ ] Keep live OAuth/quota proof marked approval-gated unless explicitly run.
- [ ] Fix command examples to match current CLI: `--auth-json`, not `--path`, unless a future rename is explicitly planned.

Proof:

- [ ] Docs match `--help` output.
- [ ] No docs/runbook claims live proof that was not run.
- [ ] No docs present plaintext file secrets as normal steady-state onboarding.
- [ ] Docs do not claim `account login`, `account logout`, `account remove`, or
      OS keyring/Keychain storage exists before Plan 2.

Checkpoint:

- [ ] `docs: align quota runtime and account command guidance`

### T12. Validation And Smoke Closeout

Required local gates:

- [ ] `cargo fmt --all --check`
- [ ] Exact proof-row preflights listed below.
- [ ] `cargo nextest run -p codex-router-auth`
- [ ] `cargo nextest run -p codex-router-secret-store`
- [ ] `cargo nextest run -p codex-router-state`
- [ ] `cargo nextest run -p codex-router-selection`
- [ ] Matrix exact commands below, then relevant package/workspace gates.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo nextest run --workspace`
- [ ] `cargo deny check`
- [ ] `cargo audit`
- [ ] `tests/smoke/installed_codex_mock.sh`
- [ ] `git diff --check`

Named smoke cases:

- [ ] `installed_codex_router_profile_against_mock_upstream`
- [ ] `installed_codex_runtime_command_uses_codex_router_profile`
- [ ] `installed_codex_startup_not_quota_blocked_when_quota_delayed`
- [ ] `installed_codex_quota_status_redacted_after_background_refresh`
- [ ] `installed_codex_http_sse_strips_local_token_and_injects_upstream_auth`
- [ ] `installed_codex_websocket_first_frame_routing_pins_connection`

Smoke harness requirement:

- [ ] Replace broad prefix-only smoke dispatch with an explicit scenario list.
- [ ] Preflight each ignored smoke test with
      `cargo test -p codex-router-test-support <scenario> -- --ignored --exact --list`.
- [ ] Run each smoke scenario individually and print scenario name plus count.
- [ ] Fail if an expected scenario is missing.

Gated live proof:

- [ ] Not run unless explicitly approved.
- [ ] If not run, record `not-run: approval required`.
- [ ] If run, redact account labels, tokens, bodies, prompts, memory traces, and tool arguments.

Checkpoint:

- [ ] `test: prove quota runtime and status behavior`

## Plan 1B Proof Matrix

Each row must run its preflight before its execution command. The stale-proof
guard fails if the preflight returns zero matches, more than one named match, or
does not list the exact expected test. Proof owner is task plus crate/module,
not a person.

| Done | ID | Requirement | Source | Task | Proof owner | Layer | Fixture/mock | Preflight list command | Execution command | Expected observation | Stale-proof guard | Red/green |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [ ] | 1B-01 | Transient failure preserves selector snapshot | spec Account/Quota | T6 | T6 / `codex-router-cli::quota` | integration | preseeded DB + failing provider | `cargo test -p codex-router-cli quota_refresh_transient_failure_preserves_previous_snapshot_and_marks_status_stale -- --exact --list` | `cargo nextest run -p codex-router-cli quota_refresh_transient_failure_preserves_previous_snapshot_and_marks_status_stale` | prior headroom/reset remains; status stale/failed redacted | new exact test listed once; asserts no failed-zero overwrite | yes |
| [ ] | 1B-02 | Terminal failure scopes ineligibility to affected account and route band | spec Account/Quota | T6 | T6 / quota failure taxonomy | integration | two accounts, terminal failure for A | `cargo test -p codex-router-cli quota_refresh_terminal_failure_scopes_ineligibility_to_affected_account_and_route_band -- --exact --list` | `cargo nextest run -p codex-router-cli quota_refresh_terminal_failure_scopes_ineligibility_to_affected_account_and_route_band` | A route band ineligible; B unaffected; response aliases fan out together; `code_review` handled as status-only | new exact test listed once; response aliases asserted together | yes |
| [ ] | 1B-03 | Startup does not block on broad quota refresh | spec Account/Quota, Smoke | T7 | T7 / `codex-router-cli::serve` runtime | integration/smoke | delayed quota mock | `cargo test -p codex-router-cli serve_command_triggers_immediate_background_refresh_after_bind_with_nonzero_interval -- --exact --list` | `cargo nextest run -p codex-router-cli serve_command_triggers_immediate_background_refresh_after_bind_with_nonzero_interval` | listener ready before quota endpoint responds; first refresh starts after bind | new exact test listed once; assert ordering, not elapsed sleep only | yes |
| [ ] | 1B-04 | Last-known quota snapshot is usable at startup | spec Account/Quota | T7/T8 | T7 / serve startup selector state | integration | preseeded SQLite | `cargo test -p codex-router-cli serve_command_routes_from_preseeded_snapshot_before_refresh -- --exact --list` | `cargo nextest run -p codex-router-cli serve_command_routes_from_preseeded_snapshot_before_refresh` | request routes from preseeded snapshot before refresh completes | new exact test listed once; fixture DB recreated in test | yes |
| [ ] | 1B-05 | Scheduled refresh continues after immediate cycle | spec Account/Quota | T7 | T7 / refresh worker | integration | controllable clock or bounded interval | `cargo test -p codex-router-cli serve_command_runs_second_background_refresh_cycle_on_schedule -- --exact --list` | `cargo nextest run -p codex-router-cli serve_command_runs_second_background_refresh_cycle_on_schedule` | second refresh cycle observed after immediate cycle | new exact test listed once; no unbounded sleep | yes |
| [ ] | 1B-06 | Worker shutdown is bounded and redacted | spec Security/Proof | T7 | T7 / refresh worker shutdown | integration | sleeping/in-flight worker | `cargo test -p codex-router-cli serve_command_shutdown_joins_sleeping_refresh_worker_within_timeout -- --exact --list` | `cargo nextest run -p codex-router-cli serve_command_shutdown_joins_sleeping_refresh_worker_within_timeout` | stop/join completes in timeout; stderr redacted | new exact test listed once; token canary included | yes |
| [ ] | 1B-07 | Overlapping manual/background refresh has one visible writer | spec Account/Quota/Security | T7 | T7 / quota refresh one-writer | integration | barrier-controlled manual and background refresh | `cargo test -p codex-router-cli quota_refresh_overlapping_manual_and_background_cycles_publish_one_winning_generation -- --exact --list` | `cargo nextest run -p codex-router-cli quota_refresh_overlapping_manual_and_background_cycles_publish_one_winning_generation` | only one per-account cycle is visible across selector/status rows; losing cycle cannot overwrite winner | new exact test listed once; deterministic ordering and fixed clock | yes |
| [ ] | 1B-08 | Unknown/no-snapshot is not free capacity while known healthy account exists | spec Account/Quota | T8 | T8 / selector eligibility | unit/integration | two accounts, one fresh, one unknown | `cargo test -p codex-router-selection eligibility_penalizes_unknown_or_stale_when_fresh_accounts_exist -- --exact --list` | `cargo nextest run -p codex-router-selection eligibility_penalizes_unknown_or_stale_when_fresh_accounts_exist` | unknown account is not selected while known healthy account exists | existing exact test listed once; fixed clock and deterministic state | yes |
| [ ] | 1B-09 | Weekly pressure beats short-reset urgency | spec Account/Quota | T8 | T8 / selector scoring | unit/integration | mixed 5h + weekly windows | `cargo test -p codex-router-selection weekly_quota_pressure_outweighs_short_reset_urgency -- --exact --list` | `cargo nextest run -p codex-router-selection weekly_quota_pressure_outweighs_short_reset_urgency` | low-weekly account is not preferred only due short reset | new exact test listed once; fixed windows and clock | yes |
| [ ] | 1B-10 | Repository-backed selector consumes durable windows | spec Account/Quota | T8 | T8 / proxy selector projection | integration | state rows with short + weekly windows | `cargo test -p codex-router-proxy repository_backed_selector_prefers_known_healthy_account_over_unknown_snapshot -- --exact --list` | `cargo nextest run -p codex-router-proxy repository_backed_selector_prefers_known_healthy_account_over_unknown_snapshot` | selector uses durable state and avoids unknown when healthy account exists | new exact test listed once; state fixture recreated | yes |
| [ ] | 1B-11 | Next HTTP/SSE request switches to another eligible account | spec Account/Quota/Protocol | T9 | T9 / `codex-router-proxy::http_sse` | protocol | two accounts + terminal ineligible A | `cargo test -p codex-router-proxy http_proxy_rotates_to_next_eligible_account_after_terminal_ineligibility -- --exact --list` | `cargo nextest run -p codex-router-proxy http_proxy_rotates_to_next_eligible_account_after_terminal_ineligibility` | next request selects B; no inline broad refresh; unaffected route bands stay scoped | new exact test listed once; route band explicit | yes |
| [ ] | 1B-12 | WebSocket existing connection stays pinned; next connection can switch | spec Routing/WebSocket | T9 | T9 / `codex-router-proxy::websocket` | protocol | two WS connections | `cargo test -p codex-router-proxy websocket_connection_stays_pinned_while_next_connection_reselects_after_ineligibility -- --exact --list` | `cargo nextest run -p codex-router-proxy websocket_connection_stays_pinned_while_next_connection_reselects_after_ineligibility` | old WS stays A; next WS selects B after A becomes ineligible | new exact test listed once; frame preservation asserted | yes |
| [ ] | 1B-13 | Turn-state envelope pins same-turn HTTP/SSE continuation | spec Routing Granularity | T9 | T9 / selection affinity | protocol | two accounts + signed envelope | `cargo test -p codex-router-proxy http_proxy_turn_state_envelope_pins_same_turn_to_owner_account -- --exact --list` | `cargo nextest run -p codex-router-proxy http_proxy_turn_state_envelope_pins_same_turn_to_owner_account` | continuation uses owning account; invalid/replayed envelope fails locally | new exact test listed once; no silent fallback to other account | yes |
| [ ] | 1B-14 | Previous-response affinity prefers owner or fails clearly | spec Routing Granularity | T9 | T9 / affinity repository | protocol | previous_response_id ownership fixture | `cargo test -p codex-router-proxy http_proxy_previous_response_id_prefers_owner_or_fails_clearly -- --exact --list` | `cargo nextest run -p codex-router-proxy http_proxy_previous_response_id_prefers_owner_or_fails_clearly` | owner is selected; disabled/unauthenticated owner fails before different account selection | new exact test listed once; account switch on continuation forbidden | yes |
| [ ] | 1B-15 | WebSocket first-frame affinity routes before upstream open | spec Routing/WebSocket | T9 | T9 / WebSocket first-frame routing | protocol | continuation metadata in first response.create | `cargo test -p codex-router-proxy websocket_first_frame_affinity_routes_to_owner_before_upstream_open -- --exact --list` | `cargo nextest run -p codex-router-proxy websocket_first_frame_affinity_routes_to_owner_before_upstream_open` | first frame routes to owner before upstream open and remains forwarded unchanged | new exact test listed once; bounded metadata only | yes |
| [ ] | 1B-16 | Local bearer token lifecycle is proven | spec Local Auth | T9/T12 | T9 / local auth + WS revocation | integration/protocol | rotated local token | `cargo test -p codex-router-proxy loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket -- --exact --list` | `cargo nextest run -p codex-router-proxy loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket` | rotation closes old-generation WS and rejects old token without upstream open | existing exact test listed once; close reason redacted | yes |
| [ ] | 1B-17 | Supported and unsupported routes are explicit | spec Supported Codex Traffic | T9 | T9 / route classifier + proxy | protocol | route fixtures | `cargo test -p codex-router-proxy route_classifier_supports_required_codex_routes_and_rejects_realtime -- --exact --list` | `cargo nextest run -p codex-router-proxy route_classifier_supports_required_codex_routes_and_rejects_realtime` | `models`, `memories_trace_summarize`, `responses_compact`, and Realtime fail-closed behavior are covered | existing exact test listed once; unsupported paths fail before selection | yes |
| [ ] | 1B-18 | Status command is SQLite-only and readable | spec Account/Quota | T10 | T10 / `codex-router-cli::quota status` | integration | persisted status rows | `cargo test -p codex-router-cli quota_status_reads_sqlite_rows_without_provider_io -- --exact --list` | `cargo nextest run -p codex-router-cli quota_status_reads_sqlite_rows_without_provider_io` | table has account, route, status, headroom, window, reset, pace, runout, notes; zero provider calls | existing exact test listed once; no provider listener required | yes |
| [ ] | 1B-19 | Expanded status keeps effective row visible first | spec Account/Quota | T10 | T10 / status renderer | integration | multi-window rows | `cargo test -p codex-router-cli quota_status_all_limits_keeps_effective_row_visible_first -- --exact --list` | `cargo nextest run -p codex-router-cli quota_status_all_limits_keeps_effective_row_visible_first` | effective row remains visible with every provider window in deterministic order | new exact test listed once; deterministic ordering | yes |
| [ ] | 1B-20 | Pace/runout math matches fixed-window expectations | spec Account/Quota | T10 | T10 / status math | unit | fixed clock/window rows | `cargo test -p codex-router-cli quota_status_formats_pace_and_projected_runout_from_fixed_windows -- --exact --list` | `cargo nextest run -p codex-router-cli quota_status_formats_pace_and_projected_runout_from_fixed_windows` | expected-vs-actual pace and burn-rate runout match fixed fixture | new exact test listed once; fixed now, elapsed, used, remaining | yes |
| [ ] | 1B-21 | Status redacts failure notes | spec Account/Quota/Security | T10 | T10 / status renderer | integration | token/account canaries | `cargo test -p codex-router-cli quota_status_redacts_failure_notes_without_token_or_account_leak -- --exact --list` | `cargo nextest run -p codex-router-cli quota_status_redacts_failure_notes_without_token_or_account_leak` | output omits token, raw account email, and secret-bearing diagnostics | new exact test listed once; unique canaries | yes |
| [ ] | 1B-22 | Docs and help state current command truth | spec Activation/Secret Storage | T11 | T11 / docs + CLI help | docs/manual | `--help` output | `cargo run -p codex-router-cli -- --help` | `cargo run -p codex-router-cli -- --help`; `cargo run -p codex-router-cli -- account --help`; `cargo run -p codex-router-cli -- quota --help` | docs use `--auth-json`; login/logout/remove/keyring marked Plan 2; file import not normal steady-state | compare docs against current help and command vocabulary | no |
| [ ] | 1B-23 | Installed smoke preflight enumerates exact scenarios | spec Smoke | T12 | T12 / smoke harness | smoke | installed Codex mock | `cargo test -p codex-router-test-support installed_codex_router_profile_against_mock_upstream -- --ignored --exact --list` | `tests/smoke/installed_codex_mock.sh` | smoke script prints and runs each named scenario individually | all named ignored tests list exactly once before script runs | yes |
| [ ] | 1B-24 | Installed smoke covers router profile and runtime command | spec Smoke | T12 | T12 / installed Codex mock | smoke | installed Codex mock | `cargo test -p codex-router-test-support installed_codex_runtime_command_uses_codex_router_profile -- --ignored --exact --list` | `tests/smoke/installed_codex_mock.sh` | installed Codex uses temp profile and router token without printing token | named scenario listed once and output includes scenario count | yes |
| [ ] | 1B-25 | Installed smoke proves startup/status/HTTP/WebSocket scenarios | spec Smoke | T12 | T12 / installed Codex mock | smoke | installed Codex mock | `cargo test -p codex-router-test-support installed_codex_websocket_first_frame_routing_pins_connection -- --ignored --exact --list` | `tests/smoke/installed_codex_mock.sh` | startup not quota-blocked, redacted status, HTTP token stripping/injection, and WS first-frame routing pass | every `installed_codex_*` scenario listed once and run individually | yes |
| [ ] | 1B-26 | Live OAuth/quota proof is gated | spec Gated live | T12 | T12 / live proof runbook | gated live | real accounts only with approval | `rg -n "not-run: approval required|live OAuth|live quota" docs/testing/live-oauth-quota.md` | runbook commands only after explicit approval | redacted proof or `not-run: approval required` | explicit approval captured | no unless approved |

## Review Gate

- [ ] Run `implementation-review-swarm` with quota runtime, selector, status UX, smoke, and docs lanes.
- [ ] Do not claim Plan 1B complete until all required proof rows are checked or explicitly deferred with user approval.

## Merge Gate B0: Failure-Taxonomy Receipt

Required before T7 starts:

- [ ] Matrix rows 1B-01 and 1B-02 pass or route back to planning.
- [ ] The plan confirms transient failures preserve last-known selector/status
      state and terminal failures scope ineligibility to affected account/route
      bands.
- [ ] Dirty-tree isolation receipt proves only T6-owned paths were staged.

## Merge Gate B1: Runtime/Status/Docs-Ready Receipt

Required before T12 starts:

- [ ] Matrix rows 1B-03 through 1B-22 pass or route back to planning.
- [ ] Local bearer-token lifecycle proof is attached as an existing-proof receipt
      or rerun with exact commands.
- [ ] Same-turn and previous-response affinity proof is complete.
- [ ] Status UX is SQLite-only and docs match current command truth.
- [ ] Dirty-tree isolation receipt proves only T7-T11 owned paths were staged.

## Replan Triggers

- [ ] Immediate refresh cannot be tested without unbounded sleeps.
- [ ] Smoke harness cannot enumerate required named scenarios.
- [ ] Weekly-aware selection cannot consume the Plan 1A durable selector source cleanly.
- [ ] Implementation reveals the spec is wrong or contradictory.
