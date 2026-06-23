# Reset-Aware Burn-Down Routing Spec Review Ledger

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Coverage: 352 lines, read in chunks 1-140, 141-280, 281-420
Verdict: needs revision

## Review Packet

The review packet included:

- full spec path and line coverage
- current selector collapse anchors:
  - `crates/codex-router-proxy/src/account_selection.rs:227-347`
  - `crates/codex-router-selection/src/weighted_deficit.rs:60-98`
  - `crates/codex-router-selection/src/eligibility.rs:35-64`
  - `crates/codex-router-state/src/quota_snapshot.rs:91-200`
  - `crates/codex-router-state/src/repositories.rs:46-59`
  - `crates/codex-router-cli/src/quota.rs:924-1007`
- existing spec/plan anchors:
  - `docs/specs/2026-06-20-codex-router-greenfield-spec.md:129-153`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md:322-338`
- prior-art anchors:
  - DRR: `https://web.stanford.edu/class/ee384x/EE384X/papers/DRR.pdf`
  - RFC 2697 token-bucket meter: `https://datatracker.ietf.org/doc/html/rfc2697`
  - RFC 3290 Diffserv model: `https://www.rfc-editor.org/rfc/rfc3290.html`
  - Liu-Layland EDF/deadline scheduling: `https://www.cs.ru.nl/~hooman/DES/liu-layland.pdf`
  - GCRA explanatory reference: `https://brandur.org/rate-limiting`

Plugin refresh status: local installed `shravan-dev-workflow` cache is `1.6.29`.

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| algorithm-prior-art-crux | Meitner | answered | needs revision |
| requirements-testability + validation | Heisenberg | answered | needs revision |
| contract/scope + architecture-boundaries + spec-difference | Dirac | answered | needs revision |
| planning-readiness + adversarial-crux | Bacon | answered | needs revision |
| progressive-disclosure + UX/status + guardrails | Mendel | answered | needs revision |
| security-threat-model | Russell | errored | not completed |

Security lane note: the dedicated security lane crashed before findings, but requirements/validation and UX/guardrail lanes independently found redaction and observability proof gaps. A follow-up security lane is still recommended after the spec is revised.

## What Held

- The algorithm family is directionally right: state classification + burn-down assessment + weighted fairness.
- `WeightedDeficitSelector` should stay generic and receive scalar weights after quota assessment.
- Runtime selection and status UI should not duplicate separate quota math.
- Long-window quota, especially weekly quota, must dominate short-window reset urgency.
- Provider quota refresh must remain out of startup/request critical path.

## Accepted Findings

### A1. Burn-down score to selector weight is not formal enough

Severity: blocker

Evidence:

- Current selector only receives scalar `(AccountId, u32)` weights.
- Spec defines pressure/surplus and a risk-adjusted weight, but does not fully freeze sign semantics, normalization, tie-break sequence, or worked deterministic examples.

Failure path:

- Planner or implementer can map high pressure to a higher selector weight by mistake.
- Weekly and 5h windows can be compared with incompatible units.
- Scenario B remains non-testable because it says an account "may" outrank another.

Required revision:

- Define a normative selector contract:
  - dimensionless pressure quantity
  - sign semantics: higher pressure is worse
  - exact scalar reduction across windows
  - exact tie-break sequence
  - clamp and rounding rules
  - deterministic worked examples with expected winners

### A2. Shared ownership and dependency edges are under-specified

Severity: blocker

Evidence:

- Spec says assessment inputs include `SelectorQuotaInput windows`, then defines `Vec<QuotaWindowFact>`.
- It recommends `codex-router-selection` but does not say who adapts state DTOs into pure assessment DTOs.
- Current crate boundaries do not imply that `codex-router-selection` may depend on `codex-router-state`.

Failure path:

- Planning can choose incompatible designs:
  - make selection depend on state DTOs
  - duplicate assessment math in proxy and CLI
  - invent a new shared crate during implementation planning

Required revision:

- Add explicit dependency contract:
  - proxy/state adapter owns `SelectorQuotaInput -> QuotaWindowFact`
  - assessment module owns math and reason enums
  - selection crate must not depend on state DTOs or CLI formatting
  - CLI may consume shared assessment output, or a new shared contract owner must be named

### A3. Threshold and salvage policy is magic-number shaped

Severity: important

Evidence:

- Current spec names 12h, 30m, pressure 25, headroom 10, multiplier 3.
- It does not say whether they are fixed v1 behavior, config defaults, or later-tunable policy.

Failure path:

- Planner has to invent whether these are constants, config, telemetry, or tests-only fixtures.
- Bounded reset salvage can punch through weekly reserve without a cap/cooldown contract.

Required revision:

- Mark every threshold as one of:
  - fixed v1 constant with rationale and proof boundaries
  - config key with default, bounds, and precedence
- Define salvage cap and repeat/cooldown semantics.

### A4. Mixed window status collapse is unspecified

Severity: important

Evidence:

- Current code uses all windows for blocking/min headroom, but only the effective window for freshness.
- Spec requires shared freshness/routing semantics but does not say how mixed `Eligible`, `Stale`, `Unknown`, and `Ineligible` windows collapse.

Failure path:

- One implementation treats any stale window as stale; another follows effective only. Routing and status reasons drift.

Required revision:

- Define exact account-level collapse rules for:
  - ineligible
  - stale
  - unknown
  - missing reset
  - no effective row
  - empty window set

### A5. Human quota/status contract is not strict enough

Severity: important

Evidence:

- User-visible section has good intent but not enough mandatory vocabulary, forbidden terms, and golden proof obligations.

Failure path:

- The implementation can regress to noisy tables, ambiguous percentages, `pp`, `bottleneck`, missing selected-next explanation, or too many rows.

Required revision:

- Add normative vocabulary table.
- Split human output from machine output.
- Add guardrails:
  - at most two rows per account
  - `left` percent only
  - Unicode bars required for human output when supported
  - default human output must not use `pp` or `bottleneck`
  - explain selected-next when routing choice is shown
- Require golden/snapshot proof for historical bad cases.

### A6. Non-blocking startup/request proof is not black-box enough

Severity: important

Evidence:

- R1 says startup/request must not block on provider quota refresh, but the proof expectations do not define which observable operations must continue.

Failure path:

- A plan can prove only boot readiness while first routed request still blocks, or prove CLI status while runtime selection still blocks.

Required revision:

- Define non-blocking surfaces:
  - server boot/listen readiness
  - first routed request
  - quota status render
- Define allowed stale-state behavior and a latency/ordering-independent pass signal.

### A7. Redaction and observability proof is surface-incomplete

Severity: important

Evidence:

- Spec says no tokens in assessment/status, but does not enumerate emission surfaces.

Failure path:

- Status table is redacted, but selection reason, refresh errors, logs, or smoke transcript leaks token/account/subscription details.

Required revision:

- Add an observability/security proof table:
  - status rows
  - selection explanations
  - refresh errors
  - traces/logs
  - smoke transcripts
- For each surface, define allowed fields and forbidden data classes.

## Contested Or Human Decisions

These need explicit product/owner decisions before plan creation:

- Are thresholds fixed v1 constants or operator-configurable defaults?
- Should Scenario B be deterministic, and should B always win when weekly reset is within threshold?
- Should empty relevant-window sets be `unknown`, `blocked`, or excluded as unsupported?
- Is `cli -> codex-router-selection` an allowed dependency, or should assessment live in a separate shared crate?
- Should current effective-window freshness behavior be preserved or replaced by an any-window collapse rule?
- What account identifier is allowed in default status: label/tag only, masked id, or full label?

## Open Review Gaps

- Dedicated security-threat-model lane crashed and should be rerun after spec revision.
- The active worktree is dirty and locally diverged; review artifacts should be committed/pushed from a clean integration path if checkpointed.

## Verdict

Needs revision. Do not route directly to `plan-creation-swarm`.

Next workflow: return to `spec-creation-swarm` to revise the spec with accepted findings, then rerun `spec-review-swarm`.
