# R15 Reset-Aware Burn-Down Routing Spec Review Ledger

Date: 2026-06-23
Phase: spec-review-swarm
Reviewed commit: `aa1ed57`
Verdict: needs revision

## Coverage

Parent coverage:

- `reset-aware-burndown-routing-spec.md`: 1936 lines, read in chunks
  1-400, 401-800, 801-1200, 1201-1600, and 1601-1936.
- R15 revision ledger and lane files were inspected.
- R14 review ledger and goal details were inspected.

Lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-spec-coverage | Pauli | needs revision |
| requirements-testability + validation-and-testability | Descartes | needs revision |
| contract-and-scope + architecture-boundaries + planning-readiness | Ohm | needs revision |
| adversarial-crux + security-threat-model + harness-fit + spec-difference | Singer | needs revision |

## Parent Reducer Verdict

The spec is not ready for `plan-creation-swarm`. R15 fixed the generated
profile/auth direction, but the review found real contract contradictions and
missing read-path details that would force implementation planning to invent
behavior.

## Accepted Blockers

1. Refresh staleness has no canonical read path or formula.
   `quota_refresh_status` is introduced, but the spec does not say whether
   staleness is projected on repository read, materialized into selector rows,
   or carried as adapter metadata. It also does not define the
   `stale_after_unix_seconds` policy formula or the consumer of
   `last_error_class`.

2. Previous-response affinity contradicts cooldown/pinning.
   One section says affinity owners must remain in current
   `weighted_candidates`; another says `usable` or `reserve` owners are valid
   continuation targets. The spec must choose whether continuation correctness
   can reuse a reserve owner outside the current selected pool.

3. WebSocket body-token rejection contradicts the first-frame allowlist.
   The local-auth section forbids request-body token carriers, while the
   WebSocket section forbids parsing non-allowlisted first-frame fields before
   selection. The spec must choose whether token-like first-frame fields are
   hard-rejected through a narrow auth-smuggling scan or accepted-but-ignored.

4. Workflow/source-of-truth details still contain stale R12/R15 auth guidance.
   `details.md` still points at older review inputs and the rejected
   `env_http_headers` generated-profile contract in earlier sections. Planning
   could follow stale workflow state instead of the R15 spec.

## Accepted Important Findings

1. Route-result envelope is still split.
   Supported assessment, unsupported result, and JSON schema do not list the
   same fields. The next revision must define one exact route-level DTO/result
   inventory including `route_band` and the full `selected_pool_reason` domain.

2. Current-state WebSocket evidence is stale.
   The spec says current code parses/classifies too late, but current code
   already performs path preflight and bounded first-frame validation. The real
   implementation delta is first-frame allowlist/redaction, affinity-secret
   ordering, and avoiding full-frame selection/logging drift.

3. Mixed-carrier local auth needs an input-boundary contract.
   Current helpers collapse accepted carriers too early. The spec must require
   preflight/post-upgrade validation to preserve both accepted carriers and
   forbidden-carrier presence until mismatch checks run.

4. Installed-Codex transcript redaction needs explicit cutover language.
   Current transcript code writes `first_frame_model`, `first_frame_has_input`,
   and `first_frame_stream`; the spec must mark those as stale/non-compliant
   fields to delete.

5. Generated-profile e2e proof needs a named bearer-auth observable.
   The spec says the transcript must prove bearer auth reaches the router but
   does not name the audit-safe signal. The next revision should either require
   a router-side `local_auth_carrier=authorization_bearer` style boolean/enum,
   or state that separate ingress tests own carrier acceptance while e2e proves
   success and upstream stripping.

6. The tail workflow gate uses the wrong vocabulary.
   The spec says plan creation waits for `phase_result: complete`, while the
   `spec-review-swarm` skill returns `verdict: ready | needs revision | blocked
   | decision-needed`. The spec should gate on parent-verified review verdict
   `ready`.

## Next Revision Inputs

- Define refresh staleness as a repository read-model overlay or choose another
  single owner; name formula, APIs, and proof rows.
- Decide and restate reserve-owner previous-response affinity semantics.
- Decide and restate WebSocket token-like first-frame field handling.
- Normalize route-result fields across ok, unsupported, JSON, status, proxy,
  and tests.
- Update current-state evidence to match live WebSocket ordering.
- Add local-auth carrier-preservation contract for mismatch detection.
- Purge stale workflow-state auth guidance and required-reading references.
- Clarify installed-Codex e2e bearer-auth proof observable.
- Replace tail workflow wording with the actual spec-review verdict contract.

## Next Workflow

Return to `shravan-dev-workflow:spec-creation-swarm`. Do not start
`plan-creation-swarm` until these findings are revised and another
`spec-review-swarm` returns a parent-verified `ready` verdict.
