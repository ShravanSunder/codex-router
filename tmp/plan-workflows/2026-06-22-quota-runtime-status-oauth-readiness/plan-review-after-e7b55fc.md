# Plan Review After `e7b55fc`

Date: 2026-06-22
Workflow: `shravan-dev-workflow:plan-review-swarm`
Goal id: `2026-06-22-codex-router-quota-oauth-runtime`
Reviewed commit: `e7b55fc31c7b7a53507eacd8c87ef0201729ba17`
Verdict: `needs_revision`
Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`

## Coverage

- Source spec:
  `docs/specs/2026-06-20-codex-router-greenfield-spec.md`,
  497 lines, read in chunks `1-180`, `181-360`, `361-497`.
- Umbrella plan:
  `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`,
  360 lines, read in chunks `1-220`, `221-360`.
- Plan 1A:
  `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`,
  553 lines, read in chunks `1-180`, `181-360`, `361-553`.
- Plan 1B:
  `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`,
  689 lines, read in chunks `1-180`, `181-360`, `361-540`, `541-689`.
- Prior review receipt:
  `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-918db95.md`,
  358 lines, read in chunks `1-180`, `181-358`.
- Workflow details:
  `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/details.md`.
- Transition log:
  `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/events.jsonl`.

Parent checks:

- Current branch/head verified:
  `feature/initial-codex-router` at
  `e7b55fc31c7b7a53507eacd8c87ef0201729ba17`.
- Table and whitespace checks passed before `e7b55fc` was committed:
  `git diff --check -- docs/plans/...` returned exit `0`.
- The committed checkpoint contains only the three durable plan files.
- Push remains blocked by configured remote
  `https://github.com/shravan-agent/codex-router.git/` returning repository
  not found.
- Exact-one command failure mode was reproduced locally: the current
  `1A-00h`/`1B-23c` command shape exits `0` when both the supposed real test
  and missing sentinel are absent, because the first `exact_one` failure is not
  chained or protected by `set -e`.
- `1B-07b` false-failure mode was reproduced locally: a state-owned API named
  `acquire_quota_refresh_lease` fails the row because the negated search bans
  `refresh_lease` inside state files before the positive state-owned lease
  search can run.
- Current path existence checks showed these planned paths do not exist yet:
  `crates/codex-router-cli/src/secret_store_factory.rs`,
  `crates/codex-router-proxy/src/secret_store_factory.rs`, and
  `crates/codex-router-proxy/src/account_selection.rs`.

## Swarm Lanes

- `spec-compliance + OAuth/account UX scope`:
  `019eefbd-66cc-7c40-8a48-79ca5d95ea06`, verdict `ready`.
- `architecture-boundary + execution-scope`:
  `019eefbd-9112-7682-885b-3e25996280ad`, verdict `needs_revision`.
- `testability-validation + proof exactness`:
  `019eefbd-bf81-7a03-824d-8cf772f7930f`, verdict `needs_revision`.
- `security-reliability`:
  `019eefbd-e907-7212-9d04-1a6be6d6aaab`, verdict `needs_revision`.

No external model lanes were requested. All lanes were read-only.

## Accepted Blockers

### B1. Exact-one helper proof commands can still false-green

Evidence:

- Plan 1A row `1A-00h` uses a command that runs
  `exact_one "tests::profile_write_command_requires_approval_flag"` and then
  continues to the missing-sentinel branch without `set -e`, `&&`, or an
  explicit failure on the real-test guard.
- Plan 1B row `1B-23c` repeats the same command shape.
- Parent reproduced the failure by substituting a missing real-test name:
  the command exited `0` when both the real-test guard and missing-sentinel
  guard failed.

Failure scenario:

- A renamed, deleted, or never-created required test can fail the real-test
  exact-one guard, but the row still exits `0` because the final missing
  sentinel check succeeds in the expected negative direction.

Required plan change:

- Replace `1A-00h` and `1B-23c` with commands that fail immediately when the
  real-test exact-one guard fails. Use `set -euo pipefail`, `&&`, or an
  explicit `real_count`/`missing_count` comparison that exits nonzero unless
  real count is exactly `1` and missing count is exactly `0`.
- Require the receipt to print or capture both observed counts.

### B2. Row `1B-07b` forbids the chosen state-backed lease API name

Evidence:

- The row says Plan 1B chooses a state-backed SQLite lease/repository API used
  by quota.
- Its execution command first runs a negated search for both
  `RefreshLeaseManager` and `refresh_lease` across quota, CLI, and state files.
- Parent reproduced that a plausible correct state API name such as
  `acquire_quota_refresh_lease` fails the negated search before the row can
  prove the state-backed lease exists.

Failure scenario:

- An implementation can correctly create state-owned refresh-lease APIs and
  still fail the structural row merely because those APIs use the natural
  `refresh_lease` vocabulary.

Required plan change:

- Split the forbidden and required searches:
  forbid `RefreshLeaseManager` / `codex_router_secret_store::refresh_lease` in
  quota and CLI refresh-cycle entrypoints, but allow and require
  `quota.*lease` or `refresh.*lease` in state repository/sqlite files.

### B3. Rows `1B-27a` and `1B-27b` rely on stale or future paths

Evidence:

- `1B-27a` hard-codes factory paths that do not exist in the reviewed checkout:
  `crates/codex-router-cli/src/secret_store_factory.rs` and
  `crates/codex-router-proxy/src/secret_store_factory.rs`.
- `1B-27b` hard-codes
  `crates/codex-router-proxy/src/account_selection.rs`, which also does not
  exist in the reviewed checkout.
- `1B-27b` wraps a multi-path `rg` in `!`; a missing path can therefore make
  the command succeed for the wrong reason.
- `1B-27b` preflight scans `server.rs` for token-carrier terms but the
  execution command's token-carrier freshness scan omits `server.rs`.

Failure scenario:

- The final structural freshness proof can fail for path-not-found instead of
  semantic regressions, or pass because an inverted `rg` absorbed a missing
  path error. A token-carrier regression in `server.rs` can also escape the
  execution command.

Required plan change:

- Make these rows either:
  - use a named helper that groups existing/runtime/factory/test-only matches
    and fails on forbidden production matches, or
  - use only existing directories plus explicit `test -e` assertions for
    planned files after the task that creates them.
- `1B-27b` must not use `! rg` over possibly missing paths, and must include
  `server.rs` in the token-carrier execution scan.

### B4. Plan 1B omits `crates/codex-router-auth/src/lib.rs` from quota-client migration scope

Evidence:

- Plan 1B moves quota DTO/client ownership out of auth and marks
  `crates/codex-router-auth/src/live_quota.rs` and
  `crates/codex-router-auth/src/quota_client.rs` as migration/deletion
  surfaces.
- Current `crates/codex-router-auth/src/lib.rs` exports both modules and
  imports their types in local tests.
- Plan 1B write surfaces and T6 file ownership do not include
  `crates/codex-router-auth/src/lib.rs`.

Failure scenario:

- A correct migration that removes/demotes auth-owned quota client modules will
  require `lib.rs` edits, but the plan forbids that path. The checkpoint
  owned-path audit can then fail despite correct implementation.

Required plan change:

- Add `crates/codex-router-auth/src/lib.rs` to Plan 1B write surfaces and T6
  ownership.
- Mention it in `1B-10a` receipt expectations as part of the provider-client
  ownership migration.

## Accepted Important Findings

### I1. WebSocket previous-response restart/unknown-owner proof is not explicit

Evidence:

- The spec requires previous-response continuation traffic to use the owning
  account or fail clearly if the owner is unavailable.
- Plan 1B T9 states previous-response ownership must be resolved before
  weighted selection and unknown ownership after restart fails closed.
- Rows `1B-14b` and related previous-response proof are HTTP/SSE-oriented.
- Row `1B-15` proves WebSocket first-frame routing and preservation, but it
  does not explicitly prove first-frame `previous_response_id` restart and
  unknown-owner fail-closed behavior before upstream open.

Failure scenario:

- HTTP/SSE continuation can reject unknown owners correctly while the WebSocket
  first-frame path falls through to normal selection or opens upstream before
  proving durable ownership.

Required plan change:

- Add a T9 proof row adjacent to `1B-14b`/`1B-15` for WebSocket
  `previous_response_id` continuation across restart:
  durable owner present pins before upstream open; absent owner rejects locally
  before selection/upstream, with selector/upstream counters at zero.

### I2. Plan 1A must make the live selector adapter boundary mechanically auditable before Plan 1B T8

Evidence:

- Plan 1B T8 owns `crates/codex-router-selection/src/*`,
  `crates/codex-router-proxy/src/account_selection.rs`, and state files.
- Current live selector types and repository-backed selector logic still live
  in `crates/codex-router-proxy/src/http_sse.rs`, and
  `server.rs` constructs that selector from the HTTP/SSE module.
- Plan 1A implies DTO extraction/adapter ownership but does not require an A2
  receipt proving the live selector used by `server.rs` has moved behind
  `account_selection.rs` or another named T8-owned adapter.

Failure scenario:

- Plan 1A can pass without moving the live selector implementation out of
  `http_sse.rs`; then Plan 1B T8 cannot implement weekly-aware selection within
  its declared file ownership.

Required plan change:

- Add an A2 structural receipt proving the live selector adapter used by
  `server.rs` is no longer owned by `http_sse.rs` and is reachable through
  `crates/codex-router-proxy/src/account_selection.rs` or another named file
  already included in Plan 1B T8 write surfaces.

### I3. T9 should state the final role of `selection::turn_state` and `selection::affinity`

Evidence:

- Plan 1B chooses durable replay and previous-response ownership in
  `codex-router-state`.
- Current `selection::turn_state` carries `upstream_token`, and
  `selection::affinity` is in-memory.
- The plan forbids provider auth in turn-state envelopes and durableizes replay
  ownership, but it does not explicitly state whether the selection modules
  become codec-only/stateless helpers or are replaced.

Failure scenario:

- Implementation can leave replay or provider-token responsibilities split
  between selection and state while still claiming state owns durable replay.

Required plan change:

- In T9 or B1, state that `selection::turn_state` becomes a stateless codec
  only, with replay and committed-owner persistence exclusively in state, or
  that it is deleted/replaced.
- Add a structural proof row or extend `1B-27b` to show no replay persistence
  or provider auth material remains in `turn_state.rs` / `affinity.rs`.

### I4. `1B-26` should reject any approval key while live proof is `not-run`

Evidence:

- Row `1B-26` rejects `approval: explicit` and `result:`.
- The intent is that live proof is absent unless approved and replanned.

Failure scenario:

- A current gate block with `approval: implicit`, `approval: stale`, or another
  approval key can still pass while the gate says `not-run`.

Required plan change:

- When `live_oauth_quota_gate: not-run`, reject any `^approval:` and any
  `^result:` key, or validate an exact allowed-key set.

## Rejected Or Deferred Candidate Findings

- Spec-compliance concerns about Plan 1/Plan 2 boundaries are rejected. The
  revised plans now clearly defer OAuth/device-code login, Keychain/keyring,
  logout/remove, and real live quota pooling proof.
- The current product-code debt is not itself a plan-review blocker. It is a
  blocker only where the plan cannot make the required future edit or proof
  mechanically auditable.
- The note that `1B-10a` should name manifest touchpoints is useful but treated
  as a nit; the existing cargo-tree proof is directionally adequate once
  `auth/src/lib.rs` is added to the write scope.

## Route Decision

phase_result: `needs_revision`

evidence:

- this review artifact
- four read-only lane receipts listed above
- parent verification commands described in coverage
- current revised plans at reviewed commit
  `e7b55fc31c7b7a53507eacd8c87ef0201729ba17`

recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`

recommended_transition_reason: The revised plan fixed the post-`918db95`
findings but still has accepted blocker and important findings in exact-one
helper commands, quota lease structural proof, final structural proof path
freshness, quota-client migration write scope, WebSocket previous-response
proof, selector-adapter auditability, selection/state replay ownership, and
live-gate approval-key parsing. Code implementation must not begin until those
issues are folded into durable plan docs and reviewed.
