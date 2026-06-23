# Reset-Aware Burn-Down Routing Spec

Date: 2026-06-23
Status: revised draft after spec-review findings
Scope: codex-router quota burn-down assessment, runtime account selection, and quota status explanation

## Product Intent

Codex-router should pick the next account using quota survivability, not only raw remaining quota. A user with several OAuth accounts needs the router to protect the account that would be stranded longest if depleted, while still using quota that is about to reset instead of wasting it.

Success means the router can answer these questions consistently:

- Which account is usable now?
- Which account is preferred for the next normal non-affinity request?
- Which window is limiting the account: 5h or weekly?
- Is low remaining quota dangerous, or is it safe because reset is imminent?
- Is the status display explaining the same burn-down reasoning runtime routing
  uses, while being honest about fairness and affinity overrides?

## Current-State Evidence

Observed in current code:

- Runtime selection reads route-band selector rows through the one-argument
  `SelectorQuotaRepository::selector_inputs_for_route_band`; this spec requires
  a hard cutover to the state-owned read overlay API with `now_unix_seconds`.
  Sources: `account_selection.rs:189-210`, `repositories.rs:46-59`.
- Current selector collapse loses reset geometry by reducing windows to minimum
  headroom after ineligible/effective checks. Source:
  `account_selection.rs:262-292`.
- `WeightedDeficitSelector` is generic weighted round-robin over
  `(AccountId, u32)` and must stay quota-semantics-free. Source:
  `weighted_deficit.rs:60-98`.
- Persisted selector windows already contain the needed raw facts:
  `limit_window_seconds`, `status`, `remaining_headroom`, `reset_unix_seconds`,
  `effective`, and `observed_unix_seconds`. Source:
  `quota_snapshot.rs:91-200`.
- CLI status already computes pace/runout from reset time and headroom. Source:
  `quota.rs:924-1007`.
- Current WebSocket routing already performs local-auth preflight, unsupported
  path rejection, and bounded first-frame validation before `/v1/responses`
  selection; the remaining delta is allowlisted first-frame evidence,
  redaction, and affinity-secret fail-closed ordering. Sources:
  `server.rs:334-363`, `server.rs:456-500`, `websocket.rs:158-217`,
  `websocket.rs:269-345`.
- Current `select_affinity_owner` requires owner membership in
  `weighted_candidates`; the target rule allows `usable`/`reserve`
  continuation owners outside the current selected pool, while still failing
  closed for blocked, unknown, excluded, stale-generation, or credential-invalid
  owners. Sources: `affinity.rs`, `repositories.rs:61-72`.
- Prior spec/plan already require weekly quota protection before short-window
  reset urgency and selection ordered by eligibility/freshness, long-window
  pressure, effective headroom, and bounded reset urgency.

## Requirements

R1. Startup and request routing must not block on provider quota refresh.

The selector uses persisted SQLite selector windows. Background refresh may update those windows, but startup and request selection do not wait on live provider I/O.

R2. Route-band-relevant exhausted windows block the account.

For the requested route band, an account is not normally selectable if any relevant quota window is `Ineligible` or has `remaining_headroom == 0`.

R3. Unknown quota is not free capacity.

Unknown accounts never compete with known `usable` or `reserve` accounts. If
only unknown accounts remain, partial headroom evidence may order the fallback
pool conservatively, but missing reset times receive no reset-salvage bonus.

R4. Weekly quota is durable budget in v1.

The v1 public quota contract is the observed 5h plus weekly window shape.
Weekly pressure dominates 5h reset urgency. An account with low weekly headroom
and a far weekly reset must not be preferred merely because its 5h window has
more headroom or resets sooner. Internally, code may keep generic
short-window/long-window helpers, but v1 user-facing status, examples, and
reason names are 5h/weekly.

R5. Soon-reset quota may be salvaged only inside a bounded rule.

Reset urgency may increase an account's weight when the account remains healthy under long-window pressure. It may not override dangerous long-window pressure unless the long-window reset itself is imminent enough that the durable-budget risk is about to disappear.

R6. Runtime selection and quota status must share assessment semantics.

The CLI may format for humans, but it must not reimplement a different definition of limiting window, pressure, or routing reason.

R7. Selection reasons must be structured enough to explain the decision.

The runtime cannot report only `fresh_quota`, `stale_quota_fallback`, or `unknown_quota_fallback`. It must expose an audit-safe reason derived from the same assessment used for routing.

R8. Generated Codex profile auth must match installed Codex behavior.

The generated Codex profile uses `env_key = "CODEX_ROUTER_TOKEN"` and Codex
therefore sends `Authorization: Bearer <token>` to the local router for
HTTP/SSE and WebSocket transports. The router accepts that bearer carrier for
the generated profile path and also accepts `X-Codex-Router-Token` as a
manual/compatibility ingress carrier. Query, cookie, WebSocket subprotocol, and
HTTP/SSE request-body token carriers remain forbidden through the same narrow
top-level JSON field-name check used for WebSocket first frames. The router
does not scan nested prompt text, tool arguments, metadata, or arbitrary string
values for token-like content.

R9. WebSocket routing must preselect from an allowlisted first-frame view.

The `/v1/responses` WebSocket route waits only for a bounded first frame, reads
only the minimum fields needed to classify an installed-Codex response-create
request and optional previous-response affinity, and then selects/pins an
account before opening upstream. Non-allowlisted first-frame/body values must
not affect route-band selection, logs, traces, audit records, or smoke
transcripts. A narrow top-level auth-smuggling field-name check rejects
token-carrier fields before selection without inspecting nested prompt/body
values.

R10. Route result and unknown-fallback semantics are first-class contracts.

`unknown` quota is a selected pool only after known `usable` and `reserve`
pools are empty. Assessment output must expose one canonical route-level
result shape for `ok` and `unsupported_route_band`, selected-pool reason,
preferred next account id, weighted candidates, and account rows so status,
runtime audit, and tests share one contract.

## Non-Goals

- No forecasting engine.
- No EWMA or historical usage model.
- No per-model token-cost estimation.
- No mid-stream account switching.
- No global optimization across future sessions.
- No live quota polling on the request path.
- No changes to `WeightedDeficitSelector` that make it know quota-window semantics.
- No live OAuth/keychain work in this burn-down routing goal.

## Spec Boundary / Separability Map

```text
provider quota refresh
  owns: provider fetch and normalization
  writes: persisted selector quota windows

persisted selector quota windows
  owns: last-known per-account, per-route-band, per-window quota facts
  exposes: SelectorQuotaRepository::selector_inputs_for_route_band(route_band, now_unix_seconds)
           and SelectorQuotaRepository::quota_refresh_statuses_for_route_band(route_band)

burn-down assessment
  owns: reset-aware pressure math, pure routability/exclusion classification
        from supplied facts, and structured explanation
  crate: codex-router-selection::burn_down
  inputs: BurnDownRouteBandAssessmentInput with core RouteBand
  exposes: BurnDownRouteBandAssessmentResult::ok(BurnDownRouteBandAssessment)
           or ::unsupported_route_band(UnsupportedRouteBandAssessment)
  assessment exposes: route_result, selected_pool, selected_pool_reason,
                      preferred_next_account_id, weighted_candidates, accounts,
                      account presentation fields, and safe route/status reason fields
  must not depend on: codex-router-state, codex-router-proxy, codex-router-cli

proxy account selection
  owns: route classification, account fact adaptation, previous-response
        affinity resolution/enforcement, process-lifetime fairness state, and
        runtime exact account choice
  adapts: Vec<SelectorQuotaInput> -> BurnDownRouteBandAssessmentInput
  consumes: BurnDownRouteBandAssessmentResult and
            BurnDownRouteBandAssessment.weighted_candidates
  exposes: RuntimeSelectedAccountDecision with the shared route assessment
           envelope plus runtime-only selected-account fields

weighted deficit selector
  owns: generic weighted fairness state
  consumes: (AccountId, u32)
  must not know: windows, weekly quota, reset time, CLI formatting

quota status CLI
  owns: human and machine rendering
  adapts: Vec<SelectorQuotaInput> -> BurnDownRouteBandAssessmentInput
  consumes: BurnDownRouteBandAssessmentResult, account presentation fields,
            route explanation, and neutral preferred-next projection
  must not own: routing math
  must not claim: runtime-exact next account after affinity or fairness state
```

Dependency contract:

- `codex-router-selection` owns the pure assessment module and may depend only on
  stable lower-level crates such as `codex-router-core` and
  `codex-router-quota`.
- `codex-router-selection` must not depend on `codex-router-state`, because
  persisted SQLite DTOs are storage concerns.
- `codex-router-state` remains the source of persisted selector rows and must
  not depend on selection, proxy, or CLI crates.
- `codex-router-proxy` owns the runtime adapter from `SelectorQuotaInput` to
  `BurnDownRouteBandAssessmentInput`, including supplying account admin and
  active-credential-generation facts. It then feeds the selected pool's
  positive scalar weights into `WeightedDeficitSelector`.
- `codex-router-selection` may classify a supplied account as excluded from
  routing from those facts, but it must not resolve credentials, read secret
  stores, or decide whether a credential can be refreshed.
- `codex-router-cli` may depend on `codex-router-selection` so status and
  routing share the same assessment output. It owns formatting only.
- Reimplementing pressure, reserve, unknown, or limiting-window math in the CLI
  or proxy is out of contract.
- Reimplementing `next_use`, public `routing_reason`, `window_slots`,
  `windows[]`, `salvage_tie_key`, or route-level unsupported-band handling in
  the CLI or proxy is out of contract. Selection owns those safe presentation
  fields because they are direct consequences of routing assessment.
- `codex-router-core::redaction` owns the shared `SafeAccountLabel` and
  deterministic redacted account hash/tag helper. Proxy, CLI, state-backed
  status adapters, traces/logs, and smoke transcript helpers must consume this
  shared helper instead of inventing surface-local account-label sanitizers.
- `codex-router-core::routes` owns the shared `RouteBand` identifier enum for
  all route bands that can enter quota assessment. Proxy route classification
  outputs this core type. Selection policy lookup accepts this core type. CLI
  status adapters use the same core type. Proxy and selection must not maintain
  parallel string-only route-band lists.

### Quota Refresh Lifecycle Contract

Refresh is background maintenance of persisted selector quota windows. It is not
part of the request critical path.

Normative lifecycle:

1. Server startup binds the loopback listener before any provider quota refresh
   attempt is awaited. Startup may schedule an immediate refresh task after the
   listener is ready, but failed or delayed provider I/O must not delay bind,
   readiness, first route-band assessment, or status rendering.
2. A periodic refresh task runs on the configured refresh interval. It fetches
   provider quota, normalizes windows, and writes selector rows through
   `codex-router-state`.
3. Refresh writes are per account and route band. A successful refresh replaces
   the account/route-band selector window set and records `observed_unix_seconds`.
4. A transient provider, auth, network, parse, or rate-limit refresh failure
   preserves the last successful selector rows. It records a redacted refresh
   error class in `quota_refresh_status` and marks the affected account/route
   band stale without deleting known quota evidence.
5. If no persisted rows exist for an account/route band, assessment receives no
   relevant rows and returns `unknown` with `needs_quota_refresh`; it does not
   synthesize `0% left`.
6. If a provider explicitly reports a window as ineligible or exhausted, refresh
   persists that status/headroom so assessment can return `blocked`; this is not
   treated as a transient failure.
7. If an account is disabled or lacks an active credential generation, the proxy
   and CLI adapters supply those administrative facts to assessment; refresh
   does not decide runtime credential availability.
8. Refresh errors, logs, traces, and status notes use safe account labels or
   hashes and redacted provider error classes only.

Ownership:

- `codex-router-proxy` owns startup orchestration, listener readiness, immediate
  refresh task scheduling, and request-path guarantee that selection reads
  persisted selector rows without awaiting provider refresh.
- `codex-router-quota` or the existing provider quota client owns provider fetch
  and normalization into route-band window facts.
- `codex-router-state` owns durable selector window writes, last-success rows,
  read-time stale overlay, and redacted refresh error metadata.
- `codex-router-selection::burn_down` owns interpretation of the supplied
  persisted facts as `fresh`, `stale`, `unknown`, `blocked`, `reserve`, or
  `usable`.

Persisted refresh state contract:

`codex-router-state` owns a small durable `quota_refresh_status` record keyed by
`account_id` plus `route_band`:

```text
QuotaRefreshStatus {
  account_id,
  route_band,
  last_success_unix_seconds: i64 | null,
  last_attempt_unix_seconds: i64 | null,
  last_error_class: provider_error | auth_error | network_error | parse_error | rate_limited | null,
  stale_after_unix_seconds: i64 | null
}
```

Refresh freshness policy:

- `QuotaRefreshFreshnessPolicy` is a refresh-worker input derived from the
  configured refresh interval.
- `stale_after_unix_seconds = last_success_unix_seconds +
  max(configured_refresh_interval_seconds * 2, 600)`.
- The minimum 600 second grace prevents short local test intervals from marking
  rows stale immediately; the two-interval rule marks rows stale after one
  missed periodic refresh at normal runtime intervals.
- The refresh worker computes and passes the exact stale-after timestamp into
  the repository success/failure operations. `codex-router-state` persists the
  timestamp and applies the read overlay below.

Successful refresh transaction:

- replaces that account/route-band selector window set
- sets `last_success_unix_seconds` and `last_attempt_unix_seconds`
- clears `last_error_class`
- computes `stale_after_unix_seconds` from the freshness policy above

Transient failed refresh transaction:

- preserves existing selector window rows exactly
- updates `last_attempt_unix_seconds`
- sets redacted `last_error_class`
- sets `stale_after_unix_seconds` to `min(existing stale_after_unix_seconds,
  now_unix_seconds)` so assessment can mark rows stale immediately without
  erasing last-known headroom/reset facts

The failed-refresh status update and row preservation are one repository
operation from the caller's perspective. Proof must show a failed refresh leaves
previous selector windows queryable, marks the account/route band stale or
needs-refresh through the assessment/status path, and exposes only the redacted
error class.

Repository operation contract:

- `record_refresh_success_and_replace_selector_windows(...)` atomically replaces
  the account/route-band selector windows and records refresh success metadata.
- `record_refresh_failure_preserving_selector_windows(...)` atomically preserves
  existing selector windows and records only redacted failure/staleness metadata.
- callers do not perform ad hoc selector-window deletion or stale marking across
  separate repository calls.

Refresh read-model contract:

- `codex-router-state` owns the read overlay that joins
  `selector_quota_windows` with `quota_refresh_status`.
- `selector_inputs_for_route_band(route_band, now_unix_seconds)` returns
  selector windows with last-known `remaining_headroom`, `reset_unix_seconds`,
  `observed_unix_seconds`, and `effective` preserved.
- If `now_unix_seconds >= stale_after_unix_seconds`, the repository overlays
  `QuotaWindowStatus::Stale` on otherwise eligible persisted rows for that
  account/route band. It does not mutate the stored selector-window row during
  read.
- If selector rows exist but the matching `quota_refresh_status` row is absent
  or has null `stale_after_unix_seconds` after upgrade, the first read treats
  those selector rows as `stale` and preserves last-known headroom/reset facts.
  The immediate background refresh may replace them with fresh rows, but
  request routing and status must not treat legacy rows with missing refresh
  metadata as fresh capacity.
- If no selector windows exist for an account/route band, the read model returns
  no windows; assessment classifies that account as `unknown` /
  `needs_quota_refresh`.
- `last_error_class` is not consumed by pure selection. It is exposed only by
  explicit status/log/proof DTOs that use redacted error classes and safe
  labels.
- The CLI status adapter may join selector inputs with refresh status metadata
  to display `needs refresh`/stale notes, but it must not recompute stale
  semantics outside the repository read model.

Refresh state transitions:

| Prior state | Refresh result | Persisted selector rows | Status/assessment consequence |
| --- | --- | --- | --- |
| no rows | success with eligible windows | replace with known windows | `fresh` known assessment |
| known rows | success with eligible windows | replace with new known windows | `fresh` known assessment |
| known rows | transient failure | preserve prior windows | `stale` known assessment plus redacted error class |
| legacy rows with missing refresh status | first post-upgrade read before refresh | preserve prior windows | `stale` known assessment plus missing-status refresh note |
| stale rows | success with eligible windows | replace with new known windows | `fresh` known assessment |
| no rows | transient failure | no selector rows | `unknown` / `needs_quota_refresh` |
| any rows | provider reports exhausted/ineligible | replace with blocked window facts | `blocked`, not transient failure |

## Burn-Down Assessment Contract

### Inputs

`BurnDownRouteBandAssessmentInput`:

- `route_band`
- `now_unix_seconds`
- `accounts: Vec<BurnDownAccountInput>`

`BurnDownAccountInput`:

- `account_id`
- `safe_account_label`
- `route_band`
- `windows: Vec<QuotaWindowFact>`
- `account_enabled: bool`
- `has_active_credential: bool`

`QuotaWindowFact`:

- `window_seconds`
- `status: QuotaWindowStatus`
- `remaining_headroom`, clamped to 0..100
- `reset_unix_seconds`
- `observed_unix_seconds`
- `effective`

`QuotaWindowStatus` is a pure assessment enum with the same string values as
persisted selector rows: `eligible`, `stale`, `unknown`, and `ineligible`.
Adapters translate from state DTO enums into this enum.

`BurnDownRouteBandPolicy` is fixed v1 behavior owned by
`codex-router-selection::burn_down`, not caller input and not operator
configuration. Plan creation may name constants and test fixtures, but must not
turn these into user-facing config unless a later spec changes the contract.

`codex-router-selection::burn_down` owns the v1 route-band policy registry,
keyed by `codex-router-core::routes::RouteBand`. Proxy and CLI call
`assess_route_band(input: BurnDownRouteBandAssessmentInput) ->
BurnDownRouteBandAssessmentResult`; they do not pass a policy and must not carry
independent route-band policy `match` logic.
V1 contains explicit policies for every currently classified route band:

- `responses`: quota-managed 5h plus weekly public quota policy
- `responses_compact`: quota-managed 5h plus weekly public quota policy
- `models`: quota-managed 5h plus weekly public quota policy
- `memories_trace_summarize`: quota-managed 5h plus weekly public quota policy

Unknown or unregistered route bands return
`BurnDownRouteBandAssessmentResult::unsupported_route_band(UnsupportedRouteBandAssessment)`
before account assessment or weighted selection. The unsupported branch payload
is:

```text
UnsupportedRouteBandAssessment {
  route_band,
  route_result: unsupported_route_band,
  selected_pool: none,
  selected_pool_reason: unsupported_route_band,
  preferred_next_account_id: null,
  weighted_candidates: [],
  accounts: []
}
```

Proxy, CLI, status, tests, and smoke proof must consume that same route-level
result; none of them may keep a separate unsupported-band branch with different
route-band strings or reason names. HTTP/SSE and WebSocket requests fail
locally with machine reason `unsupported_route_band` before selector
advancement, credential resolution, upstream auth injection, or upstream open.
JSON machine output for an explicit unsupported route-band status request emits
the payload above and no account rows. Default human quota status renders the
user `responses` quota route only; it must not emit per-route rows for the other
route bands unless a future explicit debug or multi-route mode changes the
contract.

Route-band drift guard:

- proxy route-classification tests must enumerate every `RouteBand` variant and
  prove each routed variant has a selection policy
- selection policy tests must enumerate every `RouteBand` variant and prove
  lookup returns either an explicit policy or `unsupported_route_band`
- no proxy-only route-band string may reach `BurnDownRouteBandAssessmentInput`

Fixed v1 policy:

- `short_window_cutoff_seconds = 86_400`
- `short_near_reset_seconds = min(1_800, window_seconds / 10)`
- `long_near_reset_seconds = min(43_200, window_seconds / 10)`
- `reserve_pressure_threshold = 25`
- `reserve_headroom_threshold = 10`
- `long_pressure_multiplier = 3`
- `short_salvage_cap = 10`
- `long_salvage_cap = 20`
- `risk_penalty_cap = 90`
- `selectable_weight_min = 1`
- `selectable_weight_max = 100`
- stale penalty applies only inside the selected availability pool:
  stale accounts divide by 4 when a fresh account exists in the same selected
  pool; after division, clamp again to `selectable_weight_min..selectable_weight_max`
- unknown accounts are fallback-only and do not use the legacy same-pool
  `unknown / 8` penalty in v1

Rationale:

- The 5h near-reset threshold is small enough to prevent burning a short window
  early while still using quota that is about to expire.
- The weekly near-reset threshold is 12h for v1. Weekly salvage is allowed only
  when the reset is effectively same-day, because draining weekly quota strands
  the account for much longer.
- Thresholds are intentionally fixed in v1 so tests, status explanations, and
  routing behavior agree. Configuring them before live evidence would add
  another source of truth.

Window classification:

- `short_window`: window is shorter than 24h. The known 5h window is short.
- `long_window`: window is at least 24h, or the longest relevant window when no explicit weekly label exists. The known 604800 second window is weekly.
- `effective_window`: provider-selected or router-selected limiting row, but assessment must still inspect every relevant window.

The `effective` marker is an explanation hint, not the authority for freshness
or eligibility. If no row is marked effective, the assessment still classifies
the account from all relevant windows and chooses `limiting_window` from the
worst pressure, then lowest headroom, then longest window.

### Per-Window Math

For each relevant window with known reset:

```text
time_left_seconds = clamp(reset_unix_seconds - now_unix_seconds, 0, window_seconds)
expected_remaining_percent = ceil(100 * time_left_seconds / window_seconds)
pace_margin_percent = remaining_headroom - expected_remaining_percent
pressure_percent = max(0, -pace_margin_percent)
surplus_percent = max(0, pace_margin_percent)
```

Meaning:

- `pressure_percent > 0`: the account is burning faster than the reset budget for this window.
- `surplus_percent > 0`: the account has quota that is safe to spend before reset.
- A low 5h remaining value is not automatically bad if reset is very soon.
- A low weekly remaining value is bad when the weekly reset is far away.

For windows with missing reset:

- set `reset_known = false`
- set `pressure_percent` to `reserve_pressure_threshold` for conservative
  unknown-pool ordering only
- set the account-level evidence state to `unknown` unless a blocking condition
  already applies
- do not grant reset-salvage bonus

### Account-Level Collapse

The assessment first applies account and window collapse before weight math.
`quota_evidence_reason` records raw quota evidence before route-band pool
selection. Final `routing_reason` is assigned only after selected-pool/public
mapping, so unknown fallback rows can render `fallback` without losing the raw
reason that quota evidence is incomplete.

1. If the account is not enabled, return `availability=excluded`,
   `routing_exclusion=disabled`, and omit it from selected pools. This is not a
   quota availability class, but it is still returned in `accounts` so status
   can render it without reimplementing eligibility.
2. If there is no active credential generation, return `availability=excluded`,
   `routing_exclusion=missing_credential`, and omit it from selected pools. This
   is not a quota availability class, but it is still returned in `accounts`.
3. If there are no relevant route-band windows, return `unknown` with
   `routing_weight = 1` and `quota_evidence_reason=needs_quota_refresh`.
4. In v1, the expected public response quota shape is one 5h window
   (`window_seconds=18_000`) and one weekly window
   (`window_seconds=604_800`). If only one of those expected windows is present
   for the route band, return `unknown` with
   `quota_evidence_reason=missing_expected_window`; the present window may
   provide conservative partial-headroom ordering inside the all-unknown pool,
   but the account must not compete with known `usable` or `reserve` accounts.
5. If any relevant window is `Ineligible`, return `blocked` with
   `quota_evidence_reason=window_ineligible`.
6. If any relevant window has `remaining_headroom == 0`, return `blocked` with
   `quota_evidence_reason=window_exhausted`.
7. If any relevant window is `Unknown`, return `unknown` with
   `quota_evidence_reason=unknown_quota_window`.
8. If any relevant non-blocked window has no reset time, return `unknown` with
   `quota_evidence_reason=missing_reset_time`.
9. If at least one relevant window is `Stale` and none are blocked or unknown,
   compute burn-down normally and mark freshness as `stale`.
10. If every relevant window is `Eligible`, compute burn-down normally and mark
   freshness as `fresh`.

Freshness collapse is any-window conservative:

- one stale window makes the account stale
- one unknown or missing-reset window makes the account unknown
- one missing expected v1 5h or weekly window makes the account unknown
- one ineligible or exhausted window blocks the account
- the `effective` marker never overrides a worse relevant window

### Availability Classes

The assessment returns exactly one availability class:

- `excluded`: account is disabled or lacks an active credential generation; it
  is returned for status only and never enters any selected pool.
- `blocked`: at least one relevant window is ineligible or exhausted.
- `reserve`: account is not exhausted, but long-window pressure is dangerous enough that it should be used only when no normal account is available.
- `usable`: account can be selected normally.
- `unknown`: account has insufficient quota evidence and is selectable only as a fallback when no known usable or reserve account exists.

Default reserve conditions:

- any long window has `pressure_percent >= 25`, and its reset is not within the long-window near-reset threshold
- or any long window has `remaining_headroom <= 10`, and its reset is not within the long-window near-reset threshold

Near-reset thresholds are fixed v1 policy values, not hard-coded provider
facts:

- 5h near reset: at most 30 minutes, or 10% of the window, whichever is smaller
- weekly near reset: at most 12 hours, or 10% of the window, whichever is smaller

Long-window reserve is waived only for the specific long window whose reset is
within the long near-reset threshold. A different long window that is still far
from reset can still put the account in reserve.

### Routing Weight

The route-band assessment owns cross-account pool choice and neutral
preferred-next projection. Per-account assessment does not know about sibling
accounts.

`BurnDownRouteBandAssessment`:

- `route_result: ok`
- `route_band`
- `accounts: Vec<BurnDownAccountAssessment>`
- `selected_pool: usable | reserve | unknown | none`
- `selected_pool_reason: usable_available | reserve_only | unknown_fallback_only | none_available | unsupported_route_band`
- `weighted_candidates: Vec<(AccountId, u32)>`
- `preferred_next_account_id: Option<AccountId>`
- `accounts_order: account_id ascending`
- `weighted_candidates_order: neutral selector order`

Unsupported route-band payload:

```text
BurnDownRouteBandAssessmentResult {
  route_result: unsupported_route_band
  route_band
  selected_pool: none
  selected_pool_reason: unsupported_route_band
  preferred_next_account_id: null
  weighted_candidates: []
  accounts: []
}
```

Supported and unsupported route bands return the same route-level inventory:
`route_result`, `route_band`, `selected_pool`, `selected_pool_reason`,
`preferred_next_account_id`, `weighted_candidates`, and `accounts`. Supported
route bands use `route_result=ok`; unsupported route bands use
`route_result=unsupported_route_band`, `selected_pool=none`,
`selected_pool_reason=unsupported_route_band`, null preferred account, and empty
candidate/account arrays. Unsupported route bands never enter weighted
selection, never advance selector state, and never synthesize per-account rows
from unclassified request data.

`BurnDownAccountAssessment`:

- `account_id`
- `safe_account_label`
- `availability: usable | reserve | blocked | unknown | excluded`
- `freshness: fresh | stale | unknown`
- `routing_exclusion: none | disabled | missing_credential`
- `next_use: preferred | available | held | blocked | fallback`
- `limiting_window`
- `quota_evidence_reason`
- `short_pressure`
- `long_pressure`
- `short_salvage`
- `long_salvage`
- `salvage_tie_key`
- `routing_weight`
- `routing_reason`
- `preferred_next`
- `window_slots.{5h,weekly}`
- `windows[]`

The assessment output intentionally includes the safe presentation fields used
by default table/plain and JSON output. Status renderers may choose layout,
bar glyphs, column widths, and color, but they must not derive different
`next_use`, `routing_reason`, limiting-window, slot state, pressure, salvage, or
fallback semantics from raw provider rows.

Only `usable` accounts enter the normal weighted-deficit pool. If no usable
account exists, `reserve` accounts may enter. `blocked` accounts never enter.
`unknown` accounts enter only when no known usable or reserve account exists.

Pool order is normative:

1. Build assessments for every supplied route-band account fact row, including
   disabled accounts and accounts without an active credential generation.
2. `excluded` and `blocked` assessments remain in `accounts` for status, JSON,
   logs, and proof, but never enter `weighted_candidates`.
3. If one or more `usable` accounts exist, select only from `usable`.
4. Else if one or more `reserve` accounts exist, select only from `reserve`.
5. Else if one or more `unknown` accounts exist, select only from `unknown`.
6. Else return no eligible account.

Within the selected pool, candidates are ordered before they are passed to
`WeightedDeficitSelector`. This order is part of the neutral selector contract:

1. higher `routing_weight`
2. lower `long_pressure`
3. lower `short_pressure`
4. salvage tie key
5. `account_id` ascending

Tests that need a deterministic selected winner use this same order and an
empty selector state. `preferred_next_account_id` must equal the account
selected by this exact neutral selector contract.

`accounts[]` and `weighted_candidates[]` deliberately use different ordering
contracts:

- `accounts[]` is status/audit inventory and is always sorted by
  `account_id` ascending.
- `weighted_candidates[]` is selector input and is always sorted by the neutral
  selector order above.

Any proof or JSON assertion that says "canonical account id order" applies only
to `accounts[]` unless it explicitly says `weighted_candidates[]` tie fallback.

`salvage tie key` is exact and deterministic:

1. accounts with positive `short_salvage + long_salvage` sort before accounts
   with no positive salvage
2. lower `reset_unix_seconds` among windows that contributed positive salvage
3. lower `window_seconds` for that contributing window
4. `account_id` ascending

Accounts with no positive salvage have no salvage reset key and sort after any
account with positive salvage. This tie key must be derived inside the pure
assessment and used consistently by status, tests, and the proxy adapter; it is
not displayed in the default human table.

For each selectable `usable` or `reserve` account:

```text
usable_headroom = min(remaining_headroom across relevant windows)
long_pressure = max(pressure_percent across long windows)
short_pressure = max(pressure_percent across short windows, or 0)
short_salvage = min(short_salvage_cap, max surplus_percent across near-reset short windows)
long_salvage = min(long_salvage_cap, max surplus_percent across near-reset long windows)
risk_penalty = min(risk_penalty_cap, (long_pressure_multiplier * long_pressure) + short_pressure)

risk_adjusted_weight =
  usable_headroom
  - risk_penalty
  + short_salvage
  + long_salvage
```

Then:

- clamp `risk_adjusted_weight` to `1..100` for selectable accounts
- apply stale penalties after burn-down scoring only inside the selected pool:
  - stale with a fresh alternative in the same selected pool: divide by 4 using
    integer floor division
  - after division, clamp again to `1..100`
- pass `(AccountId, risk_adjusted_weight)` into `WeightedDeficitSelector`

For each selectable `unknown` account:

- preserve the `quota_evidence_reason` from account-level collapse
- unknown candidates never receive reset-salvage bonus
- if at least one known relevant window has headroom, compute
  `routing_weight = clamp(min_known_headroom - reserve_pressure_threshold, 1..100)`
- if no usable partial headroom exists, use `routing_weight = 1`
- unknown candidates are ordered by the same neutral selector contract:
  `routing_weight` descending, then lower pressure, then `account_id` ascending
- unknown never competes with usable or reserve accounts in v1

`preferred_next_account_id` semantics:

- `preferred_next_account_id` is a neutral projection from the pure assessment
  using an empty weighted-deficit state and no previous-response affinity key.
- It answers "which account is preferred for the next normal non-affinity
  request if fairness has no accumulated deficit?"
- It is computed from the exact `weighted_candidates` order given to
  `WeightedDeficitSelector`, so neutral status and neutral runtime selection
  cannot disagree on equal weights.
- It is not the runtime-exact next request. The proxy may choose a different
  account when previous-response affinity is present or when accumulated
  weighted-deficit state rotates to a lower-weight account for fairness.
- Default status must label this as `preferred next`, not `selected next`.
- Runtime audit may additionally log the actual selected account after affinity
  and weighted-deficit selection.

Runtime selection wrapper:

```text
RuntimeSelectedAccountDecision {
  assessment: BurnDownRouteBandAssessmentResult,
  selected_account_id,
  decision_reason:
    previous_response_affinity
    | account_hold_cooldown
    | preferred_next
    | available
    | fallback,
  assessment_selected_pool: usable | reserve | unknown | none,
  actual_selected_from_weighted_deficit: bool
}
```

The pure assessment decides pool, weights, and neutral preference. The proxy may
wrap that assessment with runtime-only state such as previous-response affinity,
connection/account hold cooldown, or accumulated weighted-deficit fairness.
Runtime wrappers must not alter per-account quota classification.
Proxy, audit, log, and test surfaces consume this DTO and may render subsets,
but must not reconstruct `route_result`, `selected_pool`,
`selected_pool_reason`, `preferred_next_account_id`, `weighted_candidates`, or
account rows from raw quota windows after selection.

Cooldown and pinning contract:

- selected pool is computed before any cooldown or previous-response pin is
  considered
- a cooldown/hold can reuse an account only if that account is still present in
  the current assessment's `weighted_candidates`
- previous-response affinity is a continuation-correctness override, not a
  fairness/cooldown hold; it may reuse an owner when that owner is still
  `availability=usable` or `availability=reserve`, even if the owner is outside
  the current selected pool's `weighted_candidates`
- previous-response affinity fails closed before weighted fallback when the
  owner is `unknown`, `blocked`, `excluded`, disabled, missing active
  credential, exhausted, ineligible, or stale for credential generation
- `blocked`, `excluded`, exhausted, ineligible, and missing-credential accounts
  never survive a hold or affinity pin
- a selected-pool change breaks a hold when the held account is not in the new
  pool
- when `selected_pool=unknown`, a hold may reuse only an account still in the
  unknown fallback pool; known-pool holds do not keep an account alive after the
  route falls to unknown-only evidence
- a successful previous-response affinity hit does not advance
  `WeightedDeficitSelector` state, because no weighted fallback occurred
- a successful previous-response affinity hit refreshes the route-band
  connection/account hold for that selected account and selected route band, so
  the continuation remains pinned unless the owner later becomes blocked,
  excluded, unknown, stale-generation, credential-invalid, or unsupported
- when affinity reuses a reserve owner outside the current selected pool, the
  hold is recorded for runtime continuity only; it must not move the reserve
  owner into `weighted_candidates` or change the pure assessment envelope

Tie and determinism contract:

- The runtime keeps `WeightedDeficitSelector` as the fairness state. Its
  accumulated deficits may select a lower-weight account occasionally to
  preserve smooth weighted fairness.
- Deterministic assessment tests compare `routing_weight`, availability,
  limiting window, route-level `preferred_next_account_id`, per-account
  `preferred_next`, and reason codes, not only the final selector output.
- Integration tests that need a selected winner start the weighted selector
  from empty state and provide candidates in the neutral selector order above.

### Worked Scoring Examples

The examples use the fixed v1 policy and assume fresh windows, known resets, and
an empty weighted selector.

Scenario A:

```text
A: 5h 5% left, resets in 2m; weekly 80% left, resets in 5d
B: 5h 90% left, resets in 4h; weekly 20% left, resets in 5d

A:
  short expected = 1, short surplus = 4, short salvage = 4
  long expected = 72, long surplus = 8, long salvage = 0
  usable_headroom = 5, long_pressure = 0, short_pressure = 0
  routing_weight = 5 + 4 = 9

B:
  long expected = 72, long pressure = 52
  weekly reset is not near, so availability = reserve
```

Expected: A is selected from the `usable` pool. B is held in `reserve`.

Scenario B:

```text
A: 5h 5% left, resets in 2m; weekly 80% left, resets in 5d
B: 5h 90% left, resets in 4h; weekly 20% left, resets in 10m

A routing_weight = 9, availability = usable

B:
  short expected = 80, short surplus = 10, short salvage = 0 because reset is not near
  long expected = 1, long surplus = 19, long salvage = 19
  usable_headroom = 20, long_pressure = 0, short_pressure = 0
  routing_weight = 20 + 19 = 39
```

Expected: B outranks A and is selected from an empty weighted selector.

Scenario D:

```text
A: 5h 30% left, resets in 10m; weekly 60% left, resets in 3d
B: 5h 30% left, resets in 4h; weekly 60% left, resets in 3d

A:
  short expected = 4, short surplus = 26, short salvage = 10
  long expected = 43, long surplus = 17, long salvage = 0
  usable_headroom = 30, long_pressure = 0, short_pressure = 0
  routing_weight = 30 + 10 = 40

B:
  short expected = 80, short pressure = 50, short salvage = 0
  long expected = 43, long surplus = 17, long salvage = 0
  usable_headroom = 30, long_pressure = 0, short_pressure = 50
  routing_weight = clamp(30 - 50, 1..100) = 1
```

Expected: A has higher `routing_weight` than B and is selected from an empty
weighted selector.

## Required Scenario Contracts

### Scenario A: low 5h, healthy weekly, 5h reset soon

```text
A: 5h 5% left, resets in 2m; weekly 80% left, resets in 5d
B: 5h 90% left, resets in 4h; weekly 20% left, resets in 5d
```

Expected: A is selected from the `usable` pool. B is held in `reserve`.

Reason: B's weekly pressure is durable-budget risk. A's low 5h is acceptable because the short window resets soon and weekly is healthy.

### Scenario B: low weekly, weekly reset soon

```text
A: 5h 5% left, resets in 2m; weekly 80% left, resets in 5d
B: 5h 90% left, resets in 4h; weekly 20% left, resets in 10m
```

Expected: B outranks A when the weighted selector starts with empty deficit
state.

Reason: B's low weekly quota is near reset, so the durable-budget risk is about to disappear. This is the bounded long-window salvage case.

Normative result: B outranks A when the selector starts with empty deficit
state, as shown in the worked scoring example.

### Scenario C: weekly empty

```text
A: 5h 80% left; weekly 0% left
B: 5h 42% left; weekly 42% left
```

Expected: A is blocked for the route band until weekly reset; B is selected if otherwise eligible.

### Scenario D: same weekly, different short reset pressure

```text
A: 5h 30% left, resets in 10m; weekly 60% left, resets in 3d
B: 5h 30% left, resets in 4h; weekly 60% left, resets in 3d
```

Expected: A receives higher `routing_weight` than B and outranks B when the
weighted selector starts with empty deficit state.

Reason: A has near-reset short-window quota that is safe to spend. B is under more short-window pressure because 30% must last much longer.

### Scenario E: unknown versus known healthy

```text
A: known fresh 50% 5h, 50% weekly
B: unknown 90% 5h, unknown weekly, missing reset evidence
```

Expected: A is selected from the `usable` pool. B is not selected because
`unknown` is fallback-only while any known `usable` or `reserve` account exists.

Reason: unknown quota is not free capacity when known healthy quota exists.

## User-Visible Status Contract

The human quota status view should answer "what can I use now?" without leaking internal score math.

Default `--format table` human output must be account-centric, with one logical
row per account. A logical row may render as two physical terminal lines so 5h
and weekly quota can be shown together without duplicating the account/status
cells. Continuation lines must leave account/status blank. Expanded/debug human
output may show at most two logical rows per account.

For v1, the default human table shows exactly one short-window slot and one
long-window slot per route band:

- `5h` displays the shortest relevant known short window, or the short window
  with the worst pressure when more than one short window exists
- `weekly` displays the longest relevant known long window, or the long window
  with the worst pressure when more than one long window exists
- additional relevant windows are summarized in the displayed slot text and are
  available in JSON

`--format plain` is a colorless human fallback for terminals without Unicode or
box drawing. It is not a machine format. It uses the same logical rows and
content rules as the default table, replaces Unicode bars with ASCII bars, and
does not expose account id, raw score, tokens, or auth material.

`--format json` is the explicit machine/debug format. It may include raw
`account_id` for local scripts, but default logs, smoke transcripts, and human
output must not copy raw `account_id` unless they are explicitly redacted or
hashed. Raw local JSON stdout is allowed to contain `account_id`; any persisted
or shared artifact that captures JSON output, including logs, traces, smoke
transcripts, PR evidence, and review attachments, must redact or hash
`account_id`.

Default columns:

- `account`
- `status`
- `5h`
- `weekly`
- `routing`
- `next use`

Column semantics:

- `account`: safe display label or hash
- `status`: account admin status such as `enabled` or `disabled`; routing
  usefulness must appear in `5h`, `weekly`, `routing`, and `next use`
- `5h`: short-window bar, percent left, reset timing, and short-window note
- `weekly`: long-window bar, percent left, reset timing, and long-window note
- `routing`: stable reason phrase from the route-band assessment
- `next use`: one of `preferred`, `available`, `held`, `blocked`, or
  `fallback`

Wording:

- use `left`, never ambiguous bare percent
- avoid `pp`
- avoid `bottleneck` in default output
- use `limiting window`, `weekly pressure`, `5h pressure`, `preferred next`, `held`, `blocked`, `needs refresh`
- show Unicode bars in the Rust app's human table when the terminal supports them
- use label/tag only for `account`; do not show `account_id` in default human output
- when routing choice is shown, include why the preferred account is next

Bar rendering:

- table: `█` for filled segments and `░` for empty segments
- plain: `#` for filled segments and `-` for empty segments
- both modes include a numeric percent with `left` only when headroom is known,
  for example `54% left`
- unknown or absent headroom must not render as `0% left`; table mode uses
  `░░░░░░░░░░ unknown` or `░░░░░░░░░░ no data`, and plain mode uses
  `---------- unknown` or `---------- no data`
- known headroom with missing reset time may render a bar and percent, but its
  second line must say `reset unknown`

Normative vocabulary:

| Term | Human meaning | Machine field |
| --- | --- | --- |
| `usable` | can be selected normally | `availability=usable` |
| `reserve` | held while a usable account exists | `availability=reserve` |
| `blocked` | cannot be selected for this route band | `availability=blocked` |
| `unknown` | needs refresh; fallback only | `availability=unknown` |
| `excluded` | not routable because account is disabled or has no active credential | `availability=excluded` |
| `limiting window` | the 5h or weekly window driving the decision | `limiting_window` |
| `pressure` | quota is being spent faster than reset pace | `pressure_percent` |
| `preferred next` | this account is the neutral next normal candidate, before affinity or accumulated fairness state | `preferred_next=true` |
| `available` | selectable in the current pool, but not the neutral preferred row | `preferred_next=false, availability=usable or reserve, selected_pool matches availability` |
| `held` | usable only after a higher-priority pool is empty | `preferred_next=false, availability lower than selected_pool` |
| `fallback` | selectable only because every known usable or reserve pool is empty | `selected_pool=unknown` |

Stable routing reason enum:

| Enum | Default human phrase |
| --- | --- |
| `preferred_weekly_healthier` | `preferred next: weekly healthier` |
| `preferred_weekly_reset_soon` | `preferred next: weekly reset soon` |
| `preferred_short_reset_soon` | `preferred next: 5h reset soon` |
| `preferred_highest_weight` | `preferred next: safest quota` |
| `available_same_pool` | `available: same pool` |
| `held_reserve` | `held: reserve` |
| `held_unknown` | `held: needs refresh` |
| `unknown_fallback_preferred` | `fallback: needs refresh` |
| `unknown_fallback_available` | `fallback: same unknown pool` |
| `excluded_disabled` | `blocked: disabled` |
| `excluded_missing_credential` | `blocked: missing credential` |
| `blocked_window_exhausted` | `blocked: quota empty` |
| `blocked_window_ineligible` | `blocked: quota ineligible` |
Raw quota evidence reasons such as `unknown_quota_window`,
`missing_reset_time`, `missing_expected_window`, and `needs_quota_refresh`
remain in `quota_evidence_reason`. They are not public `routing_reason` values.
Human renderers show `needs refresh` in the window slot or phrase, while
`routing_reason` stays tied to pool status: `held_unknown`,
`unknown_fallback_preferred`, or `unknown_fallback_available`.

Public reason mapping:

| Assessment outcome | Public `routing_reason` | Human phrase | `next use` |
| --- | --- | --- | --- |
| preferred usable account with healthier weekly pressure | `preferred_weekly_healthier` | `preferred next: weekly healthier` | `preferred` |
| preferred usable account because weekly reset is near | `preferred_weekly_reset_soon` | `preferred next: weekly reset soon` | `preferred` |
| preferred usable account because 5h reset is near | `preferred_short_reset_soon` | `preferred next: 5h reset soon` | `preferred` |
| preferred account by neutral selector weight | `preferred_highest_weight` | `preferred next: safest quota` | `preferred` |
| non-preferred account in the selected pool | `available_same_pool` | `available: same pool` | `available` |
| reserve account while a usable pool exists | `held_reserve` | `held: reserve` | `held` |
| unknown account while usable or reserve pool exists | `held_unknown` | `held: needs refresh` | `held` |
| preferred unknown account when selected pool is unknown | `unknown_fallback_preferred` | `fallback: needs refresh` | `fallback` |
| non-preferred unknown account when selected pool is unknown | `unknown_fallback_available` | `fallback: same unknown pool` | `fallback` |
| disabled account | `excluded_disabled` | `blocked: disabled` | `blocked` |
| account with no active credential generation | `excluded_missing_credential` | `blocked: missing credential` | `blocked` |
| exhausted relevant window | `blocked_window_exhausted` | `blocked: quota empty` | `blocked` |
| ineligible relevant window | `blocked_window_ineligible` | `blocked: quota ineligible` | `blocked` |

Reason precedence is deterministic. Public `routing_reason` is assigned after
availability pool selection in this order:

1. `excluded` and `blocked` evidence reasons map to their exact public reason
   before any preferred-account reason.
2. Unknown accounts outside the selected pool map to `held_unknown`.
3. Reserve accounts outside the selected pool map to `held_reserve`.
4. Non-preferred accounts inside an `unknown` selected pool map to
   `unknown_fallback_available`, preserving the raw `quota_evidence_reason`
   separately.
5. The preferred account inside an `unknown` selected pool maps to
   `unknown_fallback_preferred`, preserving the raw `quota_evidence_reason`
   separately.
6. Non-preferred accounts inside a known selected pool map to
   `available_same_pool`.
7. The preferred account in a known selected pool maps to
   `preferred_weekly_reset_soon` when `long_salvage > 0`.
8. Else, the preferred account in a known selected pool maps to
   `preferred_weekly_healthier` when its `long_pressure` is strictly lower than
   at least one other known selected-pool candidate, or when another supplied
   known account was held in `reserve` because of long-window pressure.
9. Else, the preferred account maps to `preferred_short_reset_soon` when
   `short_salvage > 0`.
10. Else, the preferred account maps to `preferred_highest_weight`.

When multiple preferred-account predicates are true, this precedence wins. The
runtime audit, table/plain status, JSON status, and tests must use the same
reason-selection function.

Default human output must not contain:

- `account_id`
- raw internal score
- `pp`
- `bottleneck`
- provider token, router token, keychain identifier, or upstream auth header
- `router_affinity_hash_secret`

Example shape:

```text
account  status   5h                         weekly                     routing                         next use
askluna  enabled  ██████████ 100% left        ░░░░░░░░░░ 0% left          blocked: quota empty             blocked
                  resets in 4h 55m            resets in 1d 11h
matches  enabled  █████████░ 91% left         █████░░░░░ 54% left         preferred next: weekly healthier preferred
                  resets in 4h 8m             resets in 5d 22h
ssdev    enabled  ██████████ 100% left        ██░░░░░░░░ 16% left         held: reserve                    held
                  resets in 3h 48m            resets in 1d 9h
```

JSON output schema:

```text
{
  "route_result": "ok | unsupported_route_band",
  "route_band": "responses",
  "selected_pool": "usable | reserve | unknown | none",
  "selected_pool_reason": "usable_available | reserve_only | unknown_fallback_only | none_available | unsupported_route_band",
  "preferred_next_account_id": "acct_..." | null,
  "weighted_candidates": [
    { "account_id": "acct_...", "routing_weight": 1..100 }
  ],
  "accounts": [
    {
      "account_id": "acct_...",
      "safe_account_label": "askluna",
      "availability": "usable | reserve | blocked | unknown | excluded",
      "freshness": "fresh | stale | unknown",
      "routing_exclusion": "none | disabled | missing_credential",
      "next_use": "preferred | available | held | blocked | fallback",
      "limiting_window": "5h | weekly | unknown | none",
      "quota_evidence_reason": "needs_quota_refresh | missing_expected_window | window_ineligible | window_exhausted | unknown_quota_window | missing_reset_time | none",
      "short_pressure": 0..100 | null,
      "long_pressure": 0..100 | null,
      "short_salvage": 0..100 | null,
      "long_salvage": 0..100 | null,
      "salvage_tie_key": { "reset_unix_seconds": 0, "window_seconds": 18000 } | null,
      "routing_reason": "preferred_weekly_healthier | preferred_weekly_reset_soon | preferred_short_reset_soon | preferred_highest_weight | available_same_pool | held_reserve | held_unknown | unknown_fallback_preferred | unknown_fallback_available | excluded_disabled | excluded_missing_credential | blocked_window_exhausted | blocked_window_ineligible",
      "routing_weight": 1..100 | null,
      "preferred_next": true | false,
      "window_slots": {
        "5h": { "slot": "5h", "evidence_state": "known | unknown | no_data", "remaining_headroom": 0..100 | null, "reset_unix_seconds": 0 | null, "reset_duration_seconds": 18000 | null, "display_note": "string" },
        "weekly": { "slot": "weekly", "evidence_state": "known | unknown | no_data", "remaining_headroom": 0..100 | null, "reset_unix_seconds": 0 | null, "reset_duration_seconds": 604800 | null, "display_note": "string" }
      },
      "windows": [
        { "window_seconds": 18000, "status": "eligible | stale | unknown | ineligible", "remaining_headroom": 0..100 | null, "reset_unix_seconds": 0 | null, "observed_unix_seconds": 0 | null, "effective": true | false, "pressure_percent": 0..100 | null, "surplus_percent": 0..100 | null, "contributed_to_salvage": true | false }
      ]
    }
  ]
}
```

JSON output may expose scores. Default table/plain output must not. The JSON
schema must use stable enums for availability, limiting window, freshness, and
routing reason. Route-level fields are top-level. Per-account fields live only
inside `accounts[]`, except that `weighted_candidates[]` repeats local
`account_id` plus `routing_weight` so scripts can reproduce selector inputs.
`route_result`, `selected_pool`, `selected_pool_reason`, and
`preferred_next_account_id` are route-level fields. `preferred_next_account_id`
is the route-level neutral projection; `accounts[].preferred_next` is the
per-account boolean projection. For unsupported route bands, JSON uses the same
top-level envelope with `route_result=unsupported_route_band`, `selected_pool=none`,
`selected_pool_reason=unsupported_route_band`, null preferred account, and empty
candidate/account arrays.

`window_slots` contains the exact human-display slot inputs for `5h` and
`weekly`: slot label, evidence state `known | unknown | no_data`, optional
`remaining_headroom`, optional `reset_unix_seconds`, optional
`reset_duration_seconds`, and display note. V1 deliberately does not expose
`source_window_ids` because no stable provider/window id exists in the selector
facts. `windows` contains every relevant route-band window with safe metadata
only: window seconds, status, optional headroom, optional reset time, observed
time, effective flag, pressure, surplus, and whether that window contributed to
salvage. JSON must be able to reconstruct why the human table rendered
`unknown`, `no data`, `fallback`, `held`, `available`, or `preferred` without
reading logs or raw provider DTOs.

`safe_account_label` is always produced by the shared
`codex-router-core::redaction` helper before any assessment, status, log, trace,
or smoke transcript emission. If the configured label looks like an email
address, provider-derived identity, token, auth header, or secret-store
material, the helper must replace it with a deterministic safe hash/tag. Raw
configured labels are not emitted by default status, logs, traces, smoke
transcripts, or selection explanation DTOs.

`SafeAccountLabel` helper contract:

- input: configured local label plus canonical `AccountId`
- output: `SafeAccountLabel`, either a preserved display label or a redacted tag
- preserved labels must be printable ASCII, contain no control characters, be
  at most 64 characters, and match `^[A-Za-z0-9][A-Za-z0-9._ -]{0,63}$`
- labels are unsafe and must be replaced when they contain an email-like `@`,
  URL, filesystem path separator, `Authorization`, `Bearer`, `Basic`, `sk-`,
  `sess-`, `oauth`, `refresh`, `token`, `secret`, `keychain`, `1password`,
  or any non-printable or non-ASCII character
- redacted tag format is `acct-<12 lowercase hex chars>`, derived from
  `sha256("codex-router:safe-account-label:v1:" || account_id)` and never from
  the raw unsafe label
- every human table/plain row, JSON `safe_account_label`, proxy selection log,
  trace, audit event, and smoke transcript must use this same output value

Default quota status is account-centric for the user quota route. It must not
emit separate route-band rows such as `models`, `code_review`, or
`memories_trace_summarize` unless an explicit future debug/multi-route mode is
specified. Structural guardrails must assert one logical row per account, with
only an optional blank-account continuation line for the second physical line.

## Security And Trust Context

This design touches auth-adjacent account selection but does not expose secrets.

Assets:

- OAuth access tokens and refresh tokens
- router bearer token
- router affinity hash secret
- upstream auth headers
- account id, safe display label, and account hash
- persisted quota state
- request bodies, response bodies, prompts, memory traces, and tool arguments
- logs, audit events, traces, and smoke transcripts

Entry points:

- HTTP/SSE request routing
- WebSocket handshake and first client frame
- CLI quota status rendering
- background quota refresh
- state repository reads/writes
- secret-store and credential resolver calls

Trust boundaries:

- local client to router local-auth boundary
- router to secret store/provider credential boundary
- router to upstream API boundary
- router to SQLite state boundary
- router to logs/traces/smoke transcript boundary

Privileged actions:

- selecting an account
- resolving credentials
- injecting upstream auth
- opening upstream HTTP/SSE or WebSocket connections
- writing logs, traces, audit events, status rows, and smoke transcripts

Security-sensitive invariants:

- selector assessment must consume account ids and quota facts only, never OAuth access tokens or refresh tokens
- status output must not print provider tokens, router bearer tokens, keychain identifiers, or upstream auth headers
- status output, logs, traces, audit events, smoke transcripts, and review
  artifacts must not print or embed `router_affinity_hash_secret`, its storage
  identifier, or derived secret material
- stale/unknown quota must be conservative to avoid overusing a newly rotated or invalidated account
- account selection must remain after local router auth and before upstream auth injection
- default human, logs, traces, and smoke transcripts must use
  `safe_account_label` or a redacted/hash form, never raw account id by default
- `safe_account_label` must be an operator-chosen local label or redacted/hash
  form; labels that resemble emails, provider-derived account identifiers,
  tokens, or auth material are unsafe for default emission

### Previous-Response Affinity Contract

Previous-response affinity is a continuation correctness rule, not quota
optimization. It applies to HTTP/SSE and WebSocket requests that carry a
provider previous-response identifier.

Ownership:

- proxy extracts previous-response metadata from HTTP/SSE request bodies and
  from the bounded first WebSocket `response.create` frame
- `codex-router-state` owns durable previous-response owner records through
  `AffinityRepository`
- `codex-router-core::affinity` owns typed raw previous-response ids, typed
  affinity-key hashes, typed router affinity hash secrets, and the shared HMAC
  helper
- `codex-router-secret-store` owns persistent `router_affinity_hash_secret`
  load/create behavior for the router root
- `codex-router-selection` does not own affinity hashing, durable state, secret
  storage, or provider credentials
- `codex-router-selection::affinity` and raw `AffinityKey` are not previous-
  response owner APIs in v1. Existing raw-key helper surfaces must be deleted
  from previous-response routing or renamed for a separately specified
  non-previous-response use. Tests must prove no previous-response request path
  imports or calls the old raw-key table.
- weighted burn-down fallback is allowed only when no previous-response
  affinity key is present

Owner record:

```text
PreviousResponseOwnerRecord {
  affinity_key_hash,
  account_id,
  credential_generation,
  route_band,
  source_transport: http_sse | websocket,
  created_unix_seconds
}
```

The raw previous response id and raw canonical affinity key are sensitive
routing metadata and must not be emitted in logs, traces, status, smoke
transcripts, or review artifacts. The durable lookup key is
`affinity_key_hash = hex(HMAC-SHA-256(router_affinity_hash_secret,
"codex-router:previous_response_id:v1:<value>"))`. The record stores the
selected account id and the selected account's active credential generation at
the time the upstream response id was observed.

Affinity key hash contract:

- The hash secret is router-owned secret material and must be stored with the
  same sensitivity as router bearer/auth material.
- `router_affinity_hash_secret` is not stored in SQLite and is never returned by
  `codex-router-state` APIs. It is loaded through `codex-router-secret-store`
  and passed to `codex-router-core::affinity` only long enough to derive
  `AffinityKeyHash`.
- `codex-router-secret-store` exposes
  `load_or_create_router_affinity_hash_secret(router_root) ->
  RouterAffinityHashSecretLoad`, where the result identifies whether the secret
  was loaded or newly created without exposing the secret value in debug/error
  output.
- The stable secret key is `router_affinity_hash_secret.v1`.
- The persisted value is 32 bytes of CSPRNG entropy encoded as 64 lowercase hex
  characters and decoded into `RouterAffinityHashSecret([u8; 32])`.
- Secret-store errors must be redacted: they may expose an error class such as
  `missing`, `unreadable`, `invalid_encoding`, or `permission_denied`, but must
  not expose the secret value, secret file path, keychain item identifier, or
  backend-specific secret identifier.
- The hash secret is generated once per router root and persisted independently
  from local bearer tokens, OAuth/account credentials, and credential
  generations.
- V1 has no automatic or manual hash-secret rotation path. Local bearer-token
  rotation, OAuth token refresh, account credential rotation, server restart,
  and quota refresh must not change `router_affinity_hash_secret`.
- If the hash secret is missing, unreadable, or replaced, existing owner rows
  are ignored or purged and continuation requests fail closed before weighted
  fallback. The router must not regenerate a new secret and treat old owner rows
  as valid.
- If the hash secret cannot be loaded or created for a route that can create or
  consume previous-response ids, the request fails locally as
  `affinity_secret_unavailable` before selector advancement, credential
  resolution, upstream auth injection, or upstream open. V1 must not silently
  skip owner writes for successful response-creating requests.
- The hex digest is the full 32-byte HMAC output encoded as 64 lowercase hex
  characters; truncation is forbidden.
- Raw canonical affinity keys and raw previous response ids are never persisted.
- Proxy-edge affinity extraction accepts typed raw previous-response ids and
  constructs `affinity_key_hash` through the shared core helper before calling
  storage, logging, tracing, or audit APIs. State repository APIs accept only
  `AffinityKeyHash` and owner-record DTOs.
- Existing raw affinity pins are not migrated. This feature performs a hard
  schema cutover: old raw-key rows are discarded or ignored during the schema
  replacement, and tests must prove no raw-key fallback remains.
- If storage contains more than one owner record for the same
  `affinity_key_hash`, or if lookup ambiguity is otherwise detected, owner
  resolution fails closed before weighted fallback.

Affinity repository cutover contract:

- `AffinityRepository::pin_previous_response_owner(record:
  &PreviousResponseOwnerRecord)`.
- `AffinityRepository::load_previous_response_owner(hash: &AffinityKeyHash)
  -> OwnerLookup`, where `OwnerLookup` distinguishes `missing`, `found(record)`,
  and `ambiguous`.
- `AffinityRepository::purge_previous_response_owners()` for hard-cutover or
  invalid-secret repair paths.
- Repository methods accept only `AffinityKeyHash`, `PreviousResponseOwnerRecord`,
  and typed owner DTOs. No state repository method accepts raw
  `PreviousResponseId`, raw canonical affinity key strings, request bodies, or
  response bodies.
- The schema replacement stores `affinity_key_hash`, `account_id`,
  `credential_generation`, `route_band`, `source_transport`, and
  `created_unix_seconds`. Old raw `affinity_key, account_id` rows are discarded
  or ignored, and no raw-key fallback remains.

Owner record creation:

- HTTP/SSE: after the proxy has selected an account and receives an upstream
  response object or SSE event for that account, it may inspect only the
  upstream response identifier field needed to build the affinity key. In v1,
  the allowed field is top-level `id` on the upstream response object/event
  data. If no valid response id is observed, no owner record is written.
- WebSocket: after the proxy has selected and pinned an account for the
  connection, it may inspect only the upstream response identifier field needed
  to build the affinity key. In v1, the allowed field is top-level
  `response.id` on an upstream response event. If no valid response id is
  observed, no owner record is written.
- Owner record writes happen after upstream account selection and must not feed
  the current request's account choice.
- Owner record writes must never log or persist raw request bodies, raw response
  bodies, full WebSocket frames, prompts, tool arguments, or raw previous
  response ids.
- A later continuation owner hit is valid only when the stored account is still
  enabled, has an active credential, the active credential generation equals the
  stored `credential_generation`, and the account is route-eligible.
  Route-eligible means burn-down assessment returns `availability=usable` or
  `availability=reserve` for the owner account. `availability=unknown`,
  `availability=blocked`, and `availability=excluded` fail closed.

Owner resolution:

- affinity extraction reads only the top-level JSON field
  `previous_response_id` from HTTP/SSE request bodies and from the first
  WebSocket `response.create` frame
- absent `previous_response_id` means no previous-response affinity key is
  present and weighted burn-down selection may run
- present `previous_response_id` must be a non-empty string; `null`, empty
  string, number, boolean, array, or object is `malformed_affinity` and fails
  closed before weighted fallback, credential resolution, or upstream open
- the canonical affinity key is `previous_response_id:<value>`; logs, traces,
  audit events, and smoke transcripts may emit only a hash of this key, never
  the raw previous response id
- a continuation request with a known previous-response owner must use that
  owner account if the account is enabled, has an active credential generation,
  and is route-eligible
- missing owner, disabled owner, stale credential generation, unavailable
  credential, unknown owner quota, route-ineligible owner, or exhausted owner
  fail closed before
  weighted fallback
- a continuation request must never silently replay on a different account
- failure must be local and audit-safe, with no upstream open and no provider
  credential resolution for a different account

Proof must cover:

- HTTP/SSE owner-record creation from the allowlisted upstream response id field
- WebSocket owner-record creation from the allowlisted upstream response id
  field after account pinning
- affinity hash construction uses the exact HMAC-SHA-256 lowercase-hex contract
  and never persists raw canonical affinity keys or raw previous response ids
- HTTP/SSE continuation owner hit
- WebSocket continuation owner hit
- restart/durable owner lookup
- unknown previous-response owner
- disabled owner
- stale or unavailable credential generation
- unknown owner quota fails closed
- route-ineligible or exhausted owner
- duplicate or ambiguous owner record fails closed
- malformed affinity metadata
- no weighted fallback on any continuation-owner failure
- reserve owner continuity when another account is in the current usable
  selected pool; this must continue on the reserve owner and not weighted
  fallback

### Local Router Auth Contract

Local router auth is an ingress boundary between Codex CLI and codex-router. It
is separate from upstream OAuth/provider credentials and from router-owned
secret storage.

Accepted local auth surface in v1:

- Generated Codex profile: `env_key = "CODEX_ROUTER_TOKEN"`, which makes
  installed Codex send `Authorization: Bearer <token>`.
- HTTP/SSE generated-profile ingress: `Authorization: Bearer <token>`.
- WebSocket generated-profile ingress: `Authorization: Bearer <token>` on the
  local WebSocket upgrade.
- Manual/compatibility ingress for HTTP/SSE and WebSocket:
  `X-Codex-Router-Token: <token>`.

Forbidden local auth fallback surfaces in this goal:

- query-string tokens
- cookies
- WebSocket subprotocol token smuggling
- HTTP/SSE request-body token carriers expressed as top-level JSON field names
- WebSocket top-level first-frame auth-smuggling fields:
  `authorization`, `x-codex-router-token`, `api_key`, `token`,
  `access_token`, `refresh_token`, or `bearer`

Requests that omit both accepted carriers fail as local auth failures before
route-band assessment, selector advancement, credential resolution, upstream
auth injection, upstream HTTP/SSE open, or upstream WebSocket open. Requests
that present any forbidden token carrier also fail before those same side
effects.

HTTP/SSE forbidden body-carrier detection is intentionally narrow:

- applies only to JSON request bodies with an object at the top level
- checks only top-level field names against `authorization`,
  `x-codex-router-token`, `api_key`, `token`, `access_token`,
  `refresh_token`, and `bearer`
- never scans nested prompt text, tool arguments, metadata, message content,
  binary bodies, form strings, or arbitrary token-like values
- emits only `forbidden_carrier_kind=http_body`, never raw body fields or values

WebSocket first-frame forbidden-carrier detection uses the same field-name
denylist at the top level of the first frame only.

Mixed-carrier rule:

- If both `Authorization: Bearer` and `X-Codex-Router-Token` are present, both
  values must be syntactically valid, equal, and validate against the active
  local router token generation.
- If both accepted carriers are present and differ, are malformed, or one does
  not validate, the request fails as local auth before any selection or upstream
  side effect.
- The router strips both accepted local-auth carriers before upstream open.

Local auth validation input contract:

```text
PresentedLocalAuthCarriers {
  authorization_bearer: Option<RedactedTokenCandidate>
  x_codex_router_token: Option<RedactedTokenCandidate>
  forbidden_carrier_present: bool
  forbidden_carrier_kind:
    query | cookie | http_body | websocket_subprotocol | websocket_first_frame
}
```

HTTP/SSE handlers and WebSocket preflight preserve accepted handshake/header
carriers until local-auth validation decides accept/reject. HTTP/SSE body and
WebSocket first-frame auth-smuggling checks are separate preselection
validators owned by their protocol handlers; they run after local header auth
has established a valid client but before route assessment, selector state,
credential resolution, or upstream open. All local-auth and auth-smuggling
failures share the same zero-side-effect guarantee.

HTTP/SSE and WebSocket proof must include the generated-profile
`env_key`/Authorization bearer path, the manual `X-Codex-Router-Token` path,
and negative cases for query, cookie, HTTP body-token, WebSocket
subprotocol-token, WebSocket first-frame auth-smuggling field, and mismatched
mixed-carrier requests.

Installed-Codex e2e proof must emit the audit-safe enum
`local_auth_carrier=authorization_bearer` and boolean
`local_auth_validated=true` from the local router test harness. It must not emit
the bearer token, raw auth header, header length, token hash, or token prefix.
Upstream-only evidence is insufficient for the generated-profile bearer-auth
e2e gate.
Dedicated local-auth ingress tests remain the authoritative negative proof for
manual header, query, cookie, body, subprotocol, first-frame auth-smuggling, and
mixed-carrier failure cases.

### HTTP/SSE Routing Order Contract

HTTP/SSE routing order is normative for response-creating and
previous-response-capable routes:

1. Validate local router auth and reject any forbidden auth carrier before
   route-band assessment, selector state, credential resolution, or upstream
   open.
2. Classify the HTTP path into `codex-router-core::routes::RouteBand` or the
   shared `unsupported_route_band` result before selector state, credential
   resolution, or upstream open.
3. For any route that can create or consume previous-response ids, load or
   create `router_affinity_hash_secret.v1` before selector advancement,
   credential resolution, upstream auth injection, or upstream open.
4. If the affinity secret is unavailable, fail locally as
   `affinity_secret_unavailable` with zero selector advancement, zero credential
   resolver calls, zero upstream auth injection, and zero upstream open.
5. Extract previous-response affinity metadata, when present, before weighted
   fallback.
6. If affinity is present, apply the Previous-Response Affinity Contract. Any
   owner-resolution failure fails closed before weighted fallback.
7. If no affinity is present, call the shared route-band assessment and select
   from `weighted_candidates`.
8. Resolve the selected account credential and inject upstream auth exactly once
   after selection and before upstream open.
9. Strip local client auth, router bearer auth, and hop-by-hop headers before
   upstream open.

HTTP/SSE proof must cover mixed-carrier local-auth failure and
`affinity_secret_unavailable` before selector advancement, credential
resolution, upstream auth injection, and upstream open.

### WebSocket Compatibility Contract

The v1 router must support the Codex `/v1/responses` WebSocket path. It uses the
same `responses` route band as HTTP/SSE response creation.

WebSocket routing order is normative:

1. Validate local router auth before accepting the local WebSocket upgrade,
   selector state, quota assessment, credential resolution, or upstream
   connection open.
2. Fail closed with `unsupported_path` for any WebSocket path other than
   `/v1/responses` before accepting the local WebSocket upgrade, selector
   state, quota assessment, credential resolution, or upstream connection open.
   `/v1/realtime` and any unknown path are representative `unsupported_path`
   cases in v1.
3. Require a bounded first client frame containing a valid `response.create`
   payload before opening the upstream WebSocket.
4. Enforce the v1 first-frame resource contract before selection:
   - maximum first frame size: 1 MiB
   - maximum wait for first frame: 250 ms
   - accepted first frame shapes:
     - enveloped `response.create`: top-level `type == "response.create"`
     - installed-Codex direct Responses payload: no top-level `type`, with
       minimal structural fields `model`, `input`, and `stream=true`
   - locally read fields before selection:
     - top-level `type`, when present
     - top-level `previous_response_id`, when present
     - for direct payload compatibility only: whether `model` is present,
       whether `input` is present, and whether `stream` is `true`
     - for auth-smuggling rejection only: whether any forbidden top-level
       auth-carrier field name is present
   - no route-band field is read from the first-frame body in v1; the
     `/v1/responses` WebSocket path fixes the route band to `responses`
   - full request-schema validation remains upstream-owned
5. Do not parse any additional first-frame/body fields before selection. The
   route band comes from the `/v1/responses` path only. Direct-payload
   structural and auth-smuggling checks may accept or reject the first frame but
   must not expose or use the raw `model`, `input`, `stream`, token-like field
   values, nested prompt text, or nested body values for route-band selection,
   account scoring, logs, traces, audit records, or smoke transcripts.
6. Reject any first frame with a forbidden top-level auth-carrier field name as
   `forbidden_local_auth_carrier` before selector advancement, credential
   resolution, upstream auth injection, or upstream open. This is a narrow
   top-level field-name check only; the router does not scan nested prompt text,
   tool arguments, metadata values, or arbitrary body strings for token-like
   words.
7. Load or create `router_affinity_hash_secret.v1` for the router root before
   selection, because `/v1/responses` can create or consume previous-response
   ids. If the secret is unavailable, fail locally as
   `affinity_secret_unavailable` before selector advancement, credential
   resolution, upstream auth injection, or upstream open.
8. Extract previous-response affinity metadata from the first frame, when
   present, before weighted fallback.
9. If affinity is present, apply the Previous-Response Affinity Contract. Any
   owner-resolution failure fails closed before weighted fallback.
10. If no affinity is present, run reset-aware route-band assessment for the
   `responses` route band and select from weighted candidates.
11. Resolve the selected account credential and inject upstream auth exactly once
   after selection and before upstream open.
12. Strip local client auth, router bearer auth, and hop-by-hop headers before
   upstream open.
13. Forward the accepted first frame and all later frames unchanged at the
   protocol payload layer.
14. Pin the selected account for the lifetime of the WebSocket connection. Do
    not switch accounts mid-stream. A later connection may reselect after quota
    state changes.

Malformed first frames, unsupported WebSocket routes, and failed local auth must
advance no selector state, resolve no provider credential, inject no upstream
auth, and open no upstream connection.

Preselection WebSocket failure matrix:

| Failure mode | Local upgrade accepted? | Selector advance | Credential resolver | Upstream auth injection | Upstream open | Logging/redaction |
| --- | --- | --- | --- | --- | --- | --- |
| missing or invalid local auth | no | 0 | 0 | 0 | 0 | no tokens or auth headers |
| unsupported path | no | 0 | 0 | 0 | 0 | route and reason only |
| non-text or non-JSON first frame | yes | 0 | 0 | 0 | 0 | no full payload |
| syntactically valid wrong `type` | yes | 0 | 0 | 0 | 0 | no full payload |
| forbidden top-level auth carrier in first frame | yes | 0 | 0 | 0 | 0 | field kind only; no value |
| oversized first frame | yes | 0 | 0 | 0 | 0 | size class only |
| timed-out first frame | yes | 0 | 0 | 0 | 0 | timeout reason only |
| affinity hash secret unavailable | yes | 0 | 0 | 0 | 0 | error class only; no secret path/id/value |
| malformed affinity metadata | yes | 0 | 0 | 0 | 0 | affinity key hash only when available |
| missing/disabled/stale/unavailable/exhausted owner | yes | 0 | 0 | 0 | 0 | affinity key hash and safe label/hash only |

Required WebSocket proof:

- valid `/v1/responses` WebSocket routes through reset-aware selection
- selected account is pinned for the full WebSocket connection
- `/v1/realtime` and one unknown WebSocket path fail closed as
  `unsupported_path` before selection
- invalid local auth fails before selection
- malformed first frame and syntactically valid wrong-type first frame fail
  before selection
- forbidden top-level auth-carrier fields in the first frame fail before
  selection and emit only the forbidden carrier kind
- direct installed-Codex payload without an enveloped `type` is accepted when
  it satisfies the bounded structural response-create check
- oversized or timed-out first frame fails before selection
- affinity hash-secret unavailable fails before selection, credential
  resolution, upstream auth injection, and upstream open
- previous-response affinity is honored before weighted fallback and fails
  closed on owner-resolution failure
- malformed, wrong-type, oversized, timed-out, unsupported, auth-failed, and
  affinity-failed paths show zero selector advance, zero credential resolver
  calls, zero upstream auth injection, and zero upstream opens
- a local-field allowlist canary proves first-frame parsing reads only
  top-level `type`, top-level `previous_response_id`, and direct-payload
  structural booleans before selection; raw non-allowlisted body values,
  including `model`, `input`, `metadata`, `tools`, prompt text, or request body
  content, must not affect route-band selection, logs, traces, or audit records
  before upstream-owned validation

Allowed and forbidden emission surfaces:

| Surface | Allowed | Forbidden |
| --- | --- | --- |
| default status rows | safe account label, status, percent-left bars, reset time, availability, routing reason | account id, OAuth tokens, router bearer token, keychain identifier, auth headers, raw score, unsafe label |
| JSON machine status | account id, safe account label, enum reasons, routing weight, freshness, reset metadata | OAuth tokens, router bearer token, refresh token, upstream auth headers, request/response body, unsafe label |
| selection explanation | safe account label or hash, route band, availability, reason code | provider tokens, bearer tokens, auth headers, secret-store paths, prompts, tool args |
| refresh errors | provider status class, redacted endpoint class, safe account label/hash | response bodies containing secrets, full auth headers, tokens |
| traces/logs | route band, availability, reason enum, safe account label/hash | token values, upstream auth headers, keychain identifiers, raw request/response body, full WebSocket first-frame payload, prompts, memory traces, tool args, unsafe labels |
| smoke transcripts | commands, redacted route band, reason enums, percentages/reset durations, selected safe label/hash | any token, full auth header, secret file/keychain material, raw request/response body, full WebSocket first-frame payload, prompts, memory traces, tool args, unsafe labels |

All forbidden columns above also forbid `router_affinity_hash_secret`, its
secret-store identifier, and derived secret material. JSON machine status never
contains affinity hash secret material.

Smoke transcript WebSocket first-frame policy:

- persisted or shared smoke artifacts may emit only allowlisted safe routing
  proof fields from first-frame handling:
  - `first_frame_type=response.create`
  - `first_frame_shape=enveloped | direct`
  - local auth carrier enum such as `authorization_bearer` or
    `x_codex_router_token`
  - `local_auth_validated=true | false`
  - token/header absence booleans for upstream-open stripping only
  - route band
  - selected safe account label/hash
  - reason enum
  - selector/credential/upstream-open call counts
- smoke artifacts must not emit raw values from `model`, `input`, `metadata`,
  `tools`, prompt text, request body content, or any other non-allowlisted
  first-frame/body field, even as individual summary fields such as
  `first_frame_model`, `first_frame_has_input`, or `first_frame_stream`
- Existing transcript fields named `first_frame_model`,
  `first_frame_has_input`, and `first_frame_stream` are stale and
  non-compliant with this goal. The implementation plan must delete them or
  replace them with the allowlisted `first_frame_shape`/`first_frame_type`
  fields above.

## Proof Expectations

The implementation plan must provide proof at these layers:

- pure assessment tests for per-window pressure, surplus, near-reset salvage, reserve, blocked, stale, and unknown behavior
- repository-backed selector tests using mixed 5h and weekly windows
- tests proving weekly pressure beats short-window urgency when weekly reset is far
- tests proving long-window near-reset salvage is allowed when reset is imminent
- tests proving unknown quota loses to known healthy quota
- tests proving unknown quota is fallback-only and never competes with known
  `usable` or `reserve` accounts
- tests proving empty/no-window accounts are `unknown` fallback, not normal usable
- tests proving missing reset time is conservative and receives no salvage
- tests proving mixed stale/unknown/ineligible collapse uses any-window conservative rules
- tests proving route-band batch assessment returns the same account ordering,
  selected pool, selected-pool reason, weighted candidates, and neutral
  `preferred_next_account_id` projection used by status
- tests proving the route-band policy registry has explicit policies for every
  currently classified route band and fails closed for unregistered route bands
  before weighted selection
- tests proving `assess_route_band(...)` owns policy lookup internally, callers
  do not pass `route_band_policy`, and unsupported bands return the exact
  `UnsupportedRouteBandAssessment` payload with empty accounts/candidates
- tests proving proxy route classification, core `RouteBand` variants, and
  selection policy lookup cannot drift, with every routed `RouteBand` covered by
  either an explicit policy or `unsupported_route_band`
- tests proving the exact salvage tie key produces deterministic
  `weighted_candidates`, `preferred_next_account_id`, and empty-state
  `WeightedDeficitSelector` agreement when accounts tie on weight and pressure
- tests proving runtime exact selection may differ from
  `preferred_next_account_id` because of previous-response affinity or
  accumulated weighted-deficit fairness state, and that default status does not
  claim runtime-exact next use
- tests proving runtime audit receives one `RuntimeSelectedAccountDecision`
  containing the shared `BurnDownRouteBandAssessmentResult` envelope and does
  not reconstruct route-result fields from raw quota rows after selection
- tests proving cooldown/hold reuse happens only when the held account remains
  in the current assessment's `weighted_candidates`
- tests proving a selected-pool change breaks a hold when the held account is
  no longer in the new selected pool
- tests proving previous-response affinity can reuse a reserve owner outside
  the current selected pool and fails closed before weighted fallback only when
  the owner is `unknown`, `blocked`, `excluded`, disabled, missing credential,
  exhausted, ineligible, or stale for credential generation
- tests proving a successful previous-response affinity hit does not advance
  weighted-deficit state, refreshes the runtime hold, and does not move a
  reserve owner into the pure assessment's `weighted_candidates`
- tests proving unknown-pool fallback can hold only accounts still in the
  unknown selected pool and cannot keep a former known-pool account alive after
  evidence falls to all-unknown
- tests proving stale penalty division is applied only inside the selected pool
  and reclamped after division
- tests proving `accounts[]` uses canonical `account_id` order while
  `weighted_candidates[]` uses neutral selector order, with account id only as
  the final tie fallback
- tests proving unknown fallback never competes with known `usable` or
  `reserve`, but preserves conservative partial-headroom ordering inside the
  all-unknown fallback pool
- tests proving all-unknown fallback emits `fallback` next-use rows without
  implying healthy quota
- tests proving raw unknown evidence reasons remain in `quota_evidence_reason`
  and never appear as public `routing_reason` values
- tests proving accounts with exactly one expected v1 window missing are
  `unknown`, never normal usable, and render the missing slot as `no data`
- tests proving unknown, missing-reset, and no-window human slots never render
  fake `0% left`
- tests proving disabled and missing-active-credential accounts are returned in
  `BurnDownRouteBandAssessment.accounts` as `availability=excluded`, never enter
  `weighted_candidates`, map to `excluded_disabled` or
  `excluded_missing_credential`, and render as blocked without leaking unsafe
  labels or secret material
- CLI renderer tests proving status uses the same assessment reason and limiting-window semantics as routing
- JSON schema tests for stable machine fields and enum values
- JSON envelope tests proving route-level fields, `weighted_candidates[]`,
  `accounts[]`, per-account `window_slots.{5h,weekly}`, and per-account
  `windows[]` use the normative shape above
- JSON schema tests proving `safe_account_label` is sanitized/hash-tagged and
  no unsafe configured label is emitted
- JSON schema tests proving machine output contains selected pool, next use,
  window slots, all relevant windows, reset metadata, and enough safe fields to
  reconstruct the default human status explanation
- JSON capture redaction tests proving raw local JSON stdout may contain
  `account_id`, but any persisted/shared smoke transcript, log, trace, PR
  evidence, or review artifact containing JSON output redacts or hashes
  `account_id`
- plain renderer tests proving ASCII bars, no raw scores, no account ids, and
  the same routing phrases as table mode
- reason mapping tests from stable enum to human phrase
- routing reason precedence tests proving overlapping weekly-reset-salvage,
  preferred-weekly-health, short-reset-salvage, and highest-weight predicates
  produce one deterministic `routing_reason`
- Scenario B status proof proving long-window near-reset salvage renders
  `preferred_weekly_reset_soon`, not only `preferred_highest_weight`
- safe-label and redaction canary tests for account labels that look like
  emails, provider identities, tokens, auth headers, or secret-store material
- shared safe-label helper tests proving CLI, proxy logs/traces, JSON, and
  smoke transcript helpers consume one `SafeAccountLabel`/hash implementation
- shared safe-label helper tests proving unsafe predicate minimums and
  `acct-<12 lowercase hex chars>` redacted tag format
- CLI golden/snapshot tests for default human output:
  - healthy multi-account table with Unicode bars
  - limiting-window disagreement between 5h and weekly
  - reset-aware preferred-next explanation
  - unknown or partial data
  - blocked, reserve, usable, and unknown accounts
  - colorless/plain terminal mode
  - one logical row per account with at most one blank-account continuation line
  - no unrelated route-band rows or labels in default status output
  - negative assertions for `pp`, `bottleneck`, default `account_id`, raw score, and token-like strings
- live-safe CLI smoke proof over persisted router state for emitted `table`,
  `plain`, and `json` status output, including redaction and negative
  assertions on the actual command output
- refresh persistence integration tests proving successful refresh replaces
  account/route-band selector windows and updates `quota_refresh_status`, while
  transient failed refresh preserves previous selector windows, records only a
  redacted `last_error_class`, marks the account/route band stale or
  needs-refresh through assessment/status, and performs that transition through
  one repository operation
- refresh read-model tests proving `selector_inputs_for_route_band(route_band,
  now_unix_seconds)` overlays `QuotaWindowStatus::Stale` when
  `now_unix_seconds >= stale_after_unix_seconds`, preserves last-known headroom
  and reset metadata, and exposes `last_error_class` only through explicit
  status/log/proof DTOs
- refresh read-model tests proving legacy selector rows with missing
  `quota_refresh_status` or null `stale_after_unix_seconds` are treated as
  stale on the first post-upgrade read before a successful refresh
- WebSocket compatibility tests for `/v1/responses` routing, selected-account
  pinning, previous-response affinity before weighted fallback, no weighted
  fallback on continuation-owner failure, `/v1/realtime` and unknown path
  `unsupported_path` pre-upgrade failure, malformed first frame failure,
  wrong-type first frame failure, top-level first-frame auth-smuggling failure,
  oversized first frame failure, first-frame timeout failure, malformed affinity
  failure, and local-auth pre-upgrade failure before selection
- HTTP/SSE and WebSocket affinity pin-write tests proving owner records are
  created only from allowlisted upstream response id fields, store
  `credential_generation`, and never emit raw previous response ids, raw
  affinity keys, raw bodies, prompts, tool args, or full WebSocket frames
- affinity hash tests proving full-length lowercase-hex HMAC-SHA-256 output,
  shared helper use before storage/logging/audit, no raw-key persistence, no
  raw-key fallback after schema cutover, and fail-closed behavior for duplicate
  or ambiguous owner rows
- affinity repository cutover tests proving repository methods accept only
  `AffinityKeyHash`/owner-record DTOs, store `credential_generation`,
  `route_band`, `source_transport`, and `created_unix_seconds`, and never accept
  or persist raw previous-response ids or raw canonical keys
- previous-response affinity cutover tests proving no previous-response
  lookup/write/resolve path imports or calls `codex-router-selection::affinity`
  or raw `AffinityKey`
- secret-store affinity-secret tests proving the stable key
  `router_affinity_hash_secret.v1`, 32-byte entropy, 64-lowercase-hex encoding,
  typed core return value, loaded/newly-created result state, and redacted error
  contract
- affinity hash-secret lifecycle tests proving the secret is generated once per
  router root, persists across server restarts, is independent of local bearer
  token rotation and account credential rotation, has no v1 rotation path, and
  causes existing owner rows to be ignored or purged if missing, unreadable, or
  replaced
- affinity secret redaction tests proving `router_affinity_hash_secret`, its
  secret-store identifier, and derived secret material never appear in status,
  JSON, logs, traces, audit events, smoke transcripts, or review artifacts
- affinity-secret-unavailable tests proving response-creating HTTP/SSE and
  WebSocket requests fail locally before selector advancement, credential
  resolution, upstream auth injection, or upstream open when the hash secret
  cannot be loaded or created
- HTTP/SSE call-order tests proving local auth, route classification,
  affinity-secret load/create, affinity handling, assessment, credential
  resolution, auth injection, header stripping, and upstream open happen in the
  normative order for response-creating and previous-response-capable routes
- WebSocket redaction proof with a synthetic canary in first-frame/request-body
  content, proving audit/log/smoke artifacts do not contain the raw body or full
  first-frame payload
- WebSocket auth-smuggling proof with a top-level first-frame field named
  `token`, proving rejection before selector advancement, credential
  resolution, upstream auth injection, or upstream open and proving logs/traces
  expose only the forbidden field kind, not the value
- WebSocket first-frame allowlist proof with canary values in non-allowlisted
  fields such as `model`, `input`, `metadata`, `tools`, and prompt text,
  proving selection reads only top-level `type`, top-level
  `previous_response_id`, direct-payload structural booleans, and forbidden
  top-level auth-carrier field-name presence before selection, and never emits
  raw direct-payload values
- smoke transcript negative proof with the same non-allowlisted first-frame/body
  canaries, proving persisted/shared smoke artifacts do not emit raw `model`,
  `input`, `metadata`, `tools`, prompt text, request body content, or any
  non-allowlisted first-frame/body field as individual summary fields
- WebSocket non-blocking proof that delayed or failing quota refresh does not
  block the first valid `/v1/responses` route after bounded first-frame parsing,
  and that selection uses persisted selector rows on that path
- security call-order tests proving every row in the WebSocket preselection
  failure matrix makes zero selector-state advances, zero credential resolver
  calls, zero upstream auth injections, and zero upstream opens
- local-auth ingress tests proving generated-profile
  `env_key = "CODEX_ROUTER_TOKEN"` / `Authorization: Bearer` works for HTTP/SSE
  and WebSocket, manual `X-Codex-Router-Token` works for HTTP/SSE and
  WebSocket, and query parameter, cookie, HTTP request-body token, WebSocket
  subprotocol token, WebSocket top-level first-frame auth-smuggling field, and
  mismatched mixed-carrier auth fail before selection, credential resolution,
  upstream auth injection, or upstream open
- HTTP/SSE body auth-smuggling tests proving only top-level JSON field names in
  the denylist fail as `forbidden_carrier_kind=http_body`, while nested prompt,
  tool, metadata, message, binary, form, and arbitrary string values are not
  scanned or emitted
- black-box non-blocking proof for:
  - server boot/listen readiness while provider refresh is delayed or failing
  - first routed request while provider refresh is delayed or failing
  - quota status render using persisted data while refresh is delayed or failing
- end-to-end Codex-through-router proof before implementation completion can be
  claimed. Minimum acceptance is installed Codex CLI using a generated
  codex-router profile against a served local router and mock upstream, with
  both HTTP/SSE and WebSocket transport exercised. The fixture must include
  multiple persisted accounts and selector quota windows that force a
  reset-aware account choice, then prove the chosen safe label/hash, routing
  reason, status output, WebSocket selected-account pinning, and redacted
  transcript artifacts agree. Live OAuth, live quota refresh, or real upstream
  quota cycling is not part of this required local e2e gate unless explicitly
  approved as a separate live-proof layer.
- generated-profile local-auth proof for the installed-Codex e2e fixture. The
  helper-rendered Codex custom-provider profile must use
  `env_key = "CODEX_ROUTER_TOKEN"` and installed Codex must authenticate to the
  local router with `Authorization: Bearer` for both HTTP/SSE and WebSocket.
  The e2e transcript must prove local router receipt with the audit-safe enum
  `local_auth_carrier=authorization_bearer` plus
  `local_auth_validated=true`, must prove local auth is stripped before upstream
  HTTP/SSE and WebSocket opens, and must never print the token, raw auth header,
  token hash, token length, or token prefix.
- redaction proof for status rows, machine status, selection explanations,
  refresh errors, traces/logs, and smoke transcripts

Non-blocking pass signal:

- The server must bind/listen without waiting for provider quota refresh.
- The first routed request must either route using persisted quota state or fail
  for an auth/upstream reason unrelated to quota-refresh waiting; it must not
  wait for live refresh before selecting.
- `quota status` must render last-known persisted state immediately and may mark
  rows `needs refresh`.
- Tests should synchronize on observable readiness, request completion, rendered
  output, or bounded fake-provider calls, not wall-clock sleeps.

## Next Workflow

Run `shravan-dev-workflow:spec-review-swarm` against this revised spec. Only if
the parent reducer records a spec-review verdict of `ready` in the latest
`spec-review-*/review-ledger.md` should the orchestrator transition to
`shravan-dev-workflow:plan-creation-swarm`.
