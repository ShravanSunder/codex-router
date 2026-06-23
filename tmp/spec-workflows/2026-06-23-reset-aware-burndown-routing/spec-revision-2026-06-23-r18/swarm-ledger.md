# R18 Reset-Aware Burn-Down Routing Spec Revision Ledger

Date: 2026-06-23
Phase: spec-creation-swarm
Input review: `spec-review-2026-06-23-r17/review-ledger.md`

## Applied Revisions

- Added a normative route/API inventory covering every current routed surface:
  `POST /v1/responses`, WebSocket `/v1/responses`, `GET /v1/models`,
  `POST /v1/memories/trace_summarize`, `POST /v1/responses/compact`, HTTP
  unsupported paths, and WebSocket unsupported paths.
- Split `unsupported_path` proxy-edge rejection from `unsupported_route_band`
  assessment/status policy misses.
- Required route-native black-box e2e proof for every supported routed API, not
  only `/v1/responses`.
- Closed the refresh-status read surface as a sorted
  `BTreeMap<AccountId, QuotaRefreshStatusView>` with explicit legacy missing
  status representation.
- Removed duplicate `assessment_selected_pool` from
  `RuntimeSelectedAccountDecision`.
- Made runtime fairness and hold state route-band partitioned.
- Added a runtime side-effects matrix for weighted fallback, cooldown reuse,
  previous-response affinity hit, WebSocket connection pin, and durable owner
  writes.
- Required cooldown reuse to advance weighted-deficit state and affinity hits
  not to advance weighted-deficit state.
- Scoped HTTP/SSE body auth-smuggling checks to supported JSON POST routes and
  required compatibility proof.
- Made installed-Codex local-auth receipt transport-specific for HTTP/SSE and
  WebSocket.
- Tightened WebSocket direct payload validation to non-empty string `model`,
  top-level array `input`, and literal `stream=true`.

## Verification

- Primary spec line count after revision: under the 2000-line artifact cap.
- `git diff --check` must pass before the next review dispatch.

## Next Workflow

Run `shravan-dev-workflow:spec-review-swarm` against R18. Do not proceed to
`plan-creation-swarm` until the parent reducer records verdict `ready`.
