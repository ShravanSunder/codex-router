# Plan 1B: Quota Runtime, Selection, Status, And Smoke

Date: 2026-06-22
Parent: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
Depends on: Plan 1A credential/state substrate
Status: executable child plan after Plan 1A gate; revised after plan-review `needs_revision`
Revision status: folded accepted findings from `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-460b51e.md`

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

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-quota/Cargo.toml`
- `crates/codex-router-state/Cargo.toml`
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

- Any workspace manifest not named above.
- Plan 2 OAuth/keyring dependencies and implementation files.

## Ownership Decisions

- [ ] `codex-router-quota` owns quota refresh orchestration, failure taxonomy,
      cycle-generation fencing, one-writer lease behavior, and normalized
      selector/status publication. Plan 1B may add the manifest dependencies
      needed for that crate to depend on auth, state, secret-store/core, and
      provider clients.
- [ ] `codex-router-quota` must not read provider credential secrets directly.
      If it depends on secret-store, that dependency is limited to refresh leases
      or backend-neutral state helpers; provider auth material still flows through
      `codex-router-auth` resolver APIs.
- [ ] `codex-router-cli` owns command parsing/rendering and calls
      `codex-router-quota` service APIs for `quota refresh`, `quota status`,
      and serve-owned background refresh startup. It must not own the long-lived
      refresh state machine after T7.
- [ ] `codex-router-proxy` owns HTTP/SSE and WebSocket protocol preservation,
      local auth enforcement, route classification, affinity extraction, and
      account selection calls. It consumes durable quota state through the
      selector/state surfaces, not through CLI quota code.
- [ ] `codex-router-state` owns atomic publication APIs for selector/status
      visibility. Response-backed aliases must publish as one family unit.

## Task-Local Write Ownership

Default execution is serial.

- T6 owns quota refresh failure taxonomy and response-backed alias fan-out.
- T7 owns serve startup/background/manual refresh convergence and
  cross-process one-writer behavior in `codex-router-quota`.
- T8 owns selector scoring and durable selector projection consumption.
- T9 owns explicit precommit auth/quota rotation, next-normal account switching,
  HTTP/SSE affinity, WebSocket
  first-frame affinity, route support proof, and local bearer lifecycle receipt.
- T10 owns status rendering/math from SQLite only.
- T11 owns docs/runbook/help alignment after T10 behavior is final.
- T12 owns exact installed-smoke test expansion and final validation receipts.

Task-owned file table:

- T6: `crates/codex-router-quota/src/*`,
  `crates/codex-router-state/src/quota_snapshot.rs`,
  `crates/codex-router-state/src/repositories.rs`,
  `crates/codex-router-state/src/sqlite.rs`, and manifest edges needed by the
  quota/state publication API.
- T7: `crates/codex-router-quota/src/*`, `crates/codex-router-cli/src/lib.rs`,
  `crates/codex-router-cli/src/quota.rs`, and refresh-worker/serve startup
  manifest edges.
- T8: `crates/codex-router-selection/src/*`,
  `crates/codex-router-state/src/quota_snapshot.rs`,
  `crates/codex-router-state/src/repositories.rs`, and
  `crates/codex-router-state/src/sqlite.rs`.
- T9: `crates/codex-router-proxy/src/account_selection.rs`,
  `crates/codex-router-proxy/src/http_sse.rs`,
  `crates/codex-router-proxy/src/websocket.rs`,
  `crates/codex-router-proxy/src/server.rs`,
  `crates/codex-router-selection/src/turn_state.rs`,
  `crates/codex-router-selection/src/affinity.rs`, and local-auth route support
  files needed by those proofs.
- T10: `crates/codex-router-cli/src/quota.rs`,
  `crates/codex-router-state/src/quota_snapshot.rs`,
  `crates/codex-router-state/src/repositories.rs`, and
  `crates/codex-router-state/src/sqlite.rs`.
- T11: `README.md`, `docs/testing/live-oauth-quota.md`, and help text in
  `crates/codex-router-cli/src/lib.rs` after command behavior is final.
- T12: `tests/smoke/installed_codex_mock.sh`,
  `crates/codex-router-test-support/src/*`, and closeout-only docs/runbook
  receipts.

Each checkpoint receipt must compare `git show --name-only <checkpoint>` to this
table, attach shared-file hunk fingerprints, and explain any manifest or shared
file touched by more than one task.

## Execution Checklist

### Gate 0. Re-Verify Plan 1A Boundary

- [ ] Confirm Plan 1A is complete with validation and implementation-review
      evidence. Do not use a user-approved exception to start Plan 1B early.
- [ ] Record current `git status --short`.
- [ ] Prefer a fresh execution worktree from the Plan 1A receipt commit.
- [ ] Before editing in a fresh worktree, copy or promote the exact lifecycle
      packets:
      `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/`
      and
      `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/`.
- [ ] Record a carry-forward receipt with source path, target path, source
      commit/head, checksum or byte count, and `git status --short` before and
      after.
- [ ] Freeze required source artifacts before code edits. If
      `docs/specs/2026-06-20-codex-router-greenfield-spec.md` or
      `docs/specs/references/2026-06-20-research-evidence.md` is dirty relative
      to the execution base, either commit/promote it first or record a source
      carry-forward receipt with path, source commit/head, checksum or byte
      count, working-tree line count, execution-base line count, and normative
      flag.
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
- [ ] Publish response-backed aliases as one family visibility unit. Mutation,
      refresh success, terminal failure, and refresh-failure invalidation paths
      must use one transaction, generation fence, or equivalent recovery rule
      so no mixed-generation alias family is observable.
- [ ] Add or expose a state-owned `replace_response_family_quota_state`
      equivalent API, or a named generation-fence schema, before quota runtime
      publishes aliases. Calling an existing single-route replacement API four
      times is not acceptable.
- [ ] Keep `code_review` as status/quota state only unless a future spec change
      promotes it to routed selector input.

Red/green:

- [ ] Add transient-preservation test first and watch it fail against current failed-zero behavior.
- [ ] Add terminal-scoping test first and watch it fail or prove missing alias/failure behavior.

Proof:

- [ ] Transient failure preserves prior selectable snapshot and shows stale/failed status.
- [ ] Terminal failure zeroes only affected route bands and aliases.
- [ ] Failure injection cannot leave mismatched selector snapshot and status rows.
- [ ] Injected failure between alias writes cannot leave one response-backed
      alias stale-positive while another alias is invalidated.

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
- [ ] Define stale-owner recovery. If a process dies or times out while owning a
      refresh cycle, another process must be able to reclaim after the lease
      expiry and stale cycles must not overwrite a newer winning generation.
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
- [ ] Two-process refresh test proves owner/follower behavior and stale-owner
      recovery against one router root using two independent SQLite connections
      or processes with no shared in-memory lock.

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
- [ ] Define the turn-state envelope payload before implementation. It must bind
      at least account id, optional protocol continuation value, router/session
      scope, turn or request scope, issued/expiry time, generation or nonce, and
      route context sufficient to reject cross-turn, cross-session, wrong-key,
      stale, and replayed envelopes before selection. It must not bind provider
      access, refresh, or bearer auth.
- [ ] The T9 design packet must name the replay-state owner before code starts:
      repository/cache owner, nonce lifecycle, TTL, restart behavior,
      router-instance key behavior, and whether replay state is durable or
      process-local.
- [ ] Turn-state and affinity envelopes must never carry provider access tokens,
      refresh tokens, or bearer auth. If a protocol continuation value exists,
      call it `upstream_continuation_value`; resolver-owned provider auth is
      still injected only after affinity resolves the account/route decision.
- [ ] Decode the router-owned turn-state envelope, use the router account pin
      locally, and forward only the non-auth `upstream_continuation_value`
      upstream when needed.
- [ ] Resolve `previous_response_id` ownership before weighted quota selection.
- [ ] On disabled/unauthenticated owner, fail clearly before selecting a
      different account.
- [ ] For explicit precommit auth/quota rejection before an upstream response is
      committed, release the failed reservation, mark the account or route band
      according to failure taxonomy, and select another eligible account once.
- [ ] Do not rotate or retry for transport failures, 5xx, overload, timeout,
      DNS failure, reset, cancellation, or any post-commit stream failure.
- [ ] Extract bounded affinity metadata from the first WebSocket
      `response.create` frame before upstream open.
- [ ] Persist previous-response ownership only after successful response commit.
- [ ] Preserve local bearer-token lifecycle proof: old-token HTTP rejection
      before account selection, missing/old-token WebSocket rejection before
      upstream open, and rotation closing old-generation WebSockets with a
      redacted local close reason.
- [ ] Rerun local bearer lifecycle proof after any T9 or T12 change to
      `server.rs`, `websocket.rs`, `http_sse.rs`, local-auth code, or router
      token reload paths. Do not attach stale pre-change receipts for those
      surfaces.

Proof:

- [ ] HTTP/SSE sequence: A selected while eligible, A terminally ineligible for route band X, next request for X selects B.
- [ ] Unaffected route band can still use A when eligible there.
- [ ] Existing WebSocket connection stays pinned to A.
- [ ] Next WebSocket connection selects B after A becomes ineligible.
- [ ] Shared selector state survives separate connections.
- [ ] Same-turn and previous-response HTTP/SSE continuations stay on owner.
- [ ] Invalid/replayed turn-state envelopes fail locally.
- [ ] Cross-turn, cross-session, wrong-key, stale, and reused turn-state
      envelopes fail locally before account selection.
- [ ] Replay-state ownership receipt names the cache/repository boundary, TTL,
      restart semantics, and router-instance key behavior.
- [ ] Replay across restart/new router instance fails before account selection.
- [ ] Explicit precommit auth/quota rejection can rotate once to another eligible
      account; transport and post-commit failures do not rotate.
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
- [ ] `cargo nextest run -p codex-router-core`
- [ ] `cargo nextest run -p codex-router-cli`
- [ ] `cargo nextest run -p codex-router-proxy`
- [ ] `cargo nextest run -p codex-router-quota`
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

- [ ] `installed_codex::tests::installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket`
- [ ] `installed_codex::tests::installed_codex_mock_smoke_proves_nonblocking_quota_startup_and_redacted_status`
- [ ] `installed_codex::tests::installed_codex_mock_smoke_transcript_is_redacted_and_schema_allowlisted`
- [ ] `installed_codex::tests::installed_codex_hostile_no_token_smoke_keeps_upstream_empty`
- [ ] New split scenarios may replace the broad mock smoke only if each scenario
      has an exact ignored libtest name and one matrix row.

Smoke harness requirement:

- [ ] Replace broad prefix-only smoke dispatch with an explicit scenario list.
- [ ] Preflight each ignored smoke test with
      `cargo test -p codex-router-test-support <full::scenario> -- --ignored --exact --list`.
- [ ] Run each smoke scenario individually and print scenario name plus count.
- [ ] Fail if an expected scenario is missing.

Gated live proof:

- [ ] Not run unless explicitly approved.
- [ ] If not run, record `not-run: approval required`.
- [ ] If run, redact account labels, tokens, bodies, prompts, memory traces, and tool arguments.

Checkpoint:

- [ ] `test: prove quota runtime and status behavior`

## Plan 1B Proof Matrix

Each row must run its preflight before its execution command. Exact test rows
fail their stale-proof guard if the preflight returns zero matches, more than
one named match, or does not list the exact expected test. Structural, search,
docs, and smoke rows must state their expected match count or allowlist behavior
inside the row. Proof owner is task plus crate/module, not a person.

| Done | ID | Requirement | Source | Task | Proof owner | Layer | Fixture/mock | Preflight list command | Execution command | Expected observation | Stale-proof guard | Red/green |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [ ] | 1B-01 | Transient failure preserves selector snapshot | spec Account/Quota | T6 | T6 / `codex-router-cli::quota` | integration | preseeded DB + failing provider | `cargo test -p codex-router-cli tests::quota_refresh_transient_failure_preserves_previous_snapshot_and_marks_status_stale -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_refresh_transient_failure_preserves_previous_snapshot_and_marks_status_stale --exact` | prior headroom/reset remains; status stale/failed redacted | new exact test listed once; asserts no failed-zero overwrite | yes |
| [ ] | 1B-02 | Terminal failure scopes ineligibility to affected account and route band | spec Account/Quota | T6 | T6 / quota failure taxonomy | integration | two accounts, terminal failure for A | `cargo test -p codex-router-cli tests::quota_refresh_terminal_failure_scopes_ineligibility_to_affected_account_and_route_band -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_refresh_terminal_failure_scopes_ineligibility_to_affected_account_and_route_band --exact` | A route band ineligible; B unaffected; response aliases fan out together; `code_review` handled as status-only | new exact test listed once; response aliases asserted together | yes |
| [ ] | 1B-02a | Refresh publication is response-family atomic | plan-review-after-460b51e alias-family atomicity | T6 | T6 / `codex-router-quota` + state publication | integration | barrier/failing repository plus fixed generation clock | `cargo test -p codex-router-quota tests::quota_refresh_publication_keeps_response_alias_family_atomic_under_failure -- --exact --list` | `cargo nextest run -p codex-router-quota -- tests::quota_refresh_publication_keeps_response_alias_family_atomic_under_failure --exact` | success, transient failure, and terminal failure publish or recover `responses`, `models`, `memories_trace_summarize`, and `responses_compact` as one visible family | new exact test listed once; mixed generation readback fails the row | yes |
| [ ] | 1B-02b | State publication API is response-family atomic under SQLite failure | plan-review-after-460b51e state API atomicity | T6 | T6 / `codex-router-state` family publication | integration | real SQLite repository with injected alias failure | `cargo test -p codex-router-state tests::replace_response_family_quota_state_is_atomic_under_alias_failure -- --exact --list` | `cargo nextest run -p codex-router-state -- tests::replace_response_family_quota_state_is_atomic_under_alias_failure --exact` | state-owned family replacement or generation-fence API leaves either old family or new family visible, never a mixed family | new exact test listed once; direct four-call single-route publication fails the row | yes |
| [ ] | 1B-03 | Startup does not block on broad quota refresh | spec Account/Quota, Smoke | T7 | T7 / `codex-router-cli::serve` runtime | integration/smoke | delayed quota mock | `cargo test -p codex-router-cli tests::serve_command_triggers_immediate_background_refresh_after_bind_with_nonzero_interval -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::serve_command_triggers_immediate_background_refresh_after_bind_with_nonzero_interval --exact` | listener ready before quota endpoint responds; first refresh starts after bind | new exact test listed once; assert ordering, not elapsed sleep only | yes |
| [ ] | 1B-04 | Last-known quota snapshot is usable at startup | spec Account/Quota | T7/T8 | T7 / serve startup selector state | integration | preseeded SQLite | `cargo test -p codex-router-cli tests::serve_command_routes_from_preseeded_snapshot_before_refresh -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::serve_command_routes_from_preseeded_snapshot_before_refresh --exact` | request routes from preseeded snapshot before refresh completes | new exact test listed once; fixture DB recreated in test | yes |
| [ ] | 1B-05 | Scheduled refresh continues after immediate cycle | spec Account/Quota | T7 | T7 / refresh worker | integration | controllable clock or bounded interval | `cargo test -p codex-router-cli tests::serve_command_runs_second_background_refresh_cycle_on_schedule -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::serve_command_runs_second_background_refresh_cycle_on_schedule --exact` | second refresh cycle observed after immediate cycle | new exact test listed once; no unbounded sleep | yes |
| [ ] | 1B-06 | Worker shutdown is bounded and redacted | spec Security/Proof | T7 | T7 / refresh worker shutdown | integration | sleeping/in-flight worker | `cargo test -p codex-router-cli tests::serve_command_shutdown_joins_sleeping_refresh_worker_within_timeout -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::serve_command_shutdown_joins_sleeping_refresh_worker_within_timeout --exact` | stop/join completes in timeout; stderr redacted | new exact test listed once; token canary included | yes |
| [ ] | 1B-07 | Overlapping manual/background refresh has one visible writer | spec Account/Quota/Security | T7 | T7 / quota refresh one-writer | integration | barrier-controlled manual and background refresh | `cargo test -p codex-router-cli tests::quota_refresh_overlapping_manual_and_background_cycles_publish_one_winning_generation -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_refresh_overlapping_manual_and_background_cycles_publish_one_winning_generation --exact` | only one per-account cycle is visible across selector/status rows; losing cycle cannot overwrite winner | new exact test listed once; deterministic ordering and fixed clock | yes |
| [ ] | 1B-07a | Cross-process refresh recovers stale owner and rejects stale loser writes | spec Account/Quota/Security | T7 | T7 / `codex-router-quota` persisted one-writer | integration | two independent SQLite connections or processes, shared router root, fixed TTL, no shared in-memory lock | `cargo test -p codex-router-quota tests::quota_refresh_cross_process_lease_recovers_stale_owner_and_rejects_stale_loser -- --exact --list` | `cargo nextest run -p codex-router-quota -- tests::quota_refresh_cross_process_lease_recovers_stale_owner_and_rejects_stale_loser --exact` | follower waits or observes winner; stale owner is reclaimable after TTL; stale cycle cannot overwrite newer generation | new exact test listed once; in-memory-only lease fails this row | yes |
| [ ] | 1B-08 | Unknown/no-snapshot is not free capacity while known healthy account exists | spec Account/Quota | T8 | T8 / selector eligibility | unit/integration | two accounts, one fresh, one unknown | `cargo test -p codex-router-selection tests::eligibility_penalizes_unknown_or_stale_when_fresh_accounts_exist -- --exact --list` | `cargo nextest run -p codex-router-selection -- tests::eligibility_penalizes_unknown_or_stale_when_fresh_accounts_exist --exact` | unknown account is not selected while known healthy account exists | existing exact test listed once; fixed clock and deterministic state | yes |
| [ ] | 1B-09 | Weekly pressure beats short-reset urgency | spec Account/Quota | T8 | T8 / selector scoring | unit/integration | mixed 5h + weekly windows | `cargo test -p codex-router-selection tests::weekly_quota_pressure_outweighs_short_reset_urgency -- --exact --list` | `cargo nextest run -p codex-router-selection -- tests::weekly_quota_pressure_outweighs_short_reset_urgency --exact` | low-weekly account is not preferred only due short reset | new exact test listed once; fixed windows and clock | yes |
| [ ] | 1B-10 | Repository-backed selector consumes durable windows | spec Account/Quota | T8 | T8 / proxy selector projection | integration | state rows with short + weekly windows | `cargo test -p codex-router-proxy tests::repository_backed_selector_prefers_known_healthy_account_over_unknown_snapshot -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::repository_backed_selector_prefers_known_healthy_account_over_unknown_snapshot --exact` | selector uses durable state and avoids unknown when healthy account exists | new exact test listed once; state fixture recreated | yes |
| [ ] | 1B-11 | Next HTTP/SSE request switches to another eligible account | spec Account/Quota/Protocol | T9 | T9 / `codex-router-proxy::http_sse` | protocol | two accounts + terminal ineligible A | `cargo test -p codex-router-proxy tests::http_proxy_rotates_to_next_eligible_account_after_terminal_ineligibility -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_rotates_to_next_eligible_account_after_terminal_ineligibility --exact` | next request selects B; no inline broad refresh; unaffected route bands stay scoped | new exact test listed once; route band explicit | yes |
| [ ] | 1B-11a | Explicit precommit auth/quota rejection rotates once before response commit | spec Rotation/Quota | T9 | T9 / `codex-router-proxy::http_sse` | protocol | A precommit auth/quota reject, B eligible success | `cargo test -p codex-router-proxy tests::http_proxy_rotates_once_on_explicit_precommit_auth_or_quota_rejection -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_rotates_once_on_explicit_precommit_auth_or_quota_rejection --exact` | A rejection before response commit releases reservation and marks route/account state; B is selected once and commits response; audit rotation count is redacted | new exact test listed once; no broad retry loop or body rewrite after commit | yes |
| [ ] | 1B-11b | Transport and post-commit failures do not trigger router rotation | spec Rotation/Transport non-goal | T9 | T9 / `codex-router-proxy::http_sse` | protocol | 5xx/timeout/DNS/reset/cancel/post-commit fixtures | `cargo test -p codex-router-proxy tests::http_proxy_does_not_rotate_for_transport_or_post_commit_failures -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_does_not_rotate_for_transport_or_post_commit_failures --exact` | router does not create retry/timeout/overload/circuit-breaker policy; transport and post-commit failures surface without alternate account selection | new exact test listed once; selector call count proves no second account | yes |
| [ ] | 1B-12 | WebSocket existing connection stays pinned; next connection can switch | spec Routing/WebSocket | T9 | T9 / `codex-router-proxy::websocket` | protocol | two WS connections | `cargo test -p codex-router-proxy tests::websocket_connection_stays_pinned_while_next_connection_reselects_after_ineligibility -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::websocket_connection_stays_pinned_while_next_connection_reselects_after_ineligibility --exact` | old WS stays A; next WS selects B after A becomes ineligible | new exact test listed once; frame preservation asserted | yes |
| [ ] | 1B-13 | Turn-state envelope pins same-turn HTTP/SSE continuation | spec Routing Granularity | T9 | T9 / selection affinity | protocol | two accounts + signed envelope | `cargo test -p codex-router-proxy tests::http_proxy_turn_state_envelope_pins_same_turn_to_owner_account -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_turn_state_envelope_pins_same_turn_to_owner_account --exact` | continuation uses owning account; invalid/replayed envelope fails locally | new exact test listed once; no silent fallback to other account | yes |
| [ ] | 1B-13a | Turn-state envelope rejects wrong scope, expiry, nonce, and signing key | spec Routing Granularity/Security | T9 | T9 / `codex-router-selection` turn-state codec | unit/security | fixed clock, session, route, nonce fixtures | `cargo test -p codex-router-selection tests::turn_state_envelope_rejects_wrong_scope_expiry_nonce_and_signing_key -- --exact --list` | `cargo nextest run -p codex-router-selection -- tests::turn_state_envelope_rejects_wrong_scope_expiry_nonce_and_signing_key --exact` | cross-turn, cross-session, wrong route, expired, wrong-key, and reused nonce/generation envelopes fail locally | new exact test listed once; valid signature alone is insufficient | yes |
| [ ] | 1B-13b | Turn-state replay fails across restart or router-instance change | spec Routing Granularity/Security | T9 | T9 / turn-state replay owner | unit/integration | two codec/cache instances over one router root | `cargo test -p codex-router-selection tests::turn_state_envelope_replay_fails_across_restart_or_router_instance_change -- --exact --list` | `cargo nextest run -p codex-router-selection -- tests::turn_state_envelope_replay_fails_across_restart_or_router_instance_change --exact` | replayed envelope fails before selection after restart/new router instance; durable nonce state or router-instance signing-key rotation is proven | new exact test listed once; selector/upstream counters stay zero in integration receipt | yes |
| [ ] | 1B-14 | Previous-response affinity prefers owner or fails clearly | spec Routing Granularity | T9 | T9 / affinity repository | protocol | previous_response_id ownership fixture | `cargo test -p codex-router-proxy tests::http_proxy_previous_response_id_prefers_owner_or_fails_clearly -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_previous_response_id_prefers_owner_or_fails_clearly --exact` | owner is selected; disabled/unauthenticated owner fails before different account selection | new exact test listed once; account switch on continuation forbidden | yes |
| [ ] | 1B-14a | Previous-response ownership persists only after commit | spec Routing Granularity/Security | T9 | T9 / affinity repository | protocol/integration | committed vs uncommitted response ids | `cargo test -p codex-router-proxy tests::http_proxy_previous_response_affinity_requires_committed_owner_and_fresh_scope -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_previous_response_affinity_requires_committed_owner_and_fresh_scope --exact` | uncommitted response id does not pin; committed owner pins; disabled owner fails before alternate selection | new exact test listed once; owner-unavailable path is explicit | yes |
| [ ] | 1B-15 | WebSocket first-frame affinity routes before upstream open | spec Routing/WebSocket | T9 | T9 / WebSocket first-frame routing | protocol | continuation metadata in first response.create | `cargo test -p codex-router-proxy tests::websocket_first_frame_affinity_routes_to_owner_before_upstream_open -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::websocket_first_frame_affinity_routes_to_owner_before_upstream_open --exact` | first frame routes to owner before upstream open and remains forwarded unchanged | new exact test listed once; bounded metadata only | yes |
| [ ] | 1B-16a | Local bearer classifier rejects missing, empty, wrong, and old tokens | spec Local Auth | T9/T12 | T9 / local auth classifier | unit | rotated local token | `cargo test -p codex-router-core tests::local_auth_rejects_missing_empty_wrong_and_old_tokens -- --exact --list` | `cargo nextest run -p codex-router-core -- tests::local_auth_rejects_missing_empty_wrong_and_old_tokens --exact` | classifier rejects missing, empty, wrong, and old tokens | existing exact test listed once; this row does not claim proxy ordering | yes |
| [ ] | 1B-16aa | Missing HTTP local token rejects before selection or upstream | spec Local Auth | T9/T12 | T9 / HTTP local auth | integration/security | missing local token | `cargo test -p codex-router-proxy tests::authenticated_http_proxy_rejects_missing_token_before_selection_or_upstream -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::authenticated_http_proxy_rejects_missing_token_before_selection_or_upstream --exact` | HTTP rejects locally before selection and upstream open | existing exact test listed once; selector/upstream counters stay zero | yes |
| [ ] | 1B-16ab | Empty, wrong, and old HTTP local tokens reject before selection or upstream | spec Local Auth/Security | T9/T12 | T9 / HTTP local auth | integration/security | empty, wrong, and rotated local tokens | `cargo test -p codex-router-proxy tests::authenticated_http_proxy_rejects_invalid_and_old_tokens_before_selection_or_upstream -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::authenticated_http_proxy_rejects_invalid_and_old_tokens_before_selection_or_upstream --exact` | empty, wrong, and old HTTP tokens reject locally before account selection and upstream open | new or existing exact test listed once; selector/upstream counters stay zero | yes |
| [ ] | 1B-16b | Missing WebSocket local token rejects before upstream open | spec Local Auth/WebSocket | T9/T12 | T9 / WS local auth | protocol | missing local token | `cargo test -p codex-router-proxy tests::authenticated_websocket_router_rejects_missing_local_token_before_selection -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::authenticated_websocket_router_rejects_missing_local_token_before_selection --exact` | WebSocket rejects locally and opens zero upstream connections | existing exact test listed once; upstream-open count asserted | yes |
| [ ] | 1B-16bd | Empty, wrong, and old WebSocket local tokens reject before selection or upstream | spec Local Auth/WebSocket/Security | T9/T12 | T9 / WS local auth | protocol/security | empty, wrong, and rotated local tokens | `cargo test -p codex-router-proxy tests::authenticated_websocket_router_rejects_invalid_and_old_tokens_before_selection_or_upstream -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::authenticated_websocket_router_rejects_invalid_and_old_tokens_before_selection_or_upstream --exact` | empty, wrong, and old WebSocket tokens reject locally before account selection and upstream open | new exact test listed once; selector/upstream counters stay zero | yes |
| [ ] | 1B-16c | Local token rotation closes old-generation WebSocket | spec Local Auth | T9/T12 | T9 / local auth + WS revocation | integration/protocol | rotated local token | `cargo test -p codex-router-proxy tests::loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket --exact` | rotation closes old-generation WS and rejects old token without upstream open; close reason redacted | existing exact test listed once; rerun after proxy/WS/local-auth changes | yes |
| [ ] | 1B-17a | Route classifier classifies required routes and marks Realtime unsupported | spec Supported Codex Traffic | T9 | T9 / route classifier | unit/protocol | route fixtures | `cargo test -p codex-router-proxy tests::route_classifier_supports_required_codex_routes_and_rejects_realtime -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::route_classifier_supports_required_codex_routes_and_rejects_realtime --exact` | required route kinds classify and Realtime/WebRTC is classified unsupported | existing exact test listed once; before-selection fail-closed proof lives in 1B-17f | yes |
| [ ] | 1B-17aa | `/v1/responses` preserves body bytes and unknown fields | spec Supported Codex Traffic | T9 | T9 / proxy protocol | protocol | mock upstream | `cargo test -p codex-router-proxy tests::http_proxy_preserves_responses_body_bytes_without_interpreting_unknown_fields -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_preserves_responses_body_bytes_without_interpreting_unknown_fields --exact` | POST `/v1/responses` preserves request body bytes and unknown Codex fields | existing exact test listed once; protocol transcript asserted | yes |
| [ ] | 1B-17b | `/v1/models` forwards supported route and preserves `ETag` | spec Supported Codex Traffic | T9 | T9 / proxy protocol | protocol | mock upstream | `cargo test -p codex-router-proxy tests::http_proxy_forwards_supported_routes_and_preserves_models_etag -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_forwards_supported_routes_and_preserves_models_etag --exact` | method/path/status/headers preserved for existing models fixture; local token stripped; upstream auth injected once; `ETag` preserved | existing exact test listed once; query-string coverage lives in 1B-17ba | yes |
| [ ] | 1B-17ba | `/v1/models` preserves query string after route classification | spec Supported Codex Traffic | T9 | T9 / proxy protocol | protocol | mock upstream with query | `cargo test -p codex-router-proxy tests::http_proxy_preserves_models_query_string_after_route_classification -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_preserves_models_query_string_after_route_classification --exact` | `/v1/models?<query>` request query string survives route classification and upstream forwarding unchanged | new exact models-query test listed once; existing `/v1/responses?...` query test is not sufficient | yes |
| [ ] | 1B-17c | `/v1/memories/trace_summarize` forwards protocol | spec Supported Codex Traffic | T9 | T9 / proxy protocol | protocol | mock upstream | `cargo test -p codex-router-proxy tests::http_proxy_forwards_memories_trace_summarize_protocol_unchanged -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_forwards_memories_trace_summarize_protocol_unchanged --exact` | method/path/query/body/status/headers preserved; local token stripped; upstream auth injected once | new exact test listed once; protocol transcript asserted | yes |
| [ ] | 1B-17d | `/v1/responses/compact` forwards protocol | spec Supported Codex Traffic | T9 | T9 / proxy protocol | protocol | mock upstream | `cargo test -p codex-router-proxy tests::http_proxy_forwards_responses_compact_protocol_unchanged -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_forwards_responses_compact_protocol_unchanged --exact` | method/path/query/body/status/headers preserved; local token stripped; upstream auth injected once | new exact test listed once; protocol transcript asserted | yes |
| [ ] | 1B-17e | WebSocket preserves `x-models-etag` when catalog metadata is present | spec Supported Codex Traffic | T9 | T9 / WebSocket protocol | protocol | mock WS upstream | `cargo test -p codex-router-proxy tests::websocket_handshake_preserves_x_models_etag_from_upstream_response -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::websocket_handshake_preserves_x_models_etag_from_upstream_response --exact` | upstream `x-models-etag` survives WS handshake response when catalog metadata is present | exact test listed once; if impossible, route to spec review rather than waiving this row locally | yes |
| [ ] | 1B-17f | Unsupported Realtime/unknown routes fail before selection or upstream | spec Supported Codex Traffic | T9 | T9 / proxy fail-closed routing | protocol/security | unsupported route fixtures | `cargo test -p codex-router-proxy tests::authenticated_http_proxy_rejects_realtime_before_selection_or_upstream -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::authenticated_http_proxy_rejects_realtime_before_selection_or_upstream --exact` | Realtime/WebRTC and unknown routes fail closed before account selection and upstream open | new exact test listed once; selector/upstream counters stay zero | yes |
| [ ] | 1B-18 | Status command is SQLite-only and readable | spec Account/Quota | T10 | T10 / `codex-router-cli::quota status` | integration | persisted status rows | `cargo test -p codex-router-cli tests::quota_status_reads_sqlite_rows_without_provider_io -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_status_reads_sqlite_rows_without_provider_io --exact` | status renders a basic readable SQLite-backed table and performs zero provider calls | existing exact test listed once; detailed effective/pace/runout/redaction fields are proven by 1B-19 through 1B-21 | yes |
| [ ] | 1B-19 | Expanded status keeps effective row visible first | spec Account/Quota | T10 | T10 / status renderer | integration | multi-window rows | `cargo test -p codex-router-cli tests::quota_status_all_limits_keeps_effective_row_visible_first -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_status_all_limits_keeps_effective_row_visible_first --exact` | effective row remains visible with every provider window in deterministic order | new exact test listed once; deterministic ordering | yes |
| [ ] | 1B-20 | Pace/runout math matches fixed-window expectations | spec Account/Quota | T10 | T10 / status math | unit | fixed clock/window rows | `cargo test -p codex-router-cli tests::quota_status_formats_pace_and_projected_runout_from_fixed_windows -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_status_formats_pace_and_projected_runout_from_fixed_windows --exact` | expected-vs-actual pace and burn-rate runout match fixed fixture | new exact test listed once; fixed now, elapsed, used, remaining | yes |
| [ ] | 1B-21 | Status redacts failure notes | spec Account/Quota/Security | T10 | T10 / status renderer | integration | token/account canaries | `cargo test -p codex-router-cli tests::quota_status_redacts_failure_notes_without_token_or_account_leak -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_status_redacts_failure_notes_without_token_or_account_leak --exact` | output omits token, raw account email, and secret-bearing diagnostics | new exact test listed once; unique canaries | yes |
| [ ] | 1B-22 | Docs and root help state current command truth | spec Activation/Secret Storage | T11 | T11 / docs + CLI help | docs/manual | root `--help` output plus docs checklist | `cargo run -p codex-router-cli -- --help` | `cargo run -p codex-router-cli -- --help`; `rg -n -e "account login" -e "account logout" -e "account remove" -e "keyring" -e "--auth-json" -e "--path" README.md docs/testing/live-oauth-quota.md` | docs use `--auth-json`; login/logout/remove/keyring marked Plan 2; file import not normal steady-state; no nonexistent subcommand-help command is required unless T11 explicitly implements it | manual docs checklist must classify each match as correct Plan 2/deferred wording or fail the receipt; command output alone is not proof | no |
| [ ] | 1B-23 | Installed smoke quota startup/status scenario enumerates exactly | spec Smoke | T12 | T12 / smoke harness | smoke | installed Codex mock with delayed quota | `cargo test -p codex-router-test-support installed_codex::tests::installed_codex_mock_smoke_proves_nonblocking_quota_startup_and_redacted_status -- --ignored --exact --list` | `cargo nextest run -p codex-router-test-support --run-ignored ignored-only -- installed_codex::tests::installed_codex_mock_smoke_proves_nonblocking_quota_startup_and_redacted_status --exact` | installed Codex version/profile captured; temp profile used; startup is not quota-blocked while quota refresh is delayed; redacted quota status table is captured after background refresh | new exact ignored smoke test lists once and fails on the current stale transcript-only harness | yes |
| [ ] | 1B-23a | Installed smoke transcript is redacted and schema-allowlisted | spec Smoke/Security | T12 | T12 / smoke harness transcript | smoke/security | installed Codex transcript with token/body/prompt canaries | `cargo test -p codex-router-test-support installed_codex::tests::installed_codex_mock_smoke_transcript_is_redacted_and_schema_allowlisted -- --ignored --exact --list` | `cargo nextest run -p codex-router-test-support --run-ignored ignored-only -- installed_codex::tests::installed_codex_mock_smoke_transcript_is_redacted_and_schema_allowlisted --exact` | transcript JSON contains only allowlisted keys and omits raw tokens, account labels, prompts, request/response bodies, memory traces, tool arguments, and full first-frame payloads | new exact ignored smoke test lists once; token/body/prompt canaries fail if present | yes |
| [ ] | 1B-24 | Installed smoke hostile no-token scenario enumerates exactly | spec Smoke | T12 | T12 / installed Codex mock | smoke | installed Codex mock | `cargo test -p codex-router-test-support installed_codex::tests::installed_codex_hostile_no_token_smoke_keeps_upstream_empty -- --ignored --exact --list` | `cargo nextest run -p codex-router-test-support --run-ignored ignored-only -- installed_codex::tests::installed_codex_hostile_no_token_smoke_keeps_upstream_empty --exact` | hostile local request without router token opens zero upstream connections | named ignored test lists exactly once and output includes zero-upstream observation | yes |
| [ ] | 1B-25 | Installed smoke all-scenario wrapper is explicit | spec Smoke | T12 | T12 / installed Codex mock | smoke | installed Codex mock | `bash -n tests/smoke/installed_codex_mock.sh && bash -lc 'rg -n -e "EXPECTED_SCENARIOS" -e "installed_codex::tests::installed_codex_" tests/smoke/installed_codex_mock.sh && ! rg -n -e "installed_codex_[[:space:]]*\\\\" -e "installed_codex_[[:space:]]*$" tests/smoke/installed_codex_mock.sh'` | `tests/smoke/installed_codex_mock.sh` | wrapper enumerates expected scenarios explicitly, runs each once, prints count equal to the explicit expected-scenario array length, and fails if any expected scenario is missing | syntax passes, source search lists both exact smoke scenarios, negative broad-prefix guard passes, and wrapper output count equals array length | yes |
| [ ] | 1B-26 | Live OAuth/quota proof is gated | spec Gated live | T12 | T12 / live proof runbook | gated live | real accounts only with approval | `rg -n -e "live_oauth_quota_gate:" -e "approval:" -e "result:" -e "not-run: approval required" docs/testing/live-oauth-quota.md` | if no explicit approval: `bash -lc '! rg -n -e "live_oauth_quota_gate: run" -e "approval: explicit" -e "result:" docs/testing/live-oauth-quota.md'`; if approved: runbook commands only after explicit approval | without approval, current gate block records only `not-run: approval required`; with approval, current dated approval and redacted proof are attached | stale generic approval text is not enough; current gate block decides pass/fail | no unless approved |
| [ ] | 1B-27 | Final provider-token egress surfaces cannot bypass resolver | plan-review-after-8fb965f resolver-bypass scope | T12 | T12 / runtime provider egress surfaces | structural/compile | source search plus package compile | `rg -n -e "read_secret" -e "upstream_access_token_key" -e "upstream_refresh_token_key" crates/codex-router-quota/src crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs` | `bash -lc '! rg -n -e "read_secret" -e "upstream_access_token_key" -e "upstream_refresh_token_key" crates/codex-router-quota/src crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs' && cargo check -p codex-router-quota -p codex-router-cli -p codex-router-proxy` | zero provider secret/token-key reads in quota, quota CLI refresh/status, HTTP/SSE, and WebSocket runtime egress paths; account import/bootstrap/local router token code is outside this provider-token row | structural row expects zero matches in listed egress paths; tests/bootstrap/local-token modules are not scanned here | yes |
| [ ] | 1B-27a | Final backend construction remains isolated to named factories | plan-review-after-8fb965f backend-neutrality scope | T12 | T12 / runtime backend construction edges | structural/compile | source search plus package compile | `rg -n -e "FileSecretStore" -e "file_backend::SecretStore" crates/codex-router-cli/src crates/codex-router-proxy/src` | `bash -lc 'rg -n -e "FileSecretStore" -e "file_backend::SecretStore" crates/codex-router-cli/src crates/codex-router-proxy/src; ! rg -v -e "src/secret_store_factory.rs" -e "#\\[cfg\\(test\\)\\]" -e "mod tests" <(rg -n -e "FileSecretStore" -e "file_backend::SecretStore" crates/codex-router-cli/src crates/codex-router-proxy/src)' && cargo check -p codex-router-cli -p codex-router-proxy` | concrete file backend appears only in named factory modules or test-only code; runtime entrypoints consume backend-neutral trait/factory APIs | receipt includes raw search output and path-based allowlist; broad substring allowlists are forbidden | yes |

## Review Gate

- [ ] Run `implementation-review-swarm` with quota runtime, selector, status UX, smoke, and docs lanes.
- [ ] Do not claim Plan 1B complete until all required proof rows are checked or explicitly deferred with user approval.

## Merge Gate B0: Failure-Taxonomy Receipt

Required before T7 starts:

- [ ] Matrix rows 1B-01 through 1B-02b pass or route back to planning.
- [ ] The plan confirms transient failures preserve last-known selector/status
      state and terminal failures scope ineligibility to affected account/route
      bands.
- [ ] Response-backed alias family publication proof passes; no mixed-generation
      alias family is observable under injected failure.
- [ ] Dirty-tree isolation receipt proves only T6-owned paths were staged.
- [ ] `git show --name-only <B0-checkpoint>` lists only T6-owned paths.
- [ ] Same-path baseline hunks are accounted for and no baseline-only hunk is in
      the checkpoint commit.

## Merge Gate B1: Runtime/Status/Docs-Ready Receipt

Required before T12 starts:

- [ ] Matrix rows 1B-03 through 1B-22 pass or route back to planning.
- [ ] Explicit precommit auth/quota rotation rows 1B-11a and 1B-11b pass.
- [ ] Replay restart row 1B-13b passes.
- [ ] WebSocket invalid-token row 1B-16bd passes.
- [ ] Cross-process refresh owner/follower and stale-owner recovery proof passes.
- [ ] Replay-safe turn-state and previous-response affinity proof passes.
- [ ] Local bearer-token lifecycle proof is rerun with exact commands if T9 or
      T12 touched proxy, WebSocket, HTTP/SSE, local-auth, or token reload
      surfaces; stale pre-change receipts are not sufficient for those changes.
- [ ] Same-turn and previous-response affinity proof is complete.
- [ ] Status UX is SQLite-only and docs match current command truth.
- [ ] Dirty-tree isolation receipt proves only T7-T11 owned paths were staged.
- [ ] `git show --name-only <B1-checkpoint>` lists only T7-T11 owned paths.
- [ ] Same-path baseline hunks are accounted for and no baseline-only hunk is in
      the checkpoint commit.

## Final Closeout Gate

- [ ] Matrix rows 1B-23, 1B-23a, 1B-24, and 1B-25 pass. They are mandatory
      installed-smoke proof and must not be marked `not-run`.
- [ ] Matrix row 1B-26 passes only if explicitly approved and run; otherwise it
      records `not-run: approval required`.
- [ ] Matrix rows 1B-27 and 1B-27a pass as final resolver-bypass and backend
      construction proof across quota, serve/refresh, HTTP/SSE, and WebSocket
      runtime egress surfaces.
- [ ] Installed smoke enumerates and runs each exact scenario, including hostile
      no-token, with scenario count in output.
- [ ] Supported-route protocol/header proof passes for models `ETag`,
      WebSocket `x-models-etag`, memories trace summarize, responses compact,
      and unsupported Realtime fail-closed behavior. If WebSocket
      `x-models-etag` is impossible in the current product contract, route to
      spec review instead of waiving row 1B-17e locally.
- [ ] Full required local gates pass from the current checkout.
- [ ] Dirty-tree isolation receipt proves only T12 and closeout-owned paths were
      staged.
- [ ] `git show --name-only <final-checkpoint>` lists only owned paths.
- [ ] Same-path baseline hunks are accounted for and no baseline-only hunk is in
      the checkpoint commit.
- [ ] `implementation-review-swarm` completes with no unresolved blockers.

## Replan Triggers

- [ ] Immediate refresh cannot be tested without unbounded sleeps.
- [ ] Smoke harness cannot enumerate required named scenarios.
- [ ] Weekly-aware selection cannot consume the Plan 1A durable selector source cleanly.
- [ ] Implementation reveals the spec is wrong or contradictory.
