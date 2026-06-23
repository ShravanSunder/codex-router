# R14 Security + Harness-Fit Lane

Date: 2026-06-23
Reviewer lane: Archimedes (`019ef5f4-8e16-7000-9cee-10a2548bc9fa`)
Spec: `../reset-aware-burndown-routing-spec.md`
Parent baseline: `f104ff9`

## Coverage Receipt

Reviewed the security-sensitive and harness-sensitive surfaces in the full
spec, then checked the current implementation and installed Codex smoke harness
where the claims were cheap to verify.

Inspected current paths:

- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`

## Verdict

needs_revision

The spec is trying to harden local-router authentication and WebSocket
selection, but some requirements contradict the currently proven Codex path and
smoke transcript shape. That must be reconciled before implementation planning.

## Accepted Findings

### BLOCKER: Local Auth and Generated Codex Profile Contract Contradict Current Code

The spec requires generated Codex profiles to use `env_http_headers` carrying
`X-Codex-Router-Token`, and forbids `Authorization: Bearer` plus `env_key`
fallbacks.

Current code and tests still use or accept the old carrier:

- `profile.rs` renders `env_key = "CODEX_ROUTER_TOKEN"`.
- `local_auth.rs` accepts `Authorization: Bearer`.
- `server.rs` accepts `authorization` during WebSocket preflight.
- CLI tests assert `env_key` is present and `env_http_headers` is absent.

Refinement needed:

- Decide whether the spec target is a hard cutover to `env_http_headers`, or
  whether current Codex compatibility requires a different secure carrier.
- Name the exact generated-profile contract that installed Codex supports.
- If hard cutover remains required, add a proof row that proves installed Codex
  works through that path for HTTP/SSE and WebSocket.
- If the current carrier remains necessary, update the spec's local-auth
  security model instead of requiring an impossible path.

### BLOCKER: WebSocket First-Frame Preselection Contract Conflicts With Current Runtime

The spec says preselection may read only minimal routing metadata before account
selection. Current WebSocket code reads direct payload fields such as `model`,
`input`, and `stream` in the direct-payload branch before selection.

Refinement needed:

- Decide whether direct-payload WebSocket frames are in scope for current Codex.
- If yes, define the smallest allowed preselection read set for that real frame
  shape.
- If no, explicitly remove the direct-payload compatibility branch and update
  installed Codex smoke proof expectations.

### IMPORTANT: Smoke Transcript Redaction Contract Does Not Fit Current Harness

The spec forbids raw or non-allowlisted prompt/model/body fields in smoke
artifacts. The current installed-Codex transcript writes fields such as
`first_frame_model` and `first_frame_has_input`.

Refinement needed:

- Define an allowlisted transcript schema.
- Replace raw payload-derived fields with boolean proof markers or hashes only
  where needed.
- Add an artifact lint/golden proof that fails if raw prompt/model/body fields
  are reintroduced.

### IMPORTANT: Affinity Secret Cutover Order Is Under-Specified

The spec is directionally correct that previous-response affinity needs hashed
keys and secret ordering, but current code still has raw affinity-key paths.

Refinement needed:

- Add a cutover order: schema, repository API, hashing owner, HTTP/SSE route,
  WebSocket route, tests, and migration/deletion of raw-key helpers.

## What Held

- Local router auth is a real trust boundary.
- WebSocket routing needs preselection constraints.
- Smoke artifacts must prove behavior without leaking request content.

## Recommended Route

Return to `shravan-dev-workflow:spec-creation-swarm` before plan creation.
