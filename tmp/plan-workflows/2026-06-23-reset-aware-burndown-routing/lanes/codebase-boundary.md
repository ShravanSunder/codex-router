# Codebase Boundary Lane

Status: answered
Candidate evidence label: `candidate-codebase-boundary-r20-main-0bde7ae`
Security context: applicable

## Evidence Inspected

Source:

- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
  R1-R10, separability map, route inventory, status, security, affinity,
  auth, order, and proof sections.
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r20/review-ledger.md`
  ready verdict and remaining implementation gates.

Live code:

- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-selection/src/weighted_deficit.rs`
- `crates/codex-router-core/src/ids.rs`
- `crates/codex-router-core/src/local_auth.rs`
- `crates/codex-router-core/src/redaction.rs`
- `crates/codex-router-core/src/audit.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/routes.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-secret-store/src/*`
- `crates/codex-router-test-support/src/installed_codex.rs`

## Accepted Implementation Lanes

1. Core contract primitives
   - Write scope: `crates/codex-router-core/src/{routes,affinity,redaction,ids,lib}.rs`,
     `crates/codex-router-core/Cargo.toml`.
   - Owns: `RouteBand`, `SafeAccountLabel`, previous-response affinity
     hash/secret typed primitives, route/audit-safe identifiers.

2. Pure burn-down assessment
   - Write scope: `crates/codex-router-selection/src/burn_down.rs`,
     selection tests.
   - Owns: reset-aware math, route-band policy registry, flat
     `BurnDownRouteBandAssessmentResult`, unknown fallback, stable reason/window
     DTOs.

3. State refresh read model and affinity repository cutover
   - Write scope: `crates/codex-router-state/src/{repositories,sqlite,quota_snapshot,lib}.rs`,
     state tests.
   - Owns: `selector_inputs_for_route_band(route_band, now_unix_seconds)`,
     `quota_refresh_status`, stale overlay, refresh success/failure atomic
     operations, hash-only previous-response owner records, schema migration.

4. Secret-store affinity secret
   - Write scope: `crates/codex-router-secret-store/src/*`, secret-store tests,
     crate manifest if RNG dependency is needed.
   - Owns: `router_affinity_hash_secret.v1` load/create, loaded/new state,
     redacted error classes.

5. Proxy routing, auth, affinity, and protocol order
   - Write scope: `crates/codex-router-proxy/src/{account_selection,routes,http_sse,websocket,server,local_auth,upstream,lib}.rs`,
     proxy tests.
   - Owns: route classification to core `RouteBand`, local-auth carrier
     validation, auth-smuggling checks, runtime adapter, exact account choice,
     route-band holds, previous-response affinity, call-order proof.

6. CLI quota status and generated profile
   - Write scope: `crates/codex-router-cli/src/{quota,profile,lib}.rs`, CLI
     tests.
   - Owns: table/plain/json formatting only, default account-centric
     `responses` status, generated profile proof for
     `env_key = "CODEX_ROUTER_TOKEN"`.

7. Test-support and black-box/e2e harness
   - Write scope: `crates/codex-router-test-support/src/*`, smoke transcript
     helpers, e2e fixture tests, `tests/smoke/*`.
   - Owns: route-native black-box suite, installed-Codex HTTP/SSE and
     WebSocket fixture, redacted transcripts, exact proof fields.

## Accepted Conflicts With Current Code

- Selection still uses string route bands, per-account route band, and
  caller-provided policy.
- Selection has no first-class unknown selected pool and uses coarse public
  reasons.
- Core lacks `RouteBand`, `SafeAccountLabel`, and typed previous-response
  affinity hash/secret primitives.
- State schema v4 has no `quota_refresh_status`; selector reads cannot overlay
  stale from `now_unix_seconds`.
- State affinity persistence stores raw `affinity_key -> account_id`.
- Proxy affinity currently requires owner membership in `weighted_candidates`
  and advances weighted fairness on affinity hit.
- Proxy auth does not yet enforce mixed-carrier equality or top-level
  HTTP/WebSocket auth-smuggling gates.
- CLI status still owns older wording/schema such as `needs probe`, `next`,
  `backup`, and coarse reasons.
- Smoke transcript still emits forbidden stale first-frame summary fields.

## Receipt

Answered by codebase-boundary lane. Parent accepted the lane shape and will
write the final plan from it.
