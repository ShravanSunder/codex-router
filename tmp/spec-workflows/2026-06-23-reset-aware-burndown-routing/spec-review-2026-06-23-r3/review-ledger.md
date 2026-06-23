# Reset-Aware Burn-Down Routing Spec Review Ledger R3

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `ab89b2bb4e67a2e327a6dfb253cf7de1241ab8f5`
Coverage: 837 lines, read by parent in chunks 1-170, 171-340, 341-510, 511-680, 681-837
Verdict: needs revision

## Review Packet

The review packet included:

- full second-revision spec path and line coverage
- prior R2 review ledger:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r2/review-ledger.md`
- workflow state:
  `tmp/workflow-state/2026-06-23-quota-burndown-routing/details.md`
- code anchors for selector fairness, affinity, WebSocket routing, state rows,
  account metadata, and CLI quota rendering

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage + progressive-disclosure | Franklin | answered | needs revision |
| requirements-testability + validation-and-testability + planning-readiness | McClintock | answered | needs revision |
| contract-and-scope + architecture-boundaries + spec-difference | Raman | answered | needs revision |
| security-threat-model + WebSocket/protocol | Hubble | answered | needs revision |
| adversarial-crux + guardrail-codification + UX/status | Hypatia | answered | needs revision |

## What Held

- The primary spec is still the right entry point: it is under 2000 lines and
  carries intent, requirements, boundary map, contract, UX, security, proof, and
  next workflow in one artifact.
- Batch route-band assessment, availability-pool isolation, fixed v1 policy,
  table/plain/JSON split, local-auth-before-selection, malformed first-frame
  fail-closed behavior, and redaction inventory are materially stronger than R2.
- The remaining blockers are semantic contract gaps, not a wholesale algorithm
  redesign.

## Accepted Findings

### R3-A1. `selected_next` overclaims live runtime truth

Severity: blocker

Evidence:

- Spec says proxy owns process-lifetime fairness state while CLI consumes pure
  assessment output.
- Spec also puts `selected_next` in `BurnDownRouteBandAssessment` and renders it
  as default human status truth.
- `WeightedDeficitSelector` has mutable deficit state and the runtime may
  intentionally select a lower-weight account for fairness.
- Previous-response affinity can also override normal weighted fallback.

Failure path:

- A planner either threads proxy runtime state into CLI/status and breaks the
  boundary, or computes a pure projection and labels it as live next use. Either
  can pass local tests while misleading the operator.

Required revision:

- Rename/scope the pure assessment field to `preferred_next`.
- Define it as neutral-state, no-affinity, route-band preferred candidate.
- Default human output must say `preferred next`, not `selected next`.
- Runtime exact account choice remains proxy-owned and may differ due to
  previous-response affinity or accumulated weighted-deficit fairness state.
- Proof must show status shares burn-down semantics and separately prove runtime
  routing honors affinity/fairness.

### R3-A2. Previous-response affinity owner and fail-closed policy are missing

Severity: blocker

Evidence:

- Spec says previous-response affinity runs before weighted fallback, but does
  not define owner resolution, persistence, HTTP/SSE scope, WebSocket scope, or
  missing-owner behavior.
- Greenfield source-of-truth says previous-response continuation must fail
  clearly instead of silently replaying on a different account.
- Current code has affinity primitives, but the reviewed spec does not make the
  v1 policy normative.

Failure path:

- A planner can silently fall back to weighted routing when a continuation owner
  is missing, disabled, unauthenticated, or ineligible, causing cross-account
  continuation replay.

Required revision:

- Add a first-class affinity contract.
- Proxy extracts previous-response metadata for HTTP/SSE and WebSocket.
- Durable owner data belongs to `AffinityRepository`; selection helpers may stay
  pure.
- Missing, disabled, unauthenticated, stale-generation, or route-ineligible
  continuation owners fail closed before weighted fallback.
- Weighted fallback is used only when no previous-response affinity key is
  present.
- Proof must cover restart/durable lookup, unknown owner, disabled owner,
  ineligible owner, and no weighted fallback on continuation failure.

### R3-A3. Current WebSocket call-order delta is not named as repo reality

Severity: important

Evidence:

- Desired WebSocket order is now specific, but current-state evidence does not
  call out that current `websocket.rs` selects/resolves before bounded
  first-frame parsing.

Failure path:

- Plan creation can underestimate the refactor and test only the happy path.

Required revision:

- Add current-vs-target WebSocket call-order bullets in Current-State Evidence.
- Make first-frame parse before selection/resolution/upstream open a named
  implementation delta and proof target.

### R3-A4. WebSocket unsupported route taxonomy is underspecified

Severity: important

Evidence:

- Spec names unsupported, realtime, and unknown WebSocket routes separately, but
  does not define whether these are distinct classes or one fail-closed class.

Failure path:

- Tests can cover one generic unsupported route while leaving `/v1/realtime` or
  unknown paths ambiguous.

Required revision:

- Collapse v1 to one `unsupported_path` class for every WebSocket path other
  than `/v1/responses`.
- Require local rejection before selection, credential resolution, or upstream
  open.
- Proof must cover `/v1/realtime` and one unknown path as representatives of
  `unsupported_path`.

### R3-A5. Safe account label schema is still ambiguous

Severity: important

Evidence:

- JSON schema exposes `account_label` while security text requires safe labels.
- Account labels in state are unconstrained strings.

Failure path:

- JSON/debug output can serialize an unsafe configured label, and that value can
  be copied into logs, traces, or smoke artifacts.

Required revision:

- Replace default machine field `account_label` with `safe_account_label`.
- Define unsafe label fallback as deterministic safe hash/tag.
- Raw configured labels are not emitted in default human, logs, traces, smoke
  transcripts, or JSON status for this goal.
- Redaction canary proof must include JSON.

### R3-A6. Unknown fallback ordering contradicts missing-reset ranking text

Severity: important

Evidence:

- Missing-reset text assigns a pressure bucket "for ranking purposes".
- Later unknown-pool text gives all unknown accounts fixed weight `1`.

Failure path:

- All-unknown startup can route equally to a 1% and 90% account, or planners can
  invent hidden partial-data ranking.

Required revision:

- Unknown never competes with known `usable` or `reserve`.
- Inside the `unknown` fallback pool, use conservative partial evidence for
  ordering when known headroom exists; missing-reset pressure receives no
  salvage.
- If no usable partial evidence exists, fall back to weight `1` and canonical
  `account_id` order.

### R3-A7. Human status table remains example-shaped

Severity: important

Evidence:

- `status` column shows `enabled`, while the useful routing state is spread
  across other fields.
- The fixed `5h`/`weekly` layout is not reconciled with generic multi-window
  input.
- Example contains phrases not backed by the stable phrase map.

Failure path:

- Implementation can preserve admin noise while missing the useful status
  invariant, or silently drop extra windows.

Required revision:

- Define `status` as account admin status only, and require routing usefulness
  to live in `5h`, `weekly`, `routing`, and `next use`.
- Freeze v1 human display to one displayed short slot and one displayed long
  slot per route band, selected by the same limiting-window rules. Extra
  relevant windows are summarized in the slot and remain available in JSON.
- Define legal `next use` values and align example strings to the enum mapping.

### R3-A8. Live quota-status smoke is not explicit enough

Severity: important

Evidence:

- Proof expectations include renderer/golden/schema tests and non-blocking
  render, but not a real CLI emitted-output smoke over persisted router state.

Failure path:

- A plan can prove helper renderers but never the shipped command output.

Required revision:

- Add live-safe CLI smoke proof using persisted router state for `table`,
  `plain`, and `json`.
- Include redaction and negative assertions on emitted output.

### R3-A9. Non-blocking proof must cover WebSocket path

Severity: important

Evidence:

- R1 applies to startup and request routing broadly.
- Proof currently separates generic first routed request and WebSocket e2e, but
  does not require delayed/failing refresh on first `/v1/responses` WebSocket.

Failure path:

- HTTP non-blocking can pass while first real Codex WebSocket blocks on refresh.

Required revision:

- Add black-box delayed/failing-refresh proof for first valid `/v1/responses`
  WebSocket after bounded first-frame parse.

## Rejected Or Deferred Findings

- No accepted finding requires a new algorithm family.
- No accepted finding requires exposing proxy fairness state to CLI; the parent
  reducer chooses neutral-state `preferred_next` for default status because it
  preserves the pure shared assessment boundary and avoids a misleading
  runtime-exact claim.
- No accepted finding requires opening WebSocket v1 beyond `/v1/responses`.

## Verdict

Needs revision. Do not route to `plan-creation-swarm`.

Next workflow: `shravan-dev-workflow:spec-creation-swarm`.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r3/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: R3 accepted blockers require one more spec revision before planning.
