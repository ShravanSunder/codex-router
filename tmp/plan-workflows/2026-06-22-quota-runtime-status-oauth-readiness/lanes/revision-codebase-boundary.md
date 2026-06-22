# Revision Lane: Codebase Boundary

Lane: `codebase-boundary`
Status: `answered`
Mode: read-only planning
Confidence: high

## Evidence Inspected

- Full spec: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- Umbrella plan, Plan 1A, Plan 1B
- Accepted review receipt: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-revision.md`
- Manifests for `codex-router-auth`, `codex-router-proxy`, `codex-router-cli`, `codex-router-quota`, `codex-router-state`, and `codex-router-secret-store`
- Direct secret reads in `crates/codex-router-cli/src/quota.rs` and `crates/codex-router-proxy/src/http_sse.rs`
- Runtime `FileSecretStore` coupling in `crates/codex-router-proxy/src/server.rs`
- Helper-only quota crate surface in `crates/codex-router-quota/src/*`

## Accepted Candidate Evidence

- `codex-router-auth` must own the provider credential resolver, with manifest
  changes explicitly in Plan 1A.
- Runtime quota refresh, HTTP/SSE, and WebSocket egress must consume resolver
  APIs instead of direct token-key or `read_secret` access.
- Plan 1A must prove backend-neutral runtime entrypoints rather than leaving
  concrete `FileSecretStore` in runtime constructors.
- `codex-router-state` must own a selector projection table/API, not a
  renderer-oriented status-row DTO.
- `codex-router-quota` must become the quota runtime owner in Plan 1B, including
  refresh orchestration, failure taxonomy, one-writer lease/fence behavior, and
  normalized selector/status publication.

## Parent Synthesis

Folded into:

- Plan 1A ownership decisions, write surfaces, T4, T5, rows `1A-14a`,
  `1A-14b`, and `1A-15`.
- Plan 1B ownership decisions, write surfaces, T6/T7 ownership, and
  `codex-router-quota` runtime boundary.

Completion receipt: answered; read-only; parent wrote this lane artifact.
