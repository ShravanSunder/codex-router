# R10 Spec Review Ledger

Date: 2026-06-23
Status: needs revision; accepted findings folded into spec in this checkpoint

## Source

- Baseline commit: `71487af docs: resolve r9 quota spec findings`
- Review worktree: `/tmp/codex-router-r10-review.lHVDkc`
- Target spec:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- Coverage before R10 fixes: 1294 lines, read in chunks 1-260, 261-520,
  521-780, 781-1040, and 1041-1294.

## Lanes

| Lane | Agent | Verdict | Parent result |
| --- | --- | --- | --- |
| whole-spec-coverage | Aristotle | needs revision | accepted |
| requirements-validation | Locke | ready | accepted ready |
| contract-architecture | Hooke | needs revision | accepted |
| security-websocket | Volta | ready | accepted ready |
| planning-crux | James | ready | accepted ready |
| disclosure-harness-difference | Mill | needs revision | accepted |

## Accepted Findings

1. `router_affinity_hash_secret` must be in global security assets, forbidden
   emission surfaces, and proof expectations.
2. Public `routing_reason` precedence must be deterministic when preferred
   explanation predicates overlap.
3. Secret-unavailable behavior for response-creating routes must be explicit.
4. Affinity HMAC, hash-secret storage, owner repository methods, and schema
   cutover need concrete crate/API ownership.
5. V1 route-band policy lookup must be selection-owned and cover every currently
   classified route band.
6. `accounts[]` ordering and `weighted_candidates[]` ordering must be separate
   contracts.
7. Safe account label/hash sanitization needs one shared owner.
8. Installed-Codex e2e must pin the generated profile local-auth header
   contract to `env_http_headers`, not `env_key` or Authorization fallback.

## Revision Applied

The spec now defines:

- `codex-router-core::redaction` as shared `SafeAccountLabel`/hash owner
- `codex-router-core::affinity` as typed affinity/HMAC owner
- `codex-router-secret-store` as hash-secret load/create owner
- state repository APIs that store only hashed previous-response owner records
- selection-owned route-band policy registry
- separate `accounts[]` and `weighted_candidates[]` ordering
- deterministic routing-reason precedence
- hash-secret redaction and unavailable-secret failure behavior
- generated Codex profile local auth using `env_http_headers`

## Parent Verdict

R10 did not pass the hard gate. Accepted findings were folded into the spec.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-review-swarm
recommended_transition_reason: R10 accepted findings have been folded into the spec; run another adversarial spec review before any plan creation.
