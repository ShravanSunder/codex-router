# R16 Reset-Aware Burn-Down Routing Spec Revision Ledger

Date: 2026-06-23
Phase: spec-creation-swarm
Status: revised after R15 needs-revision findings

## Source Inputs

- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r15/review-ledger.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r15/lanes/*.md`
- `tmp/workflow-state/2026-06-23-quota-burndown-routing/details.md`

## Accepted Decisions Applied

1. Refresh staleness is a `codex-router-state` read-model overlay.
   Refresh workers compute `stale_after_unix_seconds` from the configured
   interval using `last_success + max(refresh_interval * 2, 600)`. State
   persists that timestamp, preserves selector rows exactly, and overlays
   `QuotaWindowStatus::Stale` on repository read without mutating rows.

2. Previous-response affinity is continuation correctness.
   Affinity may reuse a `usable` or `reserve` owner even when the reserve owner
   is outside the current selected pool. Unknown, blocked, excluded, exhausted,
   ineligible, missing-credential, or stale-generation owners fail closed before
   weighted fallback.

3. WebSocket auth-smuggling hard-fails at top-level field names only.
   The first-frame parser may inspect whether forbidden top-level auth-carrier
   field names are present and reject with `forbidden_local_auth_carrier`
   before selection. It must not inspect nested prompt/body values or emit raw
   field values.

4. Route-result output has one inventory across ok and unsupported.
   `route_result`, `route_band`, `selected_pool`, `selected_pool_reason`,
   `preferred_next_account_id`, `weighted_candidates`, and `accounts` are
   route-level fields for both branches.

5. Local-auth mismatch detection preserves both accepted carriers.
   HTTP/SSE, WebSocket preflight, and authenticated WebSocket tunnel validation
   must preserve bearer, `X-Codex-Router-Token`, and forbidden-carrier presence
   until validation decides accept/reject.

6. Installed-Codex bearer-auth proof uses safe observables.
   The e2e transcript may emit `local_auth_carrier=authorization_bearer` and
   `local_auth_validated=true`; it must not emit token values, hashes, lengths,
   prefixes, or raw headers.

7. Current-state WebSocket evidence and workflow gate wording were corrected.
   The spec now reflects existing path preflight/first-frame validation and
   gates plan creation on parent-verified spec-review verdict `ready`.

8. Workflow details required reading and e2e proof rows were updated to stop
   pointing planning at stale R12/header-profile guidance.

## Next Reviewer Attack Points

- Verify the refresh read overlay gives a planner enough API/migration/proof
  detail without requiring row mutation.
- Verify reserve-owner affinity semantics are consistent everywhere.
- Verify WebSocket auth-smuggling checks do not broaden into nested prompt/body
  scanning or leak raw values.
- Verify the route-result inventory is truly shared across ok, unsupported,
  JSON, status, proxy, and tests.
- Verify old transcript fields are explicitly non-compliant and replaced by
  allowlisted safe fields.
