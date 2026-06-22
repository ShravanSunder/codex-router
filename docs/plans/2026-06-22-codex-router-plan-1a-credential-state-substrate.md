# Plan 1A: Credential And State Substrate

Date: 2026-06-22
Parent: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
Status: executable stacked prerequisite child plan after review

## Goal

Create the credential and state substrate that lets quota runtime behavior be safe. After this plan, provider-bound auth reads for quota refresh, HTTP/SSE forwarding, and WebSocket upstream opens use one resolver that can refresh imported credentials or fail closed before upstream egress.

Plan 1A is a merge-gate slice, not final Plan 1 closeout. It may checkpoint and review independently, but it must not claim startup-refresh correctness, selection-policy correctness, account switching, status UX, smoke, docs, or live proof.

## Non-Goals

- [ ] Do not implement `account login`.
- [ ] Do not implement browser/device-code OAuth.
- [ ] Do not make file-backed plaintext secrets the normal steady-state onboarding story.
- [ ] Do not implement quota runtime scheduling/status UX beyond what is needed to keep state contracts coherent.
- [ ] Do not run live OAuth/quota proof without explicit approval.
- [ ] Do not claim Plan 1 final completion; Plan 1B owns final runtime/smoke closeout.

## Child Proof Contract

- [ ] Every task block contains actions and proof checkboxes.
- [ ] Every executable requirement appears in the proof matrix with a concrete command or exact test filter.
- [ ] No executable row uses placeholder proof text such as `or equivalent`, `named test`, or wrapper-only smoke references.
- [ ] Behavior-changing rows require red/green evidence.
- [ ] Deferred and gated-live items remain explicit rather than prose-only.
- [ ] Closeout reports command, exit code, pass/fail count where available, stale-proof guard result, and red/green result.
- [ ] Smoke, workspace-wide nextest, `cargo deny check`, `cargo audit`, and live proof remain deferred to Plan 1B unless the user turns Plan 1A into a separately mergeable PR.

## Write Surfaces

- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/account.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-auth/src/oauth.rs`
- `crates/codex-router-auth/src/refresh_worker.rs`
- `crates/codex-router-auth/src/router_credentials.rs`
- `crates/codex-router-auth/src/live_quota.rs`
- `crates/codex-router-auth/src/quota_client.rs`
- `crates/codex-router-secret-store/src/account_tokens.rs`
- `crates/codex-router-secret-store/src/file_backend.rs`
- `crates/codex-router-secret-store/src/lib.rs`
- `crates/codex-router-state/src/account.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-selection/src/*`

## Security Context

Assets:

- OAuth refresh tokens
- access tokens
- router bearer token
- account ids and redacted labels
- quota snapshots/status rows
- SQLite account/credential metadata
- audit/status output

Invariants:

- [ ] Runtime credentials come through router-owned secret-store boundaries, not direct `auth.json` reads.
- [ ] Token egress happens only after provider/base-url allowlisting.
- [ ] Secret-bearing DTOs do not expose raw values through debug, display, serialization, panic, logs, status, or worker stderr.
- [ ] Partial credential writes fail closed.
- [ ] Credential mutation invalidates or gates stale quota state until successful refresh.
- [ ] Expired or missing credentials fail locally before upstream auth egress when refresh is impossible.

## Execution Checklist

### Gate 0. Freeze Repo Reality

- [ ] Record `git status --short`.
- [ ] Identify pre-existing dirty implementation changes.
- [ ] Confirm Plan 1A is the only executable scope.
- [ ] Save repo-state evidence in the implementation handoff or PR body.

### T1. Runtime Boundary Extraction

Purpose:

- Keep CLI command glue from owning long-lived auth/quota behavior.

Actions:

- [ ] Extract quota refresh orchestration and normalization from CLI-owned command glue only where needed by T2-T5.
- [ ] Keep CLI command handlers thin: parse options, call runtime/service APIs, render results.
- [ ] Keep provider fetch, failure classification, and status row construction behind quota/auth service boundaries.

Proof:

- [ ] Existing account/quota/serve tests compile after extraction.
- [ ] No behavior change is intended in this step except clearer ownership.
- [ ] 1A-00 proves the extraction is behavior-preserving.

Checkpoint:

- [ ] `refactor: extract quota runtime boundaries`

### T2. Identity, Redaction, And Token-Egress Guards

Actions:

- [ ] Remove observable `Debug` or logging paths from secret-bearing DTOs.
- [ ] Use account IDs for runtime/auth contracts.
- [ ] Keep labels display-only.
- [ ] Preserve quota endpoint allowlisting before any token read or bearer-token egress.
- [ ] Ensure background-worker diagnostics are redacted.

Proof:

- [ ] Add/run canary-token tests over stdout, stderr, errors, status output, and worker diagnostics.
- [ ] Run disallowed quota URL tests and confirm no token egress.
- [ ] Add/keep serve-start preflight proof for disallowed quota base URLs.

Checkpoint:

- [ ] `fix: harden account identity and secret redaction`

### T3. Fail-Closed Credential Writes

Design decision:

- [ ] Use one bundled account credential secret or a versioned pending/active secret set with a SQLite pointer flip.
- [ ] Do not leave access-token and refresh-token generations independently observable.
- [ ] On credential mutation, invalidate quota selector/status state to explicit `unknown`/ineligible/stale state until successful refresh.

Actions:

- [ ] Add coherent write/update primitives for account metadata and token material.
- [ ] Keep accounts disabled or ineligible until required secret material and metadata are installed.
- [ ] Add recoverable repair/re-import behavior for partial failures.
- [ ] Preserve healthy accounts if one import or repair fails.

Proof:

- [ ] Inject failure after access-token write.
- [ ] Inject failure after refresh-token write.
- [ ] Inject failure after SQLite metadata/state write.
- [ ] Prove the account stays disabled/ineligible/unknown.
- [ ] Prove no mixed credential generation is visible to serve or quota refresh.
- [ ] Prove healthy pre-existing accounts remain selectable.

Checkpoint:

- [ ] `fix: make account credential writes fail closed`

### T4. Unified Credential Resolver

Invariant:

- [ ] Quota refresh, HTTP/SSE forwarding, and WebSocket upstream opens all obtain provider-bound auth through the same credential resolver.
- [ ] The resolver checks expiry metadata, uses per-account refresh leases, updates credential material through the secret-store boundary, and fails closed before upstream egress when refresh is impossible.

Actions:

- [ ] Use stored refresh-token/expiry metadata for imported accounts.
- [ ] Add single-flight or lease protection per account refresh.
- [ ] Classify auth states: fresh, refresh-needed, refreshable-expired, unrefreshable-expired, terminal missing credential.
- [ ] Replace direct request-path access-token reads with resolver calls or explicit ineligible handling.
- [ ] Keep interactive login out of this task.

Proof:

- [ ] Expired access token plus refresh token refreshes before quota refresh provider egress.
- [ ] Expired access token plus refresh token refreshes before HTTP/SSE upstream egress.
- [ ] Expired access token plus refresh token refreshes before WebSocket upstream egress.
- [ ] Expired access token without refresh token fails locally before upstream auth egress.
- [ ] Concurrent serve request plus quota refresh single-flight on the same account.
- [ ] Token canaries do not appear in logs/errors/status.

Checkpoint:

- [ ] `feat: resolve provider credentials for runtime use`

### T5. Durable Per-Window Selector Source

Decision:

- [ ] Prefer reusing existing `quota_status_rows` as the durable per-window source if it supports efficient account/route-band queries.
- [ ] If reuse is insufficient, add a schema version and migration for a new selector-input table before editing selector behavior.

Actions:

- [ ] Add repository methods needed by selector code for account + route-band window rows.
- [ ] Persist or expose enough input to distinguish short-window and weekly/long-window pressure.
- [ ] Preserve existing readable status rows and effective bottleneck behavior.

Proof:

- [ ] Repository-backed selector test proves weekly-vs-short-window scoring from the chosen durable source.
- [ ] If schema changes, upgrade a v2 fixture DB and prove `serve` and `quota status` still work.
- [ ] If schema does not change, prove old DBs still open and status rows remain readable.

Checkpoint:

- [ ] `feat: expose per-window quota state to selection`

## Plan 1A Proof Matrix

| Done | ID | Requirement | Source | Task | Layer | Fixture/mock | Command | Expected observation | Stale-proof guard | Red/green |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [ ] | 1A-00 | Boundary extraction is behavior-preserving | plan-creation lane: scope-and-proof-fit | T1 | compile/integration | existing account/quota/serve fixtures | `cargo nextest run -p codex-router-cli account_ quota_ serve_` | existing behavior remains unchanged after extraction | compare before/after failing-test intent; no new behavior claims | yes |
| [ ] | 1A-01 | Secret-bearing auth DTOs do not leak | spec Security/Rust standards | T2 | unit/integration | token canaries | `cargo nextest run -p codex-router-auth` plus CLI output tests | no canaries in debug/stdout/stderr/errors/status | unique canary per test | yes |
| [ ] | 1A-02 | Token egress only after allowlist | spec Security | T2 | integration | disallowed quota URL | `cargo nextest run -p codex-router-cli quota_` | local reject before secret read/send | assert secret/provider hit counts | yes |
| [ ] | 1A-03 | Partial credential writes fail closed | spec Secret Storage | T3 | integration | injected write failures | `cargo nextest run -p codex-router-cli account_` | partial account disabled/ineligible/unknown | inject every write boundary | yes |
| [ ] | 1A-04 | Credential mutation invalidates stale quota | spec Account/Quota | T3 | integration | credential repair fixture | `cargo nextest run -p codex-router-cli account_credential_mutation_invalidates_quota` | old positive quota not used after mutation | assert selector cannot use stale generation | yes |
| [ ] | 1A-05 | Resolver covers quota refresh egress | spec Secret Storage | T4 | integration | expired access + refresh token | `cargo nextest run -p codex-router-auth refresh_` plus quota refresh test | token refreshed before quota provider egress | no stale bearer leaves router | yes |
| [ ] | 1A-06 | Resolver covers HTTP/SSE egress | spec Secret Storage/Protocol | T4 | protocol | expired access + refresh token | `cargo nextest run -p codex-router-proxy credential_` | token refreshed before upstream HTTP/SSE egress | upstream mock rejects stale bearer | yes |
| [ ] | 1A-07 | Resolver covers WebSocket egress | spec Secret Storage/WebSocket | T4 | protocol | expired access + refresh token | `cargo nextest run -p codex-router-proxy websocket_` | token refreshed before upstream WS open | upstream mock rejects stale bearer | yes |
| [ ] | 1A-08 | Expired token without refresh fails closed | spec Secret Storage/Security | T4 | integration/protocol | expired access, no refresh | `cargo nextest run -p codex-router-auth expired_without_refresh_fails_closed` and `cargo nextest run -p codex-router-proxy expired_without_refresh_fails_closed` | local failure; zero upstream auth egress | bearer canary absent upstream | yes |
| [ ] | 1A-09 | Concurrent resolver paths single-flight | spec Secret Storage | T4 | integration | concurrent serve + quota refresh | `cargo nextest run -p codex-router-auth resolver_single_flight` and `cargo nextest run -p codex-router-proxy resolver_single_flight` | one owner; followers use result/fail closed | deterministic concurrent test | yes |
| [ ] | 1A-10 | Per-window selector source is durable | spec Account/Quota | T5 | integration | existing or migrated DB | `cargo nextest run -p codex-router-state quota_` | selector/status can read durable windows | old and new DB state tested | yes |

## Validation Gates

- [ ] `cargo fmt --all --check`
- [ ] `cargo nextest run -p codex-router-auth`
- [ ] `cargo nextest run -p codex-router-secret-store`
- [ ] `cargo nextest run -p codex-router-state`
- [ ] `cargo nextest run -p codex-router-selection`
- [ ] `cargo nextest run -p codex-router-proxy credential_`
- [ ] `cargo nextest run -p codex-router-proxy websocket_`
- [ ] `cargo nextest run -p codex-router-cli account_`
- [ ] `cargo nextest run -p codex-router-cli quota_`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `git diff --check`

## Review Gate

- [ ] Run `implementation-review-swarm` with auth/secret/state/proxy credential lanes.
- [ ] Do not start Plan 1B until Plan 1A review blockers are resolved or an explicit user-approved exception exists.

## Replan Triggers

- [ ] Credential resolver cannot cover quota refresh, HTTP/SSE, and WebSocket without a broader trait redesign.
- [ ] Secret-store cannot provide fail-closed update semantics without delete/journal support.
- [ ] Durable per-window selector source requires a larger migration than this child plan can safely carry.
