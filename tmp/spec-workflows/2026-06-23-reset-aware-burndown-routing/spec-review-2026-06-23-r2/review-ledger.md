# Reset-Aware Burn-Down Routing Spec Review Ledger R2

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `76a9e5c`
Coverage: 627 lines, read in chunks 1-160, 161-320, 321-480, 481-700
Verdict: needs revision

## Review Packet

The review packet included:

- full revised spec path and line coverage
- parent creation ledger:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/swarm-ledger.md`
- prior failed review:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/review-ledger.md`
- workflow state:
  `tmp/workflow-state/2026-06-23-quota-burndown-routing/details.md`
- current code anchors:
  - `crates/codex-router-proxy/src/account_selection.rs:180-347`
  - `crates/codex-router-selection/src/weighted_deficit.rs:60-98`
  - `crates/codex-router-selection/src/eligibility.rs:35-64`
  - `crates/codex-router-state/src/quota_snapshot.rs:91-269`
  - `crates/codex-router-state/src/repositories.rs:46-59`
  - `crates/codex-router-cli/src/quota.rs:756-1030`
  - `crates/codex-router-proxy/src/websocket.rs`
  - `crates/codex-router-proxy/src/local_auth.rs`
  - `crates/codex-router-proxy/src/headers.rs`
- existing spec anchors:
  - `docs/specs/2026-06-20-codex-router-greenfield-spec.md:88-100`
  - `docs/specs/2026-06-20-codex-router-greenfield-spec.md:404-425`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md:322-338`

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage | Lorentz | answered | needs revision |
| requirements-testability + validation + scoring crux | Hegel | answered | needs revision |
| contract/scope + architecture-boundaries + planning-readiness | Pascal | answered | needs revision |
| progressive-disclosure + UX/API/CLI + guardrails | Maxwell | answered | needs revision |
| security-threat-model + spec-difference | Huygens | answered | needs revision |

## What Held

- The burn-down algorithm direction is coherent enough to refine rather than
  redesign: pressure, salvage, durable weekly budget, and weighted fairness are
  the right design family.
- The dependency direction mostly holds: state remains persisted DTO owner;
  proxy and CLI are adapters; `WeightedDeficitSelector` stays generic.
- CLI depending on `codex-router-selection` is acceptable for this repo shape.
- The spec has no implementation task sequencing that would itself block
  planning.

## Accepted Findings

### R2-A1. Batch route-band assessment owner is missing

Severity: blocker

Evidence:

- The spec defines a per-account `BurnDownAssessmentInput`.
- The same spec also requires account-set behavior: known-fresh alternatives,
  selected availability pool, `selected_next`, and weighted candidates.
- Current code computes sibling-account context in proxy
  `account_selection.rs:231`.

Failure path:

- Plan creation either leaves cross-account quota math in proxy/CLI, violating
  the shared-assessment boundary, or invents a batch API not present in the
  spec.

Required revision:

- Add a `BurnDownRouteBandAssessment` or equivalent batch contract owned by
  `codex-router-selection::burn_down`.
- Make proxy and CLI adapters only.
- Define selected pool, `selected_next`, weighted candidates, route-band
  account order, and fallback behavior in that batch contract.

### R2-A2. Unknown fallback semantics conflict with freshness penalties

Severity: blocker

Evidence:

- Requirements and policy text preserve stale/unknown penalties against known
  fresh alternatives.
- Availability routing says `unknown` enters only after no usable or reserve
  accounts exist.

Failure path:

- A planner can implement unknown as same-pool `headroom / 8`, as fallback
  weight 1, as headroom-ranked fallback, or as missing-reset pressure, producing
  different behavior and tests.

Required revision:

- Make availability pool isolation authoritative for v1.
- Unknown accounts never compete while any usable or reserve account exists.
- Define exact unknown fallback weight and remove or re-scope the legacy
  `unknown / 8 when fresh exists` wording.
- Define post-penalty clamp/rounding for stale accounts if stale remains in
  the usable/reserve pool.

### R2-A3. WebSocket v1 routing/security contract is too generic

Severity: blocker

Evidence:

- Revised spec only requires e2e proof "including WebSocket behavior".
- Greenfield spec treats WebSocket as core v1 behavior with first-frame routing,
  bounded metadata, unchanged forwarding, connection pinning, and fail-closed
  unsupported routes.
- Current WebSocket code reads a first frame and routes through local auth,
  selection, resolver, sanitized headers, and unchanged forwarding.

Failure path:

- Planning can satisfy the spec with a broad WebSocket smoke while omitting
  reset-aware WebSocket selection, first-frame behavior, route-band semantics,
  malformed-frame fail-closed proof, or connection pinning.

Required revision:

- Add a WebSocket compatibility/security subsection:
  local auth first; unsupported WebSocket routes fail closed before selection;
  bounded first `response.create` frame is required before upstream open;
  reset-aware selection uses the `responses` route band; credential resolution
  and upstream auth injection happen only after selection; first and later frames
  forward unchanged; selected account is pinned for the connection; no mid-stream
  account switching.
- Proof must cover malformed first frame does not advance selector state,
  resolve credentials, or open upstream.

### R2-A4. Human and machine status surfaces remain underspecified

Severity: important

Evidence:

- Default human output says one rendered row per account while the example uses
  continuation lines.
- Unicode support is conditional but fallback behavior is not frozen.
- Machine output "may include" fields but does not define a format or stable
  schema.
- `routing_reason` has no enum/prose mapping.

Failure path:

- Plan creation can produce noisy output, overload `plain` as machine output,
  leak raw scores or account ids into the wrong surface, or invent ad hoc
  reason strings.

Required revision:

- Define logical account rows versus physical display lines.
- Define default Unicode table and explicit fallback/machine format policy.
- Define machine format name and required schema, or explicitly defer machine
  output.
- Add stable routing reason enum and human phrase mapping.

### R2-A5. Security/threat model proof is not complete enough

Severity: important

Evidence:

- The spec has a useful emission table, but it does not enumerate entry points,
  trust boundaries, and proof rows for local-auth-before-selection and
  upstream-auth-after-selection.
- It treats account labels as safe without a local-safe/redacted display-label
  contract.
- It forbids tokens in smoke transcripts but not raw request/response bodies,
  WebSocket first-frame payloads, prompts, memory traces, tool arguments, or
  unsafe labels.

Failure path:

- Implementation can call selector/resolver before local auth, leak raw labels
  or payloads in logs/smokes, or copy machine output into proof artifacts.

Required revision:

- Add threat model bullets for assets, entry points, trust boundaries,
  untrusted inputs, privileged actions, and proof rows.
- Define safe account display label semantics.
- Allow raw account id only in explicit local machine/debug output, not default
  human output, proxy audit, logs/traces, or smoke transcripts.
- Add schema-allowlisted smoke/log transcript contract.

### R2-A6. Scenario fixtures still need deterministic data

Severity: important

Evidence:

- Scenario D omits weekly reset facts even though missing reset makes an
  account `unknown`.
- Scenario A says "outranks" while the worked example makes B `reserve`, so the
  reason is pool precedence, not scalar weight.

Failure path:

- Plan creation writes tests for the wrong comparator or invents missing reset
  fixtures.

Required revision:

- Complete Scenario D with weekly reset facts and expected pressure/salvage/
  weight values.
- Reword Scenario A as "A is selected because usable pool precedes reserve;
  B is reserve."
- Clarify Scenario E as fallback isolation, not same-pool `unknown / 8`.

## Contested Or Human Decisions

No outer-loop human decision is required before another spec revision. The
parent reducer chooses these defaults for v1:

- Unknown accounts are fallback-only and do not use the legacy same-pool
  `unknown / 8` rule.
- Stale accounts may remain in their computed availability pool with a
  floor-clamped penalty when fresh alternatives exist in the same pool.
- Machine JSON may include raw `account_id` only when explicitly requested, and
  must not be copied into default logs/smoke artifacts without redaction.
- Default human output uses a logical row per account and may render a second
  physical continuation line without repeating account/status.

## Verdict

Needs revision. Do not route to `plan-creation-swarm`.

Next workflow: `shravan-dev-workflow:spec-creation-swarm`.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r2/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: Accepted blocker findings require one more spec revision before planning.
