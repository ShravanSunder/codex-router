# R20 Reset-Aware Burn-Down Routing Spec Review Ledger

Date: 2026-06-23
Phase: spec-review-swarm
Reviewed commit: `7146aaa`
Verdict: ready

## Coverage

Parent coverage:

- `reset-aware-burndown-routing-spec.md`: 1971 lines, read in chunks
  1-500, 501-1000, 1001-1500, and 1501-1971.
- R19 review ledger, R20 revision ledger, route classifier, HTTP/SSE service,
  WebSocket server/tunnel, account selection, burn-down, weighted selector, and
  installed-Codex harness anchors were inspected during the focused closure
  review.

Lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| blocker-closure-selector-order | Epicurus | ready |

## Parent Reducer Verdict

R20 is ready for `plan-creation-swarm`. The focused review found that the two
R19 blockers are closed: stale public-surface selector references were removed,
and HTTP/SSE plus WebSocket routing now build the shared route-band assessment
before optional route-scoped affinity enforcement.

## Accepted Blockers

None.

## What Held

- The public selector contract uses the flat
  `BurnDownRouteBandAssessmentResult` envelope.
- HTTP/SSE routing order now performs local auth, route classification,
  assessment, optional route-scoped affinity, credential resolution, auth
  injection, header stripping, and upstream open in that order.
- WebSocket routing order now validates the direct frame, runs reset-aware
  `responses` assessment, then applies optional route-scoped affinity before
  credential resolution and upstream connection.
- The proof wording preserves the selected-pool-before-affinity rule and the
  zero-selector-advancement rule for affinity reuse.
- Earlier R19 closures still hold: route inventory, wrong-method proof,
  unsupported-route-band internal scope, and non-101 WebSocket negative proof
  remain explicit.

## Remaining Implementation Proof Gates

These are implementation gates, not spec-review blockers:

- pure assessment unit tests and flat envelope contract tests
- affinity, secret, HTTP/SSE call-order, and WebSocket preselection integration
  proof
- route-native black-box coverage for routed APIs and unsupported paths
- non-blocking startup/refresh proof
- installed-Codex HTTP and WebSocket e2e proof

## Next Workflow

Proceed to `shravan-dev-workflow:plan-creation-swarm`. The plan must keep the
proof gates explicit and must not treat the running system as complete before
unit, integration, smoke, and installed-Codex e2e evidence pass.
