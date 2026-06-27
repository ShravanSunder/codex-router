# Account Quota Burn-Rate Selection Spec

Date: 2026-06-27
Status: accepted for implementation after fresh spec review

## Product Intent

`codex-router` exists to route Codex traffic across configured OAuth accounts.
Its owned behavior is account auth, account selection, quota safety, and the
minimum affinity/retry behavior required to make those safe. Everything else is
pass-through Codex protocol behavior.

The quota selector must answer one product question:

> Which account should receive the next Codex work so no single account burns
> out before its quota reset while other router accounts can still serve?

Weekly quota is the primary constraint. The 5h quota is a short-term flow guard.
Active sessions matter because they explain observed burn and project future
burn; they are not a fake fixed quota cost.

## Current-State Evidence

- `crates/codex-router-selection/src/burn_down.rs` currently accepts
  `active_load_pressure`, subtracts it from routing weight, and clamps selectable
  weight to a minimum of `1`.
- `crates/codex-router-proxy/src/account_selection.rs` currently assigns
  transport-specific pressure: HTTP/SSE `2`, WebSocket `8`.
- `crates/codex-router-selection/src/reservation.rs` models reservations as
  `headroom_cost`, and active pressure is the sum of those costs.
- `crates/codex-router-state/src/sqlite.rs` already owns SQLx state for
  selector windows, quota history, refresh status, active lease mirrors, and
  route-band account state.
- `active_client_leases` is current-state only. Released or pruned leases cannot
  reconstruct completed session-hours, so it is not enough for historical burn
  normalization.
- `crates/codex-router-selection/src/run_rate.rs` already computes same-reset
  segment quota burn and projected exhaustion from quota history. This is the
  right pure-code starting point, but it lacks session-hour inputs.
- `crates/codex-router-proxy/src/provider_error.rs` classifies
  `usage_limit_reached`, `quota_exceeded`, and `insufficient_quota` as account
  quota exhaustion. That hard evidence must remain separate from statistical
  burn-rate forecasting.
- The local Codex source verifies that `usage_limit_reached` is user-visible
  rate-limit behavior, while `websocket_connection_limit_reached` is a tested
  Responses WebSocket reconnect path.

## Superseded Slice

This spec supersedes the quota-selection and active-load parts of
`docs/specs/2026-06-26-quota-routing-safety-spec.md`.

The earlier pass-through, Codex-safe exhaustion, SQLx-only storage, and
observability requirements still stand. The old "active load as reserved future
burn" and request cost class model does not stand.

## Non-Goals

- No payload validation beyond existing routing/auth/affinity needs.
- No account switching per WebSocket message.
- No synthetic cost difference between WebSocket and HTTP/SSE for quota
  selection.
- No smooth weighted deficit as the final quota selector.
- No lower-bound weight that keeps a weak account selectable as `score 1`.
- No new storage backend. Router-owned production storage uses SQLx only.
- No Codex CLI changes.
- No raw provider quota body, token, prompt, payload, account id, account label,
  reservation id, or filesystem path in telemetry.

## Requirements

### R0. V1 Policy Constants

The first implementation uses explicit, test-visible policy constants:

```text
weekly_survival_safety_buffer_basis_points = 200   # 2.00%
short_survival_safety_buffer_basis_points = 100    # 1.00%
short_near_reset_threshold_seconds = 1_800         # 30 minutes
same_pool_reset_tolerance_seconds = 7_200          # 2 hours
same_pool_survival_margin_tolerance_basis_points = 500  # 5.00%
active_session_imbalance_threshold = 2
usage_limit_suspect_ttl_seconds = 300
active_session_rollup_bucket_seconds = 300
```

These constants are policy inputs, not hidden literals. Tests must pass them
explicitly or assert the default policy, so future tuning cannot silently change
the selector contract.

### R1. Strict Next Account

`codex-router quota` and runtime selection must agree on the next account for
the same inputs.

`preferred by quota` means the next non-affinity, non-hard-pinned request will
select that account unless fresher state arrives before selection.

The final quota selector is deterministic. It must not use smooth weighted
deficit or any balancing algorithm that eventually selects weak accounts because
their deficit accumulates.

Runtime selection owns the authoritative current active-session snapshot because
it holds the in-process connection/reservation lifecycle. SQLx owns the durable
mirror and historical event/rollup state. `codex-router quota` must use the same
selector engine and SQLx mirror; when the mirror is fresh, its selected account
must match runtime for the same fixture inputs. When the mirror is stale or
unavailable, the CLI must label active-session state as stale/unavailable and
must not present its next-account answer as live-load exact.

### R2. Weekly Survival First

For each account and route band, the selector computes:

```text
projected_weekly_burn_to_reset =
  projected_weekly_burn_percent_per_hour * hours_until_weekly_reset

weekly_survival_margin =
  current_weekly_remaining_percent - projected_weekly_burn_to_reset
```

An account is a weekly survivor when `weekly_survival_margin` is at or above the
configured safety buffer. Survivors are preferred over non-survivors even when a
non-survivor has higher raw remaining percentage.

Among weekly survivors, first form same-effective-weekly pools. Two accounts are
in the same effective weekly pool when:

- both pass hard blocks and the 5h guard;
- both are weekly survivors;
- their weekly reset times differ by no more than
  `same_pool_reset_tolerance_seconds`;
- their weekly survival margins differ by no more than
  `same_pool_survival_margin_tolerance_basis_points`; and
- both accounts have the same burn-rate confidence tier.

Inside a same-effective-weekly pool, active session imbalance is an ordering
rule, not a cosmetic tie-break. If active counts differ by at least
`active_session_imbalance_threshold`, choose the lower-active account unless its
projected burn, 5h guard, affinity, or hard block status makes it ineligible.
A same-pool account with six active sessions must not keep receiving new
sessions while a same-pool peer has zero active sessions and comparable
survival.

Before reset optimization, prefer the highest available burn-rate confidence
tier. A lower-confidence account with insufficient burn history does not beat a
known survivor only because it resets sooner. Inside the highest available
confidence tier, after same-pool active balancing, prefer the account with the
earliest weekly reset. That uses quota that is closest to being refreshed and
preserves later-reset accounts for later work. If reset times are still tied,
prefer larger survival margin, then lower current active session count, then
stable account order.

When no account survives weekly, choose the account with the least bad weekly
projection: latest projected weekly runout is primary, then less negative
survival margin, then higher confidence, then lower current active sessions,
then stable account order. The 5h guard and hard blocks still apply.

### R3. 5h Flow Guard

The 5h window is not the primary optimizer. It guards against routing new work
to an account that will stall before its short-window reset.

For each account, compute:

```text
projected_5h_burn_to_reset =
  projected_5h_burn_percent_per_hour * hours_until_5h_reset

short_survival_margin =
  current_5h_remaining_percent - projected_5h_burn_to_reset
```

If the account is projected to run out before the 5h reset and the reset is not
near enough to be considered safe, the account is held out of new work even if
weekly quota is healthy.

If the 5h reset is near and the projected burn to reset is within the safety
buffer, the account may remain eligible. This keeps short-window resets flowing
without treating short quota as more important than weekly survival.

### R4. Active Sessions Are Measurement, Not Cost

Active sessions are per account and per route band. They are not
transport-specific quota costs.

The selector must not consume:

- `HTTP_ACTIVE_LOAD_PRESSURE`;
- `WEBSOCKET_ACTIVE_LOAD_PRESSURE`;
- `headroom_cost`;
- summed active pressure;
- request cost class;
- transport kind as a quota cost.

The selector may consume:

- current active session count per account;
- current candidate session count for this selection;
- historical active session seconds per account/window interval;
- maximum concurrent sessions for diagnostics and tie-breaks;
- confidence about whether session history is sufficient.

For new work, the projected active session count for a candidate is the current
active session count plus one selection unit. That unit is the same for
WebSocket and HTTP/SSE quota selection. Transport kind may remain diagnostic
metadata, but it does not change quota math.

### R5. Burn-Rate Estimation

Quota burn-rate estimation uses same-reset-segment quota observations. Do not
compute a burn slope across a reset boundary.

For each account, route band, and window:

```text
raw_burn_percent_per_hour =
  max(0, prior_remaining_percent - latest_remaining_percent)
  / elapsed_hours

active_session_hours =
  active_session_seconds_between(prior_observed, latest_observed) / 3600

per_session_burn_percent_per_hour =
  burned_percent / active_session_hours
```

If active-session history is sufficient, project candidate burn with:

```text
projected_burn_percent_per_hour =
  per_session_burn_percent_per_hour * projected_active_session_count
```

All percent values in selector math use basis points, where `10_000` is 100%.
Burn rates use basis-points-per-hour and fixed-point integer arithmetic. When a
division cannot be exact, round projected burn up for safety. Display layers may
render decimal percentages, but selector comparisons use basis points.

If active-session history is insufficient but quota history is sufficient, use
the account aggregate `raw_burn_percent_per_hour` with lower confidence.

If quota burn occurs while active-session history says zero sessions were
active, mark the estimate anomalous and fall back to account aggregate burn. Do
not divide by zero and do not manufacture a synthetic active cost.

If quota history is insufficient, the selector can still use current remaining
quota and reset time, but the candidate is lower confidence than a candidate
with a fresh survival estimate.

### R6. Hard Account Blocks

The selector must hard-filter:

- disabled accounts;
- accounts without active credentials;
- accounts already attempted for this request/rotation;
- accounts with active usage-limit evidence for the route band;
- accounts with exhausted or ineligible selector windows;
- accounts with missing weekly reset data when known alternatives exist.

Usage-limit evidence is hard provider truth, not a burn-rate input. It blocks
the account for that route band until a successful refresh/probe clears it or a
known reset is reached and verified. If neither clear path happens before
`usage_limit_suspect_ttl_seconds`, the state may move to `unknown_needs_probe`,
but unknown fallback can only be selected when no known usable account exists.

### R7. Affinity And Existing Work

Previous-response affinity and same-turn continuation are stronger than ordinary
quota preference, but weaker than hard account unavailability.

If the affinity account is still usable, the continuation stays there. If the
affinity account is near retirement but not hard-blocked, existing work may
finish there while new work goes elsewhere. If the affinity account is hard
blocked, the router must use a Codex-compatible retry/reconnect path or return a
router-level safety error.

Account hold cooldown is a stability mechanism, not a quota override. It cannot
force new work onto an account that fails weekly survival, the 5h guard, or
usage-limit blocks.

### R8. Codex-Safe Exhaustion Containment

Codex must not see one account's raw `usage_limit_reached` while another router
account can serve.

Before downstream response bytes are committed, HTTP/SSE requests may retry on a
different account after marking the exhausted account blocked.

After HTTP/SSE response bytes are committed, the router remains pass-through
unless a separate approved spec defines a bounded provider error-envelope
detector. This spec does not approve broad buffering or parsing to hide
post-commit provider errors.

For WebSockets, the current allowed reconnect signal is the Codex source-backed
`websocket_connection_limit_reached` Responses error envelope. The router may use
it only as a compatibility signal to make Codex reconnect through the router
after the exhausted account has been marked blocked. It is not quota truth.

The WebSocket detection boundary is bounded to provider error envelopes only:
the router may inspect complete WebSocket text messages that parse as Responses
error envelopes with a recognized account-exhaustion code and must leave all
other WebSocket messages byte-for-byte pass-through at the message level. The
router must not parse prompts, tool payloads, deltas, arbitrary JSON content, or
non-error provider messages for quota purposes.

If all accounts are exhausted, Codex may see a router-level all-accounts
exhausted error. That error must not contain one account's raw provider body.

### R9. SQLx State Domains

Router-owned production storage remains SQLx-only.

The state layer owns these domains:

```text
accounts
  account_id, label, status, active_credential_generation

selector_quota_windows
  current per-account route-band window facts

quota_history_observations
  append-only provider quota observations by reset segment

quota_refresh_status
  last attempt, last success, stale-after, redacted error class

active_session_leases
  current process-visible active session mirror for status

active_session_events
  append-only acquired/released/stale-purged lifecycle events

active_session_rollups
  bucketed session-seconds for burn-rate normalization

route_band_account_states
  suspect/exhausted account state for usage-limit hard blocks

previous_response_affinity_owners
  sticky continuation ownership
```

`active_session_events` and `active_session_rollups` are new conceptual domains
for this spec. The existing `active_client_leases` table remains a current-state
mirror unless the implementation plan replaces it with an equivalent current
active-session table.

`active_session_events` must include enough data to reconstruct completed
session intervals after the current lease is released:

```text
event_id
process_run_id
reservation_id
account_id
route_band
event_kind                    # acquired | released | stale_purged | retired
observed_unix_seconds
session_started_unix_seconds
session_ended_unix_seconds    # present for terminal events
transport_kind                # diagnostic only; not selector cost
```

`active_session_rollups` must be derivable from those events and must store:

```text
account_id
route_band
bucket_start_unix_seconds
bucket_seconds
active_session_seconds
max_concurrent_sessions
completed_sessions
stale_purged_sessions
```

Overlap accounting is additive: two sessions active for the same 300-second
bucket produce 600 active-session-seconds. A re-reservation for the same
connection is a continuation of the same session interval unless the previous
reservation was terminally released, retired, or stale-purged.

Legacy `active_pressure` columns or fields may be retained only for migration
readability while being ignored by selector math. New code must not treat them
as quota pressure.

### R10. CLI And Observability Contract

`codex-router quota` is the operator truth surface. It must show cached state
first, refresh, then show updated state if refresh succeeds.

The human view must answer:

- which account will be selected next;
- why that account wins;
- which accounts are hard-blocked, held by 5h guard, unknown, stale, or lower
  priority;
- active sessions now per account with source and freshness;
- weekly runout/survival-to-reset;
- 5h guard state;
- refresh status and redacted auth/refresh failures.

The JSON view must expose stable machine fields for:

- selected account;
- selected reason code;
- weekly survival margin;
- weekly projected runout;
- 5h guard result;
- current active sessions;
- active-session source/freshness;
- burn-rate confidence;
- hard block reason.

Telemetry must use scrubbed dimensions and stable reason codes. It must not log
raw account ids, account labels, reservation ids, prompts, payloads, tokens,
provider bodies, or filesystem paths.

## Boundary / Separability Map

```text
Codex CLI
  owns: turn lifecycle, payload protocol, retries, WS fallback
  sees: router endpoint as model provider

        pass-through traffic, except router-owned auth/routing safety
        ▼

codex-router proxy
  owns: route classification, OAuth account injection, affinity, account
        selection, usage-limit containment, active session lifecycle events
  does not own: prompt/message/tool payload semantics

        typed selector input
        ▼

codex-router-selection
  owns: pure burn-rate estimation, survival-to-reset assessment, deterministic
        account choice, stable reason codes
  does not own: SQLite, HTTP/WebSocket protocol, provider credential handling

        SQLx DTOs and repositories
        ▼

codex-router-state
  owns: accounts, selector windows, quota history, refresh status, active
        session events/rollups, current leases, usage-limit account state,
        affinity owners

        scrubbed operator proof
        ▼

codex-router CLI / OTEL / Victoria
  owns: human status, JSON proof, local logs/traces/metrics
  does not own: alternate selection logic
```

## Selector Algorithm Contract

```text
for each candidate account:
  hard_filter(account)
  load current weekly + 5h windows
  load same-reset quota history
  load active session history and current active count
  estimate weekly burn and confidence
  estimate 5h burn and confidence
  compute weekly_survival_margin
  compute short_survival_margin
  classify:
    blocked
    unknown
    held_by_5h_guard
    weekly_survivor
    weekly_non_survivor

selection:
  if any weekly_survivor passing 5h guard:
    keep only the highest available burn-rate confidence tier
    group same-effective-weekly pools
    within each same pool:
      if active count difference >= active_session_imbalance_threshold:
        choose lower active session count
      else:
        choose earliest weekly reset
    across pools:
      choose earliest weekly reset
    tie: larger survival margin
    tie: fewer current active sessions
    tie: stable account order
  else if any non-survivor passing 5h guard:
    choose latest projected weekly runout
    tie: less negative survival margin
    tie: higher confidence
    tie: stable account id order
  else if only unknown candidates exist:
    choose unknown fallback only when no known usable account exists
  else:
    no usable account
```

## Required TDD Matrix

The implementation plan must start with pure selector and data-structure tests
before proxy or CLI changes.

All numeric rows use the v1 default policy constants from R0. Percentages in the
test implementation should be represented as basis points, so `0.5%/h` is
`50` basis-points-per-hour and must not round to `0` or `1%/h`.

| id | A | B | C | expected |
| --- | --- | --- | --- | --- |
| W1 | 20% weekly, reset 24h, burn 0.5%/h, 0 active | 34% weekly, reset 96h, burn 0.5%/h, 0 active | 80% weekly, reset 7d, burn 0.5%/h, 0 active | A wins: A survives soon reset; B and C burn before reset or preserve later reset only if all soon-reset accounts fail |
| W2 | 20%, reset 24h, burn 1.0%/h | 34%, reset 96h, burn 0.2%/h | 80%, reset 7d, burn 0.2%/h | B wins if B survives and A does not; C is preserved because B resets sooner |
| W3 | 60%, reset 48h, burn 0.5%/h | 70%, reset 96h, burn 0.5%/h | 90%, reset 7d, burn 0.5%/h | A wins: all survive, earliest reset wins |
| W4 | 20%, reset 24h, burn unknown, fresh quota | 34%, reset 96h, burn 0.2%/h | 80%, reset 7d, burn 0.2%/h | B wins: known survivor beats insufficient-burn A; C is far-reset reserve |
| W5 | 25%, reset crossed, prior 5%, latest 95% | 30%, same reset, burn 0.5%/h | none | B wins; A history before reset is ignored, not negative burn |
| W6 | 10%, reset 20h, runout 10h, margin -10% | 20%, reset 60h, runout 30h, margin -20% | none | B wins; non-survivor fallback uses latest projected runout before negative margin |
| W7 | 40%, reset 48h, burn 0.5%/h, lower confidence, 0 active | 38%, reset 48h, burn 0.5%/h, higher confidence, 0 active | 80%, reset 7d, burn 0.2%/h, higher confidence | B wins: confidence tier gates before survival-margin tie; C is preserved because B resets sooner |
| W8 | 21%, reset 24h, burn unknown, 0 active | 36%, reset 72h, burn 0.3%/h, 3 active | 80%, reset 7d, burn 0.3%/h, 0 active | B wins: known confidence beats A; active imbalance is only same-pool, so C is preserved as far-reset reserve |
| F1 | weekly healthy, 5h 2%, 5h reset 4h, 5h burn 1%/h | weekly lower, 5h 30%, 5h reset 4h, 5h burn 1%/h | none | B wins; A fails 5h guard |
| F2 | weekly healthy, 5h 2%, 5h reset 10m, 5h burn 1%/h | weekly lower, 5h 30%, 5h reset 4h | none | A can win if short reset safety buffer passes |
| A1 | 50%, reset 72h, raw burn 6%/h, avg 3 sessions, current 0 | 35%, reset 72h, raw burn 1%/h, avg 1 session, current 0 | none | B wins; A per-session projection still fails |
| A2 | 50%, reset 72h, raw burn 6%/h, avg 3 sessions, current 0 | 35%, reset 72h, raw burn 1%/h, avg 1 session, current 4 | none | A wins if B projected active sessions make B fail |
| A3 | same quota and history as B, active current 0 | same quota and history as A, active current 3 | none | A wins tie by fewer current active sessions |
| A4 | burn occurred, active session-hours zero | 40%, reset 48h, valid history | none | B wins or A low-confidence fallback; no divide-by-zero and no fake cost |
| A5 | 19%, reset 45h, burn 0%/h observed, current 6 | 18%, reset 46h, burn 0%/h observed, current 0 | 34%, reset 107h, burn 0.2%/h | B wins; same low-weekly reset pool must share new sessions before using far-reset reserve |
| A6 | same reset and same survival margin as B, higher confidence, current 4 | same reset and same survival margin as A, lower confidence, current 0 | none | A wins: confidence tier gates before active-count tie; active balancing does not make lower-confidence peers same-pool |
| T1 | same account inputs, request is HTTP/SSE | same account inputs, request is WebSocket | none | same selected account; transport kind cannot affect quota score |
| U1 | usage-limit active block, otherwise best | 25%, reset 24h, burn 0.5%/h | 80%, reset 7d, burn 0.5%/h | B wins; usage limit hard-blocks A |
| U2 | usage-limit block | usage-limit block | usage-limit block | no account; router all-accounts-exhausted |
| U3 | unknown quota | 25%, reset 24h, burn 0.5%/h | none | B wins; known survivor beats unknown |
| U4 | unknown quota | unknown quota | disabled | unknown fallback allowed; disabled excluded |
| C1 | affinity owner, retiring but not blocked | healthier new-work account | none | continuation stays on A; new work goes B |
| C2 | affinity owner, usage-limit hard blocked | healthier account | none | router uses Codex-safe reconnect/retry; A not reused |
| S1 | A would have old weight 1 | B strong survivor | C strong survivor | A never selected while failing survival/guard; no `score 1` fallback |
| S2 | active leases include legacy `active_pressure=8` | same with `active_pressure=2` | none | selected account unchanged; legacy pressure ignored |

## Data Proof Expectations

- Schema tests prove `active_session_events` retain completed sessions after
  current leases are released.
- Rollup tests prove session-seconds across overlapping sessions.
- Rollup tests prove partial-bucket clipping when quota observations do not
  align to `active_session_rollup_bucket_seconds`; expected active-session
  seconds and maximum concurrency are computed for the exact observation
  interval, not the whole buckets.
- Rollup tests prove stale-purged sessions contribute until purge time and
  re-reserved sessions remain one continuous interval.
- Run-rate tests prove same-reset quota observations and active session-hours
  produce the expected burn estimates.
- Migration tests prove a prior-version SQLx database with current
  `active_client_leases`, selector windows, and quota history gains the new
  `active_session_events` and `active_session_rollups` domains without losing
  current mirror state, without backfilling synthetic completed sessions, and
  without letting legacy active pressure affect selector output.
- Retention tests prove week-long quota history and active-session rollups are
  kept long enough for run-rate calculation and purged deterministically.

## Runtime Proof Expectations

- Proxy tests prove runtime selection and `codex-router quota` use the same
  selector input and produce the same selected account.
- SQLx fixture tests seed quota windows, quota history, active-session mirror,
  active-session rollups, and refresh status, then compare runtime selection
  with `codex-router quota --json` for selected account, reason code, active
  count, active source/freshness, and stale/unavailable labeling.
- HTTP/SSE precommit quota exhaustion retries another account without surfacing
  raw account exhaustion.
- HTTP/SSE precommit all-accounts-exhausted returns a router-level scrubbed
  error without leaking the final provider's raw usage-limit body.
- HTTP/SSE postcommit behavior remains pass-through unless a separate approved
  spec changes that boundary.
- WebSocket account exhaustion emits only the source-backed reconnect signal
  when alternatives remain, only after bounded Responses error-envelope
  detection, and unrelated WebSocket messages remain pass-through.
- WebSocket all-accounts-exhausted emits a router-level scrubbed error.
- Multi-session tests start at least six concurrent selection attempts across
  three accounts with controlled quota/history/session state and assert the
  selected account timeline.
- Installed-Codex smoke/e2e remains required for the reconnect path and the
  user-facing `codex-router quota` path before claiming the runtime slice works
  end to end.

## CLI And OTEL Proof Expectations

- `codex-router quota` fixture tests show cached output, refresh progress, and
  updated output.
- Human output does not show fake `score 1`, active pressure cost, or transport
  cost.
- JSON output includes active session count, active session source/freshness,
  weekly survival margin, 5h guard state, and reason codes.
- Stale or unavailable active-session mirror output explicitly labels the next
  account as not live-load exact.
- OTEL/Victoria proof can query account selections, rejections, active sessions,
  quota refresh outcomes, and usage-limit containment using scrubbed dimensions.
- Negative canaries prove logs/traces/metrics do not contain tokens, prompts,
  raw account ids, raw account labels, raw reservation ids, raw provider bodies,
  or raw filesystem paths.

## Post-V1 Tuning Questions

The constants and fallback behavior in this spec are fixed for the first
implementation and must be asserted by tests. Later live evidence may justify a
separate tuning spec for:

1. Adjusting safety buffers for weekly and 5h survival margins.
2. Adjusting the "near reset" threshold for the 5h guard.
3. Changing sparse active-session history fallback from aggregate burn with
   lower confidence to a stricter hold policy.
4. Adding a separate maximum active sessions per account. This spec does not
   add a cap; it only uses active sessions for burn projection and tie-breaks.
5. Adjusting TTL/clear conditions for usage-limit account state.
