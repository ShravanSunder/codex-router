# R14 Whole-Spec Coverage + Planning-Readiness Lane

Date: 2026-06-23
Reviewer lane: Galileo (`019ef5f4-5ad4-73f2-ad66-78185f75bcf6`)
Spec: `../reset-aware-burndown-routing-spec.md`
Parent baseline: `f104ff9`

## Coverage Receipt

Reviewed the full reset-aware burn-down routing spec as a planning-readiness
artifact, with focus on whether a future implementation agent could execute
without redefining requirements, scope, or public contracts.

Parent coverage verified the spec at 1788 lines and read chunks:

- 1-320
- 321-640
- 641-960
- 961-1280
- 1281-1600
- 1601-1788

## Verdict

needs_revision

The spec contains the right product direction, but it is not yet safe to route
to plan creation because several implementation surfaces are silently included
without being promoted into top-level requirements and proof obligations.

## Accepted Findings

### BLOCKER: Scope Trace Breaks Between Product Requirements and Technical Cutovers

The top-level product requirements center quota burn-down routing, fast startup,
status UX, and background refresh. Later sections require additional cutovers:

- affinity secret hashing
- local router token carrier policy
- generated Codex profile shape
- WebSocket first-frame security restrictions
- smoke transcript redaction

Those cutovers may be valid prerequisites, but the spec does not make them
first-class requirements or explicitly split them out as separate prerequisite
slices. A planner would have to guess whether they are required for this goal,
security cleanup piggybacking on the goal, or future work.

Refinement needed:

- Add a top-level requirement row for each required non-quota cutover, or move
  it out of this goal.
- For each retained cutover, state why it is required for reset-aware routing.
- Add direct proof rows that trace from the top-level requirement to code and
  smoke evidence.

### BLOCKER: Route-Level Result and Envelope Contract Is Internally Inconsistent

The spec uses several overlapping route-result concepts:

- `preferred_next`
- `preferred_next_account_id`
- route-level unsupported-band result
- `route_result` in unsupported payload prose
- JSON/status schemas that do not define all of the above consistently

This blocks planning because route selection, CLI status, JSON output, proxy
responses, and tests would each have to infer a different shape.

Refinement needed:

- Define one canonical route-level result envelope.
- Use one field name for the preferred next account.
- Include unsupported route-band results, unknown/fallback results, and normal
  selected-pool results in the same envelope.
- Update every status, JSON, and proof section to use that envelope.

### IMPORTANT: Refresh Staleness Semantics Need an Owner and Formula

The spec introduces durable refresh status such as
`quota_refresh_status.stale_after_unix_seconds`, but does not define the exact
freshness formula or owning module.

Refinement needed:

- Name the owner that calculates staleness.
- Define how `stale_after_unix_seconds` is derived.
- Define how startup, scheduled refresh, failed refresh, and manual refresh
  update freshness.

## What Held

- Fast startup must not synchronously block on quota refresh.
- Last-known quota must remain usable for startup and selection.
- The user-facing quota status should collapse to account-level 5h/weekly rows
  with bars and useful routing notes.
- Weekly quota must be weighted as the long-horizon limiter.

## Recommended Route

Return to `shravan-dev-workflow:spec-creation-swarm` before plan creation.
