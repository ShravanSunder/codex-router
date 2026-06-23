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
- Which accounts need a background probe before they may be used?
- Is the status display explaining the same burn-down reasoning runtime routing
  uses, while being honest about fairness and affinity overrides?

## Current-State Evidence

Observed in current code:

- Runtime selection reads route-band selector rows from `SelectorQuotaRepository::selector_inputs_for_route_band`.
  Source: `crates/codex-router-proxy/src/account_selection.rs:189-210`, `crates/codex-router-state/src/repositories.rs:46-59`.
- Current selector collapse loses reset geometry. It rejects any ineligible window, finds the effective window, then uses the minimum `remaining_headroom` across windows.
  Source: `crates/codex-router-proxy/src/account_selection.rs:262-292`.
- `WeightedDeficitSelector` is generic weighted round-robin over `(AccountId, u32)`. It does not know freshness, reset time, weekly quota, or route semantics.
  Source: `crates/codex-router-selection/src/weighted_deficit.rs:60-98`.
- Persisted selector windows already contain the raw facts needed for reset-aware routing: `limit_window_seconds`, `status`, `remaining_headroom`, `reset_unix_seconds`, `effective`, and `observed_unix_seconds`.
  Source: `crates/codex-router-state/src/quota_snapshot.rs:91-200`.
- CLI status already computes pace and projected runout from reset time and remaining headroom.
  Source: `crates/codex-router-cli/src/quota.rs:924-1007`.
- Current WebSocket routing selects an account and resolves provider credentials
  before bounded first-frame parsing. The target design intentionally changes
  that order so local auth and first-frame routing metadata validation happen
  before quota assessment, credential resolution, or upstream open.
  Source: `crates/codex-router-proxy/src/websocket.rs`.
- Current WebSocket routing synthesizes a fixed `/v1/responses` selection
  request and does not classify the handshake path before selection. The target
  design intentionally changes that order so shared WebSocket route
  classification and `unsupported_path` failure happen before selection,
  credential resolution, or upstream open.
  Source: `crates/codex-router-proxy/src/websocket.rs`.
- Current affinity helpers and repositories exist, but the target design makes
  previous-response owner lookup and fail-closed behavior explicit for both
  HTTP/SSE and WebSocket continuation requests.
  Source: `crates/codex-router-selection/src/affinity.rs`,
  `crates/codex-router-state/src/repositories.rs:61-72`.
- Existing spec already says weekly quota must be protected before short-window reset urgency.
  Source: `docs/specs/2026-06-20-codex-router-greenfield-spec.md:147-151`.
- Existing plan already calls for selection order: eligibility/freshness, long-window pressure, effective limiting headroom, reset urgency as bounded tiebreaker.
  Source: `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md:322-338`.

## Requirements

R1. Startup and request routing must not block on provider quota refresh.

The selector uses persisted SQLite selector windows. Background refresh or probe
work may update those windows, but startup and request selection do not wait on
live provider I/O.

R2. Route-band-relevant exhausted windows block the account.

For the requested route band, an account is not normally selectable if any relevant quota window is `Ineligible` or has `remaining_headroom == 0`.

R3. Unknown quota is not free capacity.

Unknown accounts never compete with known `usable` or `reserve` accounts. They
are not fallback capacity. Unknown, missing, or insufficient quota evidence
returns `availability=probe_required` and is not routable until background probe
or refresh writes verified quota facts back to SQLite.

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

## Non-Goals

- No forecasting engine.
- No EWMA or historical usage model.
- No per-model token-cost estimation.
- No mid-stream account switching.
- No global optimization across future sessions.
- No live quota polling on the request path.
- No live quota probe on the request path, even when every account is unknown.
- No changes to `WeightedDeficitSelector` that make it know quota-window semantics.

## Spec Boundary / Separability Map

```text
provider quota refresh / probe
  owns: background provider fetch/probe and normalization
  writes: persisted selector quota windows
  must not own: request-path account selection

persisted selector quota windows
  owns: last-known per-account, per-route-band, per-window quota facts
  exposes: SelectorQuotaRepository::selector_inputs_for_route_band(route_band)

burn-down assessment
  owns: reset-aware pressure math, pure routability/exclusion classification
        from supplied facts, and structured explanation
  crate: codex-router-selection::burn_down
  inputs: BurnDownRouteBandAssessmentInput, fixed v1 route band policy
  exposes: BurnDownRouteBandAssessment { accounts, selected_pool, weighted_candidates, preferred_next }
  must not depend on: codex-router-state, codex-router-proxy, codex-router-cli

proxy account selection
  owns: route classification, account fact adaptation, previous-response
        affinity resolution/enforcement, process-lifetime account-hold cooldown,
        process-lifetime fairness state, and runtime exact account choice
  adapts: Vec<SelectorQuotaInput> -> BurnDownRouteBandAssessmentInput
  consumes: BurnDownRouteBandAssessment.weighted_candidates
  exposes: SelectedAccountDecision

weighted deficit selector
  owns: generic weighted fairness state
  consumes: (AccountId, u32)
  must not know: windows, weekly quota, reset time, CLI formatting

quota status CLI
  owns: human and machine rendering
  adapts: Vec<SelectorQuotaInput> -> BurnDownRouteBandAssessmentInput
  consumes: BurnDownRouteBandAssessment explanation and neutral preferred-next projection
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
- Request-path selection must not repair unknown quota by calling the provider.
  It routes from the last-known verified SQLite state and may only emit
  audit-safe probe-required diagnostics. Prompt startup and periodic background
  refresh/probe own provider calls.

## Burn-Down Assessment Contract

### Inputs

`BurnDownRouteBandAssessmentInput`:

- `route_band`
- `now_unix_seconds`
- `accounts: Vec<BurnDownAccountInput>`
- `route_band_policy: BurnDownRouteBandPolicy`

`BurnDownAccountInput`:

- `account_id`
- `account_label`
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

`BurnDownRouteBandPolicy` is fixed v1 behavior, not operator
configuration. Plan creation may name constants and test fixtures, but must not
turn these into user-facing config unless a later spec changes the contract.

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
- unknown accounts are probe-required and are not weighted candidates in v1

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
- set the account-level evidence state to `probe_required` unless a blocking
  condition already applies
- do not grant reset-salvage bonus
- do not compute a routing weight or selected-pool candidate from this account

### Account-Level Collapse

The assessment first applies account and window collapse before weight math.
`quota_evidence_reason` records raw quota evidence before route-band pool
selection. Final `routing_reason` is assigned only after selected-pool/public
mapping, so probe-required rows can explain the missing evidence without
pretending that the account is usable capacity.

1. If the account is not enabled, return `availability=excluded`,
   `routing_exclusion=disabled`, and omit it from selected pools. This is not a
   quota availability class, but it is still returned in `accounts` so status
   can render it without reimplementing eligibility.
2. If there is no active credential generation, return `availability=excluded`,
   `routing_exclusion=missing_credential`, and omit it from selected pools. This
   is not a quota availability class, but it is still returned in `accounts`.
3. If there are no relevant route-band windows, return
   `availability=probe_required`, no `routing_weight`, and
   `quota_evidence_reason=needs_quota_probe`.
4. In v1, the expected public response quota shape is one 5h window
   (`window_seconds=18_000`) and one weekly window
   (`window_seconds=604_800`). If only one of those expected windows is present
   for the route band, return `availability=probe_required` with
   `quota_evidence_reason=missing_expected_window`; the present window may
   be displayed for user context, but the account must not enter any selected
   pool until background probe verifies the missing window.
5. If any relevant window is `Ineligible`, return `blocked` with
   `quota_evidence_reason=window_ineligible`.
6. If any relevant window has `remaining_headroom == 0`, return `blocked` with
   `quota_evidence_reason=window_exhausted`.
7. If any relevant window is `Unknown`, return `availability=probe_required`
   with
   `quota_evidence_reason=unknown_quota_window`.
8. If any relevant non-blocked window has no reset time, return
   `availability=probe_required` with
   `quota_evidence_reason=missing_reset_time`.
9. If at least one relevant window is `Stale` and none are blocked or unknown,
   compute burn-down normally and mark freshness as `stale`.
10. If every relevant window is `Eligible`, compute burn-down normally and mark
   freshness as `fresh`.

Freshness collapse is any-window conservative:

- one stale window makes the account stale
- one unknown or missing-reset window makes the account probe-required
- one missing expected v1 5h or weekly window makes the account probe-required
- one ineligible or exhausted window blocks the account
- the `effective` marker never overrides a worse relevant window

### Availability Classes

The assessment returns exactly one availability class:

- `excluded`: account is disabled or lacks an active credential generation; it
  is returned for status only and never enters any selected pool.
- `blocked`: at least one relevant window is ineligible or exhausted.
- `reserve`: account is not exhausted, but long-window pressure is dangerous enough that it should be used only when no normal account is available.
- `usable`: account can be selected normally.
- `probe_required`: account has insufficient quota evidence. It is not routable
  on the request path. Background probe or refresh must prove usable quota and
  persist new selector windows before a later request may select it.

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

- `accounts: Vec<BurnDownAccountAssessment>`
- `selected_pool: usable | reserve | none`
- `weighted_candidates: Vec<(AccountId, u32)>`
- `preferred_next: Option<AccountId>`
- `account_order: account_id ascending`

`BurnDownAccountAssessment`:

- `account_id`
- `account_label`
- `availability: usable | reserve | blocked | probe_required | excluded`
- `freshness: fresh | stale | unknown`
- `routing_exclusion: none | disabled | missing_credential`
- `limiting_window`
- `quota_evidence_reason`
- `short_pressure`
- `long_pressure`
- `short_salvage`
- `long_salvage`
- `routing_weight`
- `routing_reason`
- `preferred_next`

Only `usable` accounts enter the normal weighted-deficit pool. If no usable
account exists, `reserve` accounts may enter. `blocked` accounts never enter.
`probe_required` and `excluded` accounts never enter weighted routing.

Pool order is normative:

1. Build assessments for every enabled account with an active credential.
2. If one or more `usable` accounts exist, select only from `usable`.
3. Else if one or more `reserve` accounts exist, select only from `reserve`.
4. Else return no eligible account. The current request fails fast rather than
   blocking to probe live quota. Prompt startup and periodic background refresh
   are the normal probe mechanisms; request handling must not introduce
   synchronous provider I/O or proxy-to-worker coupling.

Within the selected pool, candidates are ordered before they are passed to
`WeightedDeficitSelector`. This order is part of the neutral selector contract:

1. higher `routing_weight`
2. lower `long_pressure`
3. lower `short_pressure`
4. salvage tie key
5. `account_id` ascending

Tests that need a deterministic selected winner use this same order and an
empty selector state. `preferred_next` must equal the account selected by this
exact neutral selector contract.

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

For each `probe_required` account:

- preserve the `quota_evidence_reason` from account-level collapse
- do not compute `routing_weight`
- do not add it to `weighted_candidates`
- emit a probe-required status/audit reason
- rely on prompt startup and periodic background refresh/probe work to verify
  future use; request-path handling may only emit audit-safe probe-required
  diagnostics
- only a later persisted successful probe/refresh can make the account
  `usable` or `reserve`

`preferred_next` semantics:

- `preferred_next` is a neutral projection from the pure assessment using an
  empty weighted-deficit state and no previous-response affinity key.
- It answers "which account is preferred for the next normal non-affinity
  request if fairness has no accumulated deficit?"
- It is computed from the exact `weighted_candidates` order given to
  `WeightedDeficitSelector`, so neutral status and neutral runtime selection
  cannot disagree on equal weights.
- It is not the runtime-exact next request. The proxy may choose a different
  account when previous-response affinity is present, when an active
  account-hold cooldown is still valid, or when accumulated weighted-deficit
  state rotates to a lower-weight account for fairness.
- Default status must label this as `preferred next`, not `selected next`.
- Runtime audit may additionally log the actual selected account after affinity,
  account-hold, and weighted-deficit selection.

### Account-Hold Cooldown

The proxy must avoid thrashing between OAuth accounts across adjacent normal
requests. WebSocket lifetime pinning protects one stream; account-hold cooldown
protects the next connection or request from immediately switching accounts
unless the held account can no longer be used.

Ownership:

- `codex-router-proxy` owns process-lifetime hold state keyed by route band.
- The hold state is intentionally not persisted. Restart clears the hold so
  stale process memory never controls a new router instance.
- The pure burn-down assessment owns candidate facts only; it does not know
  cooldown state.
- `WeightedDeficitSelector` remains generic, but the proxy must keep fairness
  state coherent when a held account is reused during cooldown.

Default v1 policy:

- `minimum_account_switch_cooldown_seconds = 120`
- tests may inject a shorter duration and deterministic clock
- no user-facing command flag is required for v1 unless implementation already
  has a clean runtime config path for it

Selection order for non-affinity requests:

1. Run route classification and local auth.
2. Build the route-band burn-down assessment from last-known SQLite state.
3. If no usable/reserve candidate exists, fail fast with
   `no_verified_usable_account`.
4. If a route-band hold exists, its age is below
   `minimum_account_switch_cooldown_seconds`, and the held account is still in
   the current selected pool's `weighted_candidates`, reuse that account with
   `selection_reason=account_hold_cooldown`.
5. Break the hold immediately when the held account is no longer in the current
   selected pool, becomes blocked, probe-required, excluded, disabled, lacks an
   active credential generation, or its relevant quota is exhausted.
6. A valid previous-response affinity owner bypasses account-hold cooldown.
   Affinity owner failure still fails closed before weighted fallback.
7. When the hold is reused, account for that request in fairness state so the
   held account does not receive untracked extra traffic after cooldown expires.
8. When weighted selection chooses a new account, write a new route-band hold
   timestamp for that selected account.

The cooldown is a minimum stability window, not a quota override. It must never
make an empty, blocked, unknown, disabled, credential-invalid, or
probe-required account routable.

Sign semantics:

- higher `pressure_percent` is worse
- higher `risk_penalty` is worse
- higher `surplus_percent` is better only inside near-reset salvage caps
- higher `routing_weight` means safer to use and therefore receives more turns
- `routing_weight` is always a positive scalar for selectable accounts

Tie and determinism contract:

- The runtime keeps `WeightedDeficitSelector` as the fairness state. Its
  accumulated deficits may select a lower-weight account occasionally to
  preserve smooth weighted fairness.
- Deterministic assessment tests compare `routing_weight`, availability,
  limiting window, `preferred_next`, and reason codes, not only the final
  selector output.
- Integration tests that need a selected winner start the weighted selector
  from empty state and provide candidates in the neutral selector order above.

Rationale:

- long pressure has a larger penalty because weekly depletion strands the account longer
- short salvage can help use quota before it expires
- long salvage applies only when long reset is near, so low weekly quota far from reset is not excused
- weighted deficit still balances among similarly safe candidates instead of turning routing into strict single-account priority

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

Expected: A is selected from the `usable` pool. B is `probe_required`, is not a
weighted candidate, and may only become usable after background probe writes
verified quota evidence to SQLite.

Reason: unknown quota is not free capacity.

### Scenario F: all accounts need probe

```text
A: no relevant quota windows
B: unknown 5h and weekly
```

Expected: no account is selected. The request fails fast with an audit-safe
`no_verified_usable_account` class. Prompt startup or periodic background probe
work may later verify A or B and persist selector rows for future requests.

Reason: request routing must not block on live provider quota probes.

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
hashed.

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
  `needs probe`

Wording:

- use `left`, never ambiguous bare percent
- avoid `pp`
- avoid `bottleneck` in default output
- use `limiting window`, `weekly pressure`, `5h pressure`, `preferred next`, `held`, `blocked`, `needs probe`
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
| `probe_required` | needs background probe before use | `availability=probe_required` |
| `excluded` | not routable because account is disabled or has no active credential | `availability=excluded` |
| `limiting window` | the 5h or weekly window driving the decision | `limiting_window` |
| `pressure` | quota is being spent faster than reset pace | `pressure_percent` |
| `preferred next` | this account is the neutral next normal candidate, before affinity or accumulated fairness state | `preferred_next=true` |
| `available` | selectable in the current pool, but not the neutral preferred row | `preferred_next=false, availability=usable or reserve, selected_pool matches availability` |
| `held` | usable only after a higher-priority pool is empty | `preferred_next=false, availability lower than selected_pool` |
| `needs probe` | not selectable until background probe verifies quota | `availability=probe_required` |

Stable routing reason enum:

| Enum | Default human phrase |
| --- | --- |
| `preferred_weekly_healthier` | `preferred next: weekly healthier` |
| `preferred_short_reset_soon` | `preferred next: 5h reset soon` |
| `preferred_highest_weight` | `preferred next: safest quota` |
| `available_same_pool` | `available: same pool` |
| `held_reserve` | `held: reserve` |
| `probe_required_unknown_quota` | `needs probe: unknown quota` |
| `probe_required_missing_reset` | `needs probe: missing reset` |
| `probe_required_missing_window` | `needs probe: missing 5h or weekly` |
| `probe_required_no_data` | `needs probe` |
| `excluded_disabled` | `blocked: disabled` |
| `excluded_missing_credential` | `blocked: missing credential` |
| `blocked_window_exhausted` | `blocked: quota empty` |
| `blocked_window_ineligible` | `blocked: quota ineligible` |

Public reason mapping:

| Assessment outcome | Public `routing_reason` | Human phrase | `next use` |
| --- | --- | --- | --- |
| preferred usable account with healthier weekly pressure | `preferred_weekly_healthier` | `preferred next: weekly healthier` | `preferred` |
| preferred usable account because 5h reset is near | `preferred_short_reset_soon` | `preferred next: 5h reset soon` | `preferred` |
| preferred account by neutral selector weight | `preferred_highest_weight` | `preferred next: safest quota` | `preferred` |
| non-preferred account in the selected pool | `available_same_pool` | `available: same pool` | `available` |
| reserve account while a usable pool exists | `held_reserve` | `held: reserve` | `held` |
| disabled account | `excluded_disabled` | `blocked: disabled` | `blocked` |
| account with no active credential generation | `excluded_missing_credential` | `blocked: missing credential` | `blocked` |
| exhausted relevant window | `blocked_window_exhausted` | `blocked: quota empty` | `blocked` |
| ineligible relevant window | `blocked_window_ineligible` | `blocked: quota ineligible` | `blocked` |
| unknown relevant window | `probe_required_unknown_quota` | `needs probe: unknown quota` | `needs probe` |
| missing reset time | `probe_required_missing_reset` | `needs probe: missing reset` | `needs probe` |
| exactly one expected v1 5h or weekly window missing | `probe_required_missing_window` | `needs probe: missing 5h or weekly` | `needs probe` |
| no relevant route-band windows | `probe_required_no_data` | `needs probe` | `needs probe` |

Default human output must not contain:

- `account_id`
- raw internal score
- `pp`
- `bottleneck`
- provider token, router token, keychain identifier, or upstream auth header

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

- `route_band`
- `selected_pool`
- `weighted_candidates`
- `account_id`
- `safe_account_label`
- `availability`
- `freshness`
- `routing_exclusion`
- `next_use`
- `limiting_window`
- `quota_evidence_reason`
- `short_pressure`
- `long_pressure`
- `short_salvage`
- `long_salvage`
- `salvage_tie_key`
- `routing_reason`
- `routing_weight`
- `preferred_next`
- `window_slots`
- `windows`

JSON output may expose scores. Default table/plain output must not. JSON is a
debug/proof surface for the burn-down decision; it is not the product center.
The JSON schema must use stable enums for availability, limiting window,
freshness, routing reason, and probe-required state.

`window_slots` contains the exact human-display slot inputs for `5h` and
`weekly`: slot label, evidence state `known | unknown | no_data`, optional
`remaining_headroom`, optional `reset_unix_seconds`, optional
`reset_duration_seconds`, display note, and the source window ids included in
the slot. `windows` contains every relevant route-band window with safe
metadata only: window seconds, status, optional headroom, optional reset time,
observed time, effective flag, pressure, surplus, and whether that window
contributed to salvage. JSON must be able to reconstruct why the human table
rendered `unknown`, `no data`, `needs probe`, `held`, `available`, or
`preferred` without reading logs or raw provider DTOs.

`safe_account_label` is always sanitized for emission. If the configured label
looks like an email address, provider-derived identity, token, auth header, or
secret-store material, the renderer must replace it with a deterministic safe
hash/tag. Raw configured labels are not emitted by default status, logs, traces,
or smoke transcripts.

## Security And Trust Context

This design touches auth-adjacent account selection but does not expose secrets.

Assets:

- OAuth access tokens and refresh tokens
- router bearer token
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
- `codex-router-selection` may provide pure affinity key helpers, but it does
  not own durable state or provider credentials
- weighted burn-down fallback is allowed only when no previous-response
  affinity key is present

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
  credential, route-ineligible owner, or exhausted owner fail closed before
  weighted fallback
- a continuation request must never silently replay on a different account
- failure must be local and audit-safe, with no upstream open and no provider
  credential resolution for a different account

Proof must cover:

- HTTP/SSE continuation owner hit
- WebSocket continuation owner hit
- restart/durable owner lookup
- unknown previous-response owner
- disabled owner
- stale or unavailable credential generation
- route-ineligible or exhausted owner
- malformed affinity metadata
- no weighted fallback on any continuation-owner failure

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
   - accepted first frame type: `response.create`
   - locally read fields before selection: top-level `type` and top-level
     `previous_response_id` only
   - no route-band field is read from the first-frame body in v1; the
     `/v1/responses` WebSocket path fixes the route band to `responses`
   - full request-schema validation remains upstream-owned
5. Parse only the routing metadata needed for route-band assessment from the
   first frame; do not log the full payload.
6. Extract previous-response affinity metadata from the first frame, when
   present, before weighted fallback.
7. If affinity is present, apply the Previous-Response Affinity Contract. Any
   owner-resolution failure fails closed before weighted fallback.
8. If no affinity is present, run reset-aware route-band assessment for the
   `responses` route band and select from weighted candidates.
9. Resolve the selected account credential and inject upstream auth exactly once
   after selection and before upstream open.
10. Strip local client auth, router bearer auth, and hop-by-hop headers before
   upstream open.
11. Forward the accepted first frame and all later frames unchanged at the
   protocol payload layer.
12. Pin the selected account for the lifetime of the WebSocket connection. Do
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
| oversized first frame | yes | 0 | 0 | 0 | 0 | size class only |
| timed-out first frame | yes | 0 | 0 | 0 | 0 | timeout reason only |
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
- oversized or timed-out first frame fails before selection
- previous-response affinity is honored before weighted fallback and fails
  closed on owner-resolution failure
- malformed, wrong-type, oversized, timed-out, unsupported, auth-failed, and
  affinity-failed paths show zero selector advance, zero credential resolver
  calls, zero upstream auth injection, and zero upstream opens
- a local-field allowlist canary proves first-frame parsing reads only top-level
  `type` and top-level `previous_response_id` before selection; any
  non-allowlisted body field, including `model`, `input`, `metadata`, `tools`,
  prompt text, or request body content, must not affect route-band selection,
  logs, traces, or audit records before upstream-owned validation

Allowed and forbidden emission surfaces:

| Surface | Allowed | Forbidden |
| --- | --- | --- |
| default status rows | safe account label, status, percent-left bars, reset time, availability, routing reason | account id, OAuth tokens, router bearer token, keychain identifier, auth headers, raw score, unsafe label |
| JSON machine status | account id, safe account label, enum reasons, routing weight, freshness, reset metadata | OAuth tokens, router bearer token, refresh token, upstream auth headers, request/response body, unsafe label |
| selection explanation | safe account label or hash, route band, availability, reason code | provider tokens, bearer tokens, auth headers, secret-store paths, prompts, tool args |
| refresh errors | provider status class, redacted endpoint class, safe account label/hash | response bodies containing secrets, full auth headers, tokens |
| traces/logs | route band, availability, reason enum, safe account label/hash | token values, upstream auth headers, keychain identifiers, raw request/response body, full WebSocket first-frame payload, prompts, memory traces, tool args, unsafe labels |
| smoke transcripts | commands, redacted route band, reason enums, percentages/reset durations, selected safe label/hash | any token, full auth header, secret file/keychain material, raw request/response body, full WebSocket first-frame payload, prompts, memory traces, tool args, unsafe labels |

## Proof Expectations

The implementation plan must provide proof at these layers:

- pure assessment tests for per-window pressure, surplus, near-reset salvage, reserve, blocked, stale, and unknown behavior
- repository-backed selector tests using mixed 5h and weekly windows
- tests proving weekly pressure beats short-window urgency when weekly reset is far
- tests proving long-window near-reset salvage is allowed when reset is imminent
- tests proving unknown quota is probe-required and never enters
  `weighted_candidates`
- tests proving empty/no-window accounts are `probe_required`, not normal usable
- tests proving all-probe-required accounts fail fast without request-path
  provider I/O or request-path probe signaling
- tests proving missing reset time is conservative and receives no salvage
- tests proving mixed stale/unknown/ineligible collapse uses any-window conservative rules
- tests proving route-band batch assessment returns the same account ordering,
  selected pool, weighted candidates, and neutral `preferred_next` projection
  used by status
- tests proving the exact salvage tie key produces deterministic
  `weighted_candidates`, `preferred_next`, and empty-state
  `WeightedDeficitSelector` agreement when accounts tie on weight and pressure
- tests proving runtime exact selection may differ from `preferred_next` because
  of previous-response affinity, route-band account-hold cooldown, or
  accumulated weighted-deficit fairness state, and that default status does not
  claim runtime-exact next use
- tests proving stale penalty division is applied only inside the selected pool
  and reclamped after division
- tests proving canonical `account_id` order for deterministic selector inputs
- tests proving `probe_required` never competes with known `usable` or
  `reserve`
- tests proving all-probe-required status emits `needs probe` rows without
  implying healthy quota or fallback routing
- tests proving accounts with exactly one expected v1 window missing are
  `probe_required`, never normal usable, and render the missing slot as `no data`
- tests proving unknown, missing-reset, and no-window human slots never render
  fake `0% left`
- CLI renderer tests proving status uses the same assessment reason and limiting-window semantics as routing
- JSON schema tests for stable machine fields and enum values
- JSON schema tests proving `safe_account_label` is sanitized/hash-tagged and
  no unsafe configured label is emitted
- JSON schema tests proving machine output contains selected pool, next use,
  window slots, all relevant windows, reset metadata, and enough safe fields to
  reconstruct the default human status explanation
- plain renderer tests proving ASCII bars, no raw scores, no account ids, and
  the same routing phrases as table mode
- reason mapping tests from stable enum to human phrase
- safe-label and redaction canary tests for account labels that look like
  emails, provider identities, tokens, auth headers, or secret-store material
- CLI golden/snapshot tests for default human output:
  - healthy multi-account table with Unicode bars
  - limiting-window disagreement between 5h and weekly
  - reset-aware preferred-next explanation
  - probe-required or partial data
  - blocked, reserve, usable, and probe-required accounts
  - colorless/plain terminal mode
  - negative assertions for `pp`, `bottleneck`, default `account_id`, raw score, and token-like strings
- live-safe CLI smoke proof over persisted router state for emitted `table`,
  `plain`, and `json` status output, including redaction and negative
  assertions on the actual command output
- WebSocket compatibility tests for `/v1/responses` routing, selected-account
  pinning, previous-response affinity before weighted fallback, no weighted
  fallback on continuation-owner failure, `/v1/realtime` and unknown path
  `unsupported_path` pre-upgrade failure, malformed first frame failure,
  wrong-type first frame failure, oversized first frame failure, first-frame
  timeout failure, malformed affinity failure, and local-auth pre-upgrade
  failure before selection
- WebSocket redaction proof with a synthetic canary in first-frame/request-body
  content, proving audit/log/smoke artifacts do not contain the raw body or full
  first-frame payload
- WebSocket first-frame allowlist proof with canary values in non-allowlisted
  fields such as `model`, `input`, `metadata`, `tools`, and prompt text,
  proving only top-level `type` and top-level `previous_response_id` are read
  before selection
- WebSocket non-blocking proof that delayed or failing quota refresh does not
  block the first valid `/v1/responses` route after bounded first-frame parsing,
  and that selection uses persisted selector rows on that path
- background probe proof that unknown/no-data accounts are checked outside the
  request path, after startup/listen readiness, and that successful probe
  results become persisted selector rows used by later requests
- account-hold cooldown proof that adjacent normal requests reuse the held
  route-band account during the minimum cooldown, affinity bypasses the hold,
  and exhausted, blocked, disabled, credential-invalid, or probe-required held
  accounts are not reused
- security call-order tests proving every row in the WebSocket preselection
  failure matrix makes zero selector-state advances, zero credential resolver
  calls, zero upstream auth injections, and zero upstream opens
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
- redaction proof for status rows, machine status, selection explanations,
  refresh errors, traces/logs, and smoke transcripts

Non-blocking pass signal:

- The server must bind/listen without waiting for provider quota refresh.
- The first routed request must either route using persisted quota state or fail
  for an auth/upstream reason unrelated to quota-refresh waiting; it must not
  wait for live refresh or quota probe before selecting.
- `quota status` must render last-known persisted state immediately and may mark
  rows `needs probe`.
- Tests should synchronize on observable readiness, request completion, rendered
  output, or bounded fake-provider calls, not wall-clock sleeps.

## Open Decisions For Review

No open product decisions remain before the next spec review. This revision
chooses:

- weekly near-reset threshold: fixed v1 12h cap
- reserve traffic: zero normal traffic while any usable account exists
- assessment owner: `codex-router-selection::burn_down`
- CLI dependency: CLI may depend on `codex-router-selection`
- default account identifier: account label/tag only
- empty relevant window set: `probe_required`, not routable
- no effective row: compute from all windows and mark limiting window by worst
  pressure/headroom/longest-window order
- default human score visibility: no raw score
- route-band assessment owner: `codex-router-selection::burn_down`
- batch assessment contract: `BurnDownRouteBandAssessment` owns selected pool,
  weighted candidates, and neutral `preferred_next`
- unknown quota selection: not routable; background probe must verify quota and
  persist selector rows before later requests can use the account
- deterministic candidate order: `account_id` ascending
- WebSocket support: `/v1/responses` supported; every other WebSocket path is
  `unsupported_path` and fails closed before selection or upstream open
- WebSocket selection: valid first `response.create` frame is required before
  upstream open; selected account is pinned for connection lifetime
- previous-response affinity: applies to HTTP/SSE and WebSocket; owner
  resolution failures fail closed before weighted fallback
- default status surfaces: table/plain are human-only and JSON is explicit
  machine/debug output
- default account display: safe label or hash; JSON uses `safe_account_label`
  plus raw `account_id` only for explicit local machine/debug use
- smoke/log transcript policy: allowlisted fields only, no raw bodies, prompts,
  tool args, memory traces, unsafe labels, tokens, auth headers, or secret-store
  material

Spec review may still reject these choices, but plan creation must not reopen
them silently.

## Next Workflow

The corrected implementation plan now lives at
`tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`.
Run `shravan-dev-workflow:plan-review-swarm` against this spec and that plan.
Only if plan review has no accepted blockers should orchestrator transition to
`shravan-dev-workflow:implementation-execute-plan`.
