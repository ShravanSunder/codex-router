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

The selector uses persisted SQLite selector windows. Background refresh may update those windows, but startup and request selection do not wait on live provider I/O.

R2. Route-band-relevant exhausted windows block the account.

For the requested route band, an account is not normally selectable if any relevant quota window is `Ineligible` or has `remaining_headroom == 0`.

R3. Unknown quota is not free capacity.

Unknown accounts never compete with known `usable` or `reserve` accounts. If
only unknown accounts remain, partial headroom evidence may order the fallback
pool conservatively, but missing reset times receive no reset-salvage bonus.

R4. Weekly or other long-window quota is durable budget.

Long-window pressure dominates short-window reset urgency. An account with low weekly headroom and a far weekly reset must not be preferred merely because its 5h window has more headroom or resets sooner.

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
- No changes to `WeightedDeficitSelector` that make it know quota-window semantics.

## Spec Boundary / Separability Map

```text
provider quota refresh
  owns: provider fetch and normalization
  writes: persisted selector quota windows

persisted selector quota windows
  owns: last-known per-account, per-route-band, per-window quota facts
  exposes: SelectorQuotaRepository::selector_inputs_for_route_band(route_band)

burn-down assessment
  owns: reset-aware pressure math and structured explanation
  crate: codex-router-selection::burn_down
  inputs: BurnDownRouteBandAssessmentInput, fixed v1 route band policy
  exposes: BurnDownRouteBandAssessment { accounts, selected_pool, weighted_candidates, preferred_next }
  must not depend on: codex-router-state, codex-router-proxy, codex-router-cli

proxy account selection
  owns: route classification, account eligibility, previous-response affinity,
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
  `BurnDownRouteBandAssessmentInput`, then feeds the selected pool's positive
  scalar weights into `WeightedDeficitSelector`.
- `codex-router-cli` may depend on `codex-router-selection` so status and
  routing share the same assessment output. It owns formatting only.
- Reimplementing pressure, reserve, unknown, or limiting-window math in the CLI
  or proxy is out of contract.

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

The assessment first applies account and window collapse before weight math:

1. If the account is not enabled, exclude it from routing. This is not a quota
   availability class.
2. If there is no active credential generation, exclude it from routing. This
   is not a quota availability class.
3. If there are no relevant route-band windows, return `unknown` with
   `routing_weight = 1` and reason `needs_quota_refresh`.
4. If any relevant window is `Ineligible`, return `blocked` with reason
   `window_ineligible`.
5. If any relevant window has `remaining_headroom == 0`, return `blocked` with
   reason `window_exhausted`.
6. If any relevant window is `Unknown`, return `unknown` with reason
   `unknown_quota_window`.
7. If any relevant non-blocked window has no reset time, return `unknown` with
   reason `missing_reset_time`.
8. If at least one relevant window is `Stale` and none are blocked or unknown,
   compute burn-down normally and mark freshness as `stale`.
9. If every relevant window is `Eligible`, compute burn-down normally and mark
   freshness as `fresh`.

Freshness collapse is any-window conservative:

- one stale window makes the account stale
- one unknown or missing-reset window makes the account unknown
- one ineligible or exhausted window blocks the account
- the `effective` marker never overrides a worse relevant window

### Availability Classes

The assessment returns exactly one availability class:

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

- `accounts: Vec<BurnDownAccountAssessment>`
- `selected_pool: usable | reserve | unknown | none`
- `weighted_candidates: Vec<(AccountId, u32)>`
- `preferred_next: Option<AccountId>`
- `account_order: account_id ascending`

`BurnDownAccountAssessment`:

- `account_id`
- `account_label`
- `availability`
- `freshness: fresh | stale | unknown`
- `limiting_window`
- `short_pressure`
- `long_pressure`
- `short_salvage`
- `long_salvage`
- `routing_weight`
- `routing_reason`
- `preferred_next`

Only `usable` accounts enter the normal weighted-deficit pool. If no usable
account exists, `reserve` accounts may enter. `blocked` accounts never enter.
`unknown` accounts enter only when no known usable or reserve account exists.

Pool order is normative:

1. Build assessments for every enabled account with an active credential.
2. If one or more `usable` accounts exist, select only from `usable`.
3. Else if one or more `reserve` accounts exist, select only from `reserve`.
4. Else if one or more `unknown` accounts exist, select only from `unknown`.
5. Else return no eligible account.

Within the selected pool, candidates are ordered by `account_id` ascending
before they are passed to `WeightedDeficitSelector`. Tests that need a
deterministic selected winner use the same order and an empty selector state.

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

- `routing_reason = needs_quota_refresh`
- unknown candidates never receive reset-salvage bonus
- if at least one known relevant window has headroom, compute
  `routing_weight = clamp(min_known_headroom - reserve_pressure_threshold, 1..100)`
- if no usable partial headroom exists, use `routing_weight = 1`
- unknown candidates are ordered by `routing_weight` descending, then
  `account_id` ascending
- unknown never competes with usable or reserve accounts in v1

`preferred_next` semantics:

- `preferred_next` is a neutral projection from the pure assessment using an
  empty weighted-deficit state and no previous-response affinity key.
- It answers "which account is preferred for the next normal non-affinity
  request if fairness has no accumulated deficit?"
- It is not the runtime-exact next request. The proxy may choose a different
  account when previous-response affinity is present or when accumulated
  weighted-deficit state rotates to a lower-weight account for fairness.
- Default status must label this as `preferred next`, not `selected next`.
- Runtime audit may additionally log the actual selected account after affinity
  and weighted-deficit selection.

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
  from empty state and provide candidates in canonical `account_id` order.
- For a pure assessment tie, the stable explanation order is:
  availability pool, higher `routing_weight`, lower `long_pressure`, lower
  `short_pressure`, earlier near-reset salvage, then `account_id`.

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
- `next use`: one of `preferred`, `held`, `blocked`, or `needs refresh`

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
- both modes include a numeric percent with `left`, for example `54% left`

Normative vocabulary:

| Term | Human meaning | Machine field |
| --- | --- | --- |
| `usable` | can be selected normally | `availability=usable` |
| `reserve` | held while a usable account exists | `availability=reserve` |
| `blocked` | cannot be selected for this route band | `availability=blocked` |
| `unknown` | needs refresh; fallback only | `availability=unknown` |
| `limiting window` | the 5h or weekly window driving the decision | `limiting_window` |
| `pressure` | quota is being spent faster than reset pace | `pressure_percent` |
| `preferred next` | this account is the neutral next normal candidate, before affinity or accumulated fairness state | `preferred_next=true` |
| `held` | usable only after higher-priority pool is empty | `preferred_next=false` |

Stable routing reason enum:

| Enum | Default human phrase |
| --- | --- |
| `preferred_weekly_healthier` | `preferred next: weekly healthier` |
| `preferred_short_reset_soon` | `preferred next: 5h reset soon` |
| `preferred_highest_weight` | `preferred next: safest quota` |
| `held_reserve` | `held: reserve` |
| `held_unknown` | `held: needs refresh` |
| `blocked_window_exhausted` | `blocked: quota empty` |
| `blocked_window_ineligible` | `blocked: quota ineligible` |
| `needs_quota_refresh` | `needs refresh` |

Default human output must not contain:

- `account_id`
- raw internal score
- `pp`
- `bottleneck`
- provider token, router token, keychain identifier, or upstream auth header

Example shape:

```text
account  status   5h                         weekly                     routing                         next use
askluna  enabled  ██████████ 100% left        ░░░░░░░░░░ 0% left          blocked: weekly empty           blocked
                  resets in 4h 55m            resets in 1d 11h
matches  enabled  █████████░ 91% left         █████░░░░░ 54% left         preferred next: weekly healthier preferred
                  resets in 4h 8m             resets in 5d 22h
ssdev    enabled  ██████████ 100% left        ██░░░░░░░░ 16% left         held: weekly reserve             held
                  resets in 3h 48m            resets in 1d 9h
```

JSON output schema:

- `account_id`
- `safe_account_label`
- `availability`
- `freshness`
- `limiting_window`
- `short_pressure`
- `long_pressure`
- `short_salvage`
- `long_salvage`
- `routing_reason`
- `routing_weight`
- `preferred_next`

JSON output may expose scores. Default table/plain output must not. The JSON
schema must use stable enums for availability, limiting window, freshness, and
routing reason.

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
- no weighted fallback on any continuation-owner failure

### WebSocket Compatibility Contract

The v1 router must support the Codex `/v1/responses` WebSocket path. It uses the
same `responses` route band as HTTP/SSE response creation.

WebSocket routing order is normative:

1. Validate local router auth before selector state, quota assessment,
   credential resolution, or upstream connection open.
2. Fail closed with `unsupported_path` for any WebSocket path other than
   `/v1/responses` before selector state, quota assessment, credential
   resolution, or upstream connection open. `/v1/realtime` and any unknown path
   are representative `unsupported_path` cases in v1.
3. Require a bounded first client frame containing a valid `response.create`
   payload before opening the upstream WebSocket.
4. Parse only the routing metadata needed for route-band assessment from the
   first frame; do not log the full payload.
5. Extract previous-response affinity metadata from the first frame, when
   present, before weighted fallback.
6. If affinity is present, apply the Previous-Response Affinity Contract. Any
   owner-resolution failure fails closed before weighted fallback.
7. If no affinity is present, run reset-aware route-band assessment for the
   `responses` route band and select from weighted candidates.
8. Resolve the selected account credential and inject upstream auth exactly once
   after selection and before upstream open.
9. Strip local client auth, router bearer auth, and hop-by-hop headers before
   upstream open.
10. Forward the accepted first frame and all later frames unchanged at the
   protocol payload layer.
11. Pin the selected account for the lifetime of the WebSocket connection. Do
    not switch accounts mid-stream. A later connection may reselect after quota
    state changes.

Malformed first frames, unsupported WebSocket routes, and failed local auth must
advance no selector state, resolve no provider credential, inject no upstream
auth, and open no upstream connection.

Required WebSocket proof:

- valid `/v1/responses` WebSocket routes through reset-aware selection
- selected account is pinned for the full WebSocket connection
- `/v1/realtime` and one unknown WebSocket path fail closed as
  `unsupported_path` before selection
- invalid local auth fails before selection
- malformed first frame fails before selection
- previous-response affinity is honored before weighted fallback and fails
  closed on owner-resolution failure
- malformed/unsupported/auth-failed paths show zero selector advance, zero
  credential resolver calls, and zero upstream opens

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
- tests proving unknown quota loses to known healthy quota
- tests proving unknown quota is fallback-only and never competes with known
  `usable` or `reserve` accounts
- tests proving empty/no-window accounts are `unknown` fallback, not normal usable
- tests proving missing reset time is conservative and receives no salvage
- tests proving mixed stale/unknown/ineligible collapse uses any-window conservative rules
- tests proving route-band batch assessment returns the same account ordering,
  selected pool, weighted candidates, and neutral `preferred_next` projection
  used by status
- tests proving runtime exact selection may differ from `preferred_next` because
  of previous-response affinity or accumulated weighted-deficit fairness state,
  and that default status does not claim runtime-exact next use
- tests proving stale penalty division is applied only inside the selected pool
  and reclamped after division
- tests proving canonical `account_id` order for deterministic selector inputs
- tests proving unknown fallback never competes with known `usable` or
  `reserve`, but preserves conservative partial-headroom ordering inside the
  all-unknown fallback pool
- CLI renderer tests proving status uses the same assessment reason and limiting-window semantics as routing
- JSON schema tests for stable machine fields and enum values
- JSON schema tests proving `safe_account_label` is sanitized/hash-tagged and
  no unsafe configured label is emitted
- plain renderer tests proving ASCII bars, no raw scores, no account ids, and
  the same routing phrases as table mode
- reason mapping tests from stable enum to human phrase
- safe-label and redaction canary tests for account labels that look like
  emails, provider identities, tokens, auth headers, or secret-store material
- CLI golden/snapshot tests for default human output:
  - healthy multi-account table with Unicode bars
  - limiting-window disagreement between 5h and weekly
  - reset-aware preferred-next explanation
  - unknown or partial data
  - blocked, reserve, usable, and unknown accounts
  - colorless/plain terminal mode if supported
  - negative assertions for `pp`, `bottleneck`, default `account_id`, raw score, and token-like strings
- live-safe CLI smoke proof over persisted router state for emitted `table`,
  `plain`, and `json` status output, including redaction and negative
  assertions on the actual command output
- WebSocket compatibility tests for `/v1/responses` routing, selected-account
  pinning, previous-response affinity before weighted fallback, no weighted
  fallback on continuation-owner failure, `/v1/realtime` and unknown path
  `unsupported_path` failure, malformed first frame failure, and local-auth
  failure before selection
- WebSocket non-blocking proof that delayed or failing quota refresh does not
  block the first valid `/v1/responses` route after bounded first-frame parsing,
  and that selection uses persisted selector rows on that path
- security call-order tests proving failed local auth, unsupported WebSocket
  route, and malformed first frame make zero selector-state advances, zero
  credential resolver calls, and zero upstream opens
- black-box non-blocking proof for:
  - server boot/listen readiness while provider refresh is delayed or failing
  - first routed request while provider refresh is delayed or failing
  - quota status render using persisted data while refresh is delayed or failing
- end-to-end Codex-through-router proof, including WebSocket behavior, before
  implementation completion can be claimed
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

## Open Decisions For Review

No open product decisions remain before the next spec review. This revision
chooses:

- weekly near-reset threshold: fixed v1 12h cap
- reserve traffic: zero normal traffic while any usable account exists
- assessment owner: `codex-router-selection::burn_down`
- CLI dependency: CLI may depend on `codex-router-selection`
- default account identifier: account label/tag only
- empty relevant window set: `unknown` fallback
- no effective row: compute from all windows and mark limiting window by worst
  pressure/headroom/longest-window order
- default human score visibility: no raw score
- route-band assessment owner: `codex-router-selection::burn_down`
- batch assessment contract: `BurnDownRouteBandAssessment` owns selected pool,
  weighted candidates, and neutral `preferred_next`
- unknown quota selection: fallback-only, never competing with known `usable` or
  `reserve`, with conservative partial-headroom ordering inside the all-unknown
  pool
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

Run `shravan-dev-workflow:spec-review-swarm` against this revised spec. Only if
that review returns `phase_result: complete` should orchestrator transition to
`shravan-dev-workflow:plan-creation-swarm`.
