# R19 Reset-Aware Burn-Down Routing Spec Revision Ledger

Date: 2026-06-23
Phase: spec-creation-swarm
Input review: `spec-review-2026-06-23-r18/review-ledger.md`

## Applied Revisions

- Chose one normative result shape: a flat
  `BurnDownRouteBandAssessmentResult` envelope with `route_result` as
  discriminator. Constructor names are allowed only as helpers.
- Removed per-account `route_band` from `BurnDownAccountInput`; the top-level
  assessment route band is the only pure-assessment route-band input.
- Marked unsupported-route-band JSON as v1 internal/test-only serialization
  proof, not a new user-facing debug/status command.
- Made HTTP/SSE routing order apply to every supported HTTP route.
- Split raw HTTP method/path classifier misses into `unsupported_path` and
  classified policy misses into `unsupported_route_band`.
- Scoped affinity to routes marked previous-response capable. Non-capable
  routes forward top-level `previous_response_id` as ordinary upstream-owned
  payload after local auth and auth-smuggling checks.
- Added proof for wrong HTTP methods on supported paths.
- Added handshake/connect or non-101 local rejection proof for invalid
  WebSocket local auth and unsupported WebSocket paths.
- Kept the R18 account-selection direction intact: cooldown debits fairness,
  affinity does not, and route-band state is partitioned.

## Verification

- Primary spec line count after revision: under the 2000-line artifact cap.
- `git diff --check` and JSONL validation must pass before the next review
  dispatch.

## Next Workflow

Run `shravan-dev-workflow:spec-review-swarm` against R19. Do not proceed to
`plan-creation-swarm` until the parent reducer records verdict `ready`.
