# Reset-Aware Burn-Down Routing Spec

Date: 2026-06-23
Status: draft for spec review
Scope: codex-router quota burn-down assessment, runtime account selection, and quota status explanation

## Product Intent

Codex-router should pick the next account using quota survivability, not only raw remaining quota. A user with several OAuth accounts needs the router to protect the account that would be stranded longest if depleted, while still using quota that is about to reset instead of wasting it.

Success means the router can answer these questions consistently:

- Which account is usable now?
- Which account should be selected next?
- Which window is limiting the account: 5h or weekly?
- Is low remaining quota dangerous, or is it safe because reset is imminent?
- Is the status display explaining the same decision the runtime selector is making?

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
- Existing spec already says weekly quota must be protected before short-window reset urgency.
  Source: `docs/specs/2026-06-20-codex-router-greenfield-spec.md:147-151`.
- Existing plan already calls for selection order: eligibility/freshness, long-window pressure, effective bottleneck headroom, reset urgency as bounded tiebreaker.
  Source: `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md:322-338`.

## Requirements

R1. Startup and request routing must not block on provider quota refresh.

The selector uses persisted SQLite selector windows. Background refresh may update those windows, but startup and request selection do not wait on live provider I/O.

R2. Route-band-relevant exhausted windows block the account.

For the requested route band, an account is not normally selectable if any relevant quota window is `Ineligible` or has `remaining_headroom == 0`.

R3. Unknown quota is not free capacity.

If known fresh accounts exist, stale and unknown accounts are penalized before selection. Unknown or missing reset times must not make an account look safer than an account with known healthy quota.

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
  inputs: SelectorQuotaInput windows, now_unix_seconds, route band policy
  exposes: BurnDownAssessment { availability, risk, weight, reasons }

proxy account selection
  owns: route classification, account eligibility, process-lifetime fairness state
  consumes: BurnDownAssessment.routing_weight
  exposes: SelectedAccountDecision

weighted deficit selector
  owns: generic weighted fairness state
  consumes: (AccountId, u32)
  must not know: windows, weekly quota, reset time, CLI formatting

quota status CLI
  owns: human and machine rendering
  consumes: persisted windows plus BurnDownAssessment explanation
  must not own: routing math
```

## Burn-Down Assessment Contract

### Inputs

`BurnDownAssessmentInput`:

- `account_id`
- `route_band`
- `now_unix_seconds`
- `windows: Vec<QuotaWindowFact>`
- `freshness_policy`
- `route_band_policy`

`QuotaWindowFact`:

- `window_seconds`
- `status`
- `remaining_headroom`, clamped to 0..100
- `reset_unix_seconds`
- `observed_unix_seconds`
- `effective`

Window classification:

- `short_window`: window is shorter than 24h. The known 5h window is short.
- `long_window`: window is at least 24h, or the longest relevant window when no explicit weekly label exists. The known 604800 second window is weekly.
- `effective_window`: provider-selected or router-selected limiting row, but assessment must still inspect every relevant window.

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
- set `pressure_percent` to an unknown-reset penalty bucket
- do not grant reset-salvage bonus

### Availability Classes

The assessment returns exactly one availability class:

- `blocked`: at least one relevant window is ineligible or exhausted.
- `reserve`: account is not exhausted, but long-window pressure is dangerous enough that it should be used only when no normal account is available.
- `usable`: account can be selected normally.
- `unknown`: account has insufficient quota evidence and is selectable only as a fallback when no known usable account exists.

Default reserve conditions:

- any long window has `pressure_percent >= 25`, and its reset is not within the long-window near-reset threshold
- or any long window has `remaining_headroom <= 10`, and its reset is not within the long-window near-reset threshold

Near-reset thresholds are policy values, not hard-coded provider facts:

- 5h near reset: at most 30 minutes, or 10% of the window, whichever is smaller
- weekly near reset: at most 12 hours, or 10% of the window, whichever is smaller

### Routing Weight

Only `usable` accounts enter the normal weighted-deficit pool. If no usable account exists, `reserve` accounts may enter. `blocked` accounts never enter. `unknown` accounts enter only when no known usable or reserve account exists.

For each selectable account:

```text
usable_headroom = min(remaining_headroom across relevant windows)
long_pressure = max(pressure_percent across long windows)
all_pressure = max(pressure_percent across all windows)
short_salvage = sum bounded surplus_percent for near-reset short windows
long_salvage = sum bounded surplus_percent for near-reset long windows

risk_adjusted_weight =
  usable_headroom
  - (3 * long_pressure)
  - all_pressure
  + bounded(short_salvage)
  + bounded(long_salvage when long reset is near)
```

Then:

- clamp `risk_adjusted_weight` to `1..100` for selectable accounts
- apply freshness penalties after burn-down scoring:
  - stale with known fresh alternatives: divide by 4
  - unknown with known fresh alternatives: divide by 8
- pass `(AccountId, risk_adjusted_weight)` into `WeightedDeficitSelector`

Rationale:

- long pressure has a larger penalty because weekly depletion strands the account longer
- short salvage can help use quota before it expires
- long salvage applies only when long reset is near, so low weekly quota far from reset is not excused
- weighted deficit still balances among similarly safe candidates instead of turning routing into strict single-account priority

## Required Scenario Contracts

### Scenario A: low 5h, healthy weekly, 5h reset soon

```text
A: 5h 5% left, resets in 2m; weekly 80% left, resets in 5d
B: 5h 90% left, resets in 4h; weekly 20% left, resets in 5d
```

Expected: A outranks B.

Reason: B's weekly pressure is durable-budget risk. A's low 5h is acceptable because the short window resets soon and weekly is healthy.

### Scenario B: low weekly, weekly reset soon

```text
A: 5h 5% left, resets in 2m; weekly 80% left, resets in 5d
B: 5h 90% left, resets in 4h; weekly 20% left, resets in 10m
```

Expected: B may outrank A.

Reason: B's low weekly quota is near reset, so the durable-budget risk is about to disappear. This is the bounded long-window salvage case.

### Scenario C: weekly empty

```text
A: 5h 80% left; weekly 0% left
B: 5h 42% left; weekly 42% left
```

Expected: A is blocked for the route band until weekly reset; B is selected if otherwise eligible.

### Scenario D: same weekly, different short reset pressure

```text
A: 5h 30% left, resets in 10m; weekly 60% left
B: 5h 30% left, resets in 4h; weekly 60% left
```

Expected: A outranks B or receives meaningfully higher weight.

Reason: A has near-reset short-window quota that is safe to spend. B is under more short-window pressure because 30% must last much longer.

### Scenario E: unknown versus known healthy

```text
A: known fresh 50% 5h, 50% weekly
B: unknown 90% 5h, unknown weekly
```

Expected: A outranks B.

Reason: unknown quota is not free capacity when known healthy quota exists.

## User-Visible Status Contract

The human quota status view should answer "what can I use now?" without leaking internal score math.

Default table should be account-centric, with no more than two rendered lines per account. Recommended columns:

- `account`
- `status`
- `5h left`
- `5h resets`
- `weekly left`
- `weekly resets`
- `pace`
- `routing`

Wording:

- use `left`, never ambiguous bare percent
- avoid `pp`
- avoid `bottleneck` in default output
- use `limiting window`, `weekly pressure`, `5h pressure`, `selected next`, `held`, `blocked`, `needs refresh`
- show Unicode bars in the Rust app's human table when the terminal supports them

Machine output may include structured fields:

- `availability`
- `limiting_window`
- `long_pressure`
- `short_pressure`
- `reset_salvage`
- `freshness`
- `routing_reason`
- `selected_next`

Machine output may expose scores. Default human output should not.

## Security And Trust Context

This design touches auth-adjacent account selection but does not expose secrets.

Security-sensitive invariants:

- selector assessment must consume account ids and quota facts only, never OAuth access tokens or refresh tokens
- status output must not print provider tokens, router bearer tokens, keychain identifiers, or upstream auth headers
- stale/unknown quota must be conservative to avoid overusing a newly rotated or invalidated account
- account selection must remain after local router auth and before upstream auth injection

## Proof Expectations

The implementation plan must provide proof at these layers:

- pure assessment tests for per-window pressure, surplus, near-reset salvage, reserve, blocked, stale, and unknown behavior
- repository-backed selector tests using mixed 5h and weekly windows
- tests proving weekly pressure beats short-window urgency when weekly reset is far
- tests proving long-window near-reset salvage is allowed when reset is imminent
- tests proving unknown quota loses to known healthy quota
- CLI renderer tests proving status uses the same assessment reason and limiting-window semantics as routing
- live or smoke proof that request routing still does not block on provider quota refresh

## Open Decisions For Review

OD1. Should default weekly near-reset threshold be 12h or 24h?

Recommendation: start with 12h. It is conservative enough to protect weekly quota while still allowing same-day reset salvage.

OD2. Should reserve accounts receive zero normal traffic or a tiny trickle?

Recommendation: zero normal traffic while any usable account exists. Weighted trickle can be added later if live data shows starvation or stale reserve classification.

OD3. Should the burn-down assessment live in `codex-router-selection` or a new small crate?

Recommendation: start in `codex-router-selection` as a pure module. Split later only if dependency direction forces it.

OD4. Should status display the internal risk score?

Recommendation: no in default human output; yes in machine/debug output.

## Next Workflow

Run `shravan-dev-workflow:spec-review-swarm` against this spec. After accepted feedback is folded in, run `shravan-dev-workflow:plan-creation-swarm` to convert it into implementation tasks and proof gates.
