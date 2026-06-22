# Plan 1A: Credential And State Substrate

Date: 2026-06-22
Parent: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
Status: executable stacked prerequisite child plan after review; revised after plan-review `needs_revision`
Revision status: folded accepted findings from `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-460b51e.md`

## Goal

Create the credential and state substrate that lets quota runtime behavior be safe. After this plan, provider-bound auth reads for quota refresh, HTTP/SSE forwarding, and WebSocket upstream opens use one resolver that can refresh imported credentials or fail closed before upstream egress.

Plan 1A is a merge-gate slice, not final Plan 1 closeout. It may checkpoint and review independently, but it must not claim startup-refresh correctness, selection-policy correctness, account switching, status UX, smoke, docs, or live proof.

Plan 1A also defines the substrate Plan 2 needs for OAuth/device-code/keyring
onboarding: runtime code must depend on backend-neutral credential boundaries,
not `FileSecretStore` as an architectural contract. Plan 1A does not implement
interactive login, Keychain storage, logout, or remove.

## Non-Goals

- [ ] Do not implement `account login`.
- [ ] Do not implement browser/device-code OAuth.
- [ ] Do not make file-backed plaintext secrets the normal steady-state onboarding story.
- [ ] Do not implement OS keyring/Keychain storage.
- [ ] Do not implement `account logout` or `account remove`.
- [ ] Do not implement quota runtime scheduling/status UX beyond what is needed to keep state contracts coherent.
- [ ] Do not run live OAuth/quota proof without explicit approval.
- [ ] Do not claim Plan 1 final completion; Plan 1B owns final runtime/smoke closeout.

## Child Proof Contract

- [ ] Every task block contains actions and proof checkboxes.
- [ ] Every executable requirement appears in the proof matrix with proof owner,
      exact preflight list command, exact execution command, expected
      observation, and stale-proof guard.
- [ ] No executable row uses vague substitute wording, broad prefix filters, or
      wrapper-only smoke references.
- [ ] Behavior-changing rows require red/green evidence.
- [ ] Deferred and gated-live items remain explicit rather than prose-only.
- [ ] Closeout reports command, exit code, pass/fail count where available, stale-proof guard result, and red/green result.
- [ ] Smoke, workspace-wide nextest, `cargo deny check`, `cargo audit`, and live proof remain deferred to Plan 1B unless the user turns Plan 1A into a separately mergeable PR.

## Write Surfaces

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-auth/Cargo.toml`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-secret-store/Cargo.toml`
- `crates/codex-router-state/Cargo.toml`
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
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-selection/src/*`

Closed unless task-local amendment is approved:

- Any workspace manifest not named above.
- Plan 2 keyring/OAuth dependencies and implementation files.

## Ownership Decisions

- [ ] `codex-router-auth` owns the provider credential resolver. Plan 1A may
      add the manifest dependencies needed for `codex-router-auth` to call the
      backend-neutral secret-store capability and account metadata surfaces.
- [ ] `codex-router-proxy` and `codex-router-cli` consume the resolver through
      `codex-router-auth`; they must not keep direct runtime token-key or
      `read_secret` access in quota refresh, HTTP/SSE, or WebSocket egress.
- [ ] `codex-router-secret-store` owns backend-neutral secret storage traits and
      file-backend implementation details. Plan 2 can add a keyring backend
      behind the same capability surface without changing runtime call sites.
- [ ] Account credential atomicity uses one versioned bundled credential secret
      per account generation plus a SQLite active-generation pointer flip. The
      bundle contains access token, optional refresh token, expiry metadata,
      provider/source metadata, and credential generation. Legacy
      `openai_access_token.*` and `openai_refresh_token.*` keys are import or
      migration inputs only; they are not runtime truth after T3/T4.
- [ ] `codex-router-state` owns durable account, quota, and selector projection
      persistence. The selector consumes a selector-owned durable projection
      table/API, not human status rows or a DTO over renderer-oriented rows.
- [ ] Manifest changes are part of the plan, not task-local surprises. The A2
      receipt must list dependency-direction changes and prove the chosen
      resolver path compiles through CLI and proxy consumers.

## Task-Local Write Ownership

Default execution is serial. Do not fan out T2/T3 unless the executor first
creates a task-local write-surface table proving the intersection is empty.

- T1 owns extraction boundaries in `crates/codex-router-cli/src/lib.rs`,
  `crates/codex-router-cli/src/quota.rs`, and adjacent auth/quota service files.
- T2 owns secret DTO/debug/redaction and token-egress guards in auth,
  secret-store, quota refresh, and proxy audit/status surfaces.
- T3 owns `crates/codex-router-cli/src/account.rs`,
  `crates/codex-router-secret-store/src/account_tokens.rs`,
  `crates/codex-router-secret-store/src/file_backend.rs`,
  `crates/codex-router-secret-store/src/lib.rs`,
  `crates/codex-router-state/src/account.rs`,
  `crates/codex-router-state/src/repositories.rs`,
  `crates/codex-router-state/src/quota_snapshot.rs`, and
  `crates/codex-router-state/src/sqlite.rs`.
- T4 owns resolver call-site migration across quota refresh, HTTP/SSE, and
  WebSocket egress paths, plus the manifest changes required for
  `codex-router-auth` to own the resolver.
- T5 owns selector-input repository/schema surfaces only after T4 removes direct
  runtime secret reads.

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
- [ ] Audit JSONL events use an allowlisted schema and never rely on denylist
      substring checks alone.
- [ ] Runtime request/refresh paths cannot bypass the credential resolver with
      direct `read_secret` or token-key calls.

## Execution Checklist

### Gate 0. Freeze Repo Reality

- [ ] Record `git status --short`.
- [ ] Prefer a fresh execution worktree from the reviewed plan commit or latest
      approved receipt commit.
- [ ] Before editing in a fresh worktree, copy or promote the exact lifecycle
      packets:
      `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/`
      and
      `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/`.
- [ ] Record a carry-forward receipt with source path, target path, source
      commit/head, checksum or byte count, and `git status --short` before and
      after.
- [ ] Freeze required source artifacts before code edits. If
      `docs/specs/2026-06-20-codex-router-greenfield-spec.md` or
      `docs/specs/references/2026-06-20-research-evidence.md` is dirty relative
      to the execution base, either commit/promote it first or record a source
      carry-forward receipt with path, source commit/head, checksum or byte
      count, working-tree line count, execution-base line count, and normative
      flag.
- [ ] If executing in this worktree, create a dirty-path manifest classifying
      every dirty path as `out-of-scope`, `in-scope-preexisting`, or
      `new-task-surface`.
- [ ] For every dirty path overlapping a Plan 1A write surface, save hunk
      fingerprints or a baseline patch before editing.
- [ ] Confirm Plan 1A is the only executable scope.
- [ ] Save repo-state evidence in the implementation handoff, checkpoint
      receipts, and PR body.

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
- [ ] Parse audit JSONL in tests and assert exact allowed key sets for allowed
      HTTP, rejected HTTP, and rejected WebSocket cases.

Proof:

- [ ] Add/run canary-token tests over stdout, stderr, errors, status output, and worker diagnostics.
- [ ] Run disallowed quota URL tests and confirm no token egress.
- [ ] Add/keep serve-start preflight proof for disallowed quota base URLs.
- [ ] Add/run audit JSONL allowlist proof; canary absence alone is not enough.
- [ ] Use best-effort audit append with surfaced redacted local diagnostics for
      allowed proxy traffic; local-auth failures still reject before account
      selection. Audit append failures must not be silently swallowed.

Checkpoint:

- [ ] `fix: harden account identity and secret redaction`

### T3. Fail-Closed Credential Writes

Design decision:

- [ ] Use one versioned bundled account credential secret per account generation
      plus a SQLite active-generation pointer flip. The bundle contains access
      token, optional refresh token, expiry metadata, provider/source metadata,
      and credential generation.
- [ ] `codex-router-secret-store` owns bundle read/write semantics.
- [ ] `codex-router-state` owns active-generation metadata and atomic pointer
      activation.
- [ ] `codex-router-auth` resolver reads only the active bundle through the
      backend-neutral capability contract.
- [ ] Legacy `openai_access_token.*` and `openai_refresh_token.*` keys are
      import/migration input only and are not runtime truth after T3/T4.
- [ ] Do not leave access-token and refresh-token generations independently observable.
- [ ] On credential mutation, invalidate quota selector/status state to explicit
      `unknown`/ineligible/stale state until successful refresh.
- [ ] Hard route-band requirement: invalidate all response-backed selector bands
      from the spec: `responses`, `models`, `memories_trace_summarize`, and
      `responses_compact`.
- [ ] Current implementation note: `code_review` is status/quota state only, not
      a routed selector band. Invalidate its status rows for consistency, but do
      not make it a selector input unless a later spec change promotes it.

Actions:

- [ ] Add coherent write/update primitives for account metadata and token material.
- [ ] Add or expose `stage_account_credential_bundle`,
      `activate_account_credential_generation`, and
      `read_active_account_credential_bundle` equivalent APIs.
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
- [ ] Prove no response-backed alias remains eligible after mutation failure or
      repair failure until successful post-repair refresh.
- [ ] Prove resolver reads only the active bundled credential generation.
- [ ] Prove `code_review` status invalidation separately from selector
      eligibility.

Checkpoint:

- [ ] `fix: make account credential writes fail closed`

### T4. Unified Credential Resolver

Invariant:

- [ ] Quota refresh, HTTP/SSE forwarding, and WebSocket upstream opens all obtain provider-bound auth through the same credential resolver.
- [ ] The resolver checks expiry metadata, uses per-account refresh leases, updates credential material through the secret-store boundary, and fails closed before upstream egress when refresh is impossible.
- [ ] Runtime request/refresh paths do not own direct `SecretStore`,
      `read_secret`, `upstream_access_token_key`, or
      `upstream_refresh_token_key` access outside the resolver module.
- [ ] Selector-owned code returns account and route decisions without raw
      provider token material. Provider auth material is emitted only by the auth
      resolver immediately before HTTP/SSE, WebSocket, or quota-provider egress.

Actions:

- [ ] Use stored refresh-token/expiry metadata for imported accounts.
- [ ] Add single-flight or lease protection per account refresh.
- [ ] Classify auth states: fresh, refresh-needed, refreshable-expired, unrefreshable-expired, terminal missing credential.
- [ ] Replace direct request-path access-token reads with resolver calls or explicit ineligible handling.
- [ ] Replace direct quota-refresh access-token reads with resolver calls or
      explicit ineligible handling.
- [ ] Use this runtime call order: local auth, route classification, selector
      account/route decision without provider token material, then auth resolver
      provider credential resolution immediately before upstream egress.
- [ ] Remove raw provider token material from selector-owned return types such as
      `SelectedUpstreamAccount`; provider tokens belong only to resolver output.
- [ ] Add a backend-neutral secret-store capability surface that Plan 2 can
      extend to keyring/default login and logout/remove purge, without making
      file storage the normal backend.
- [ ] Replace runtime constructor signatures that require `FileSecretStore`
      directly with trait/factory surfaces that can accept the file backend now
      and a keyring backend in Plan 2.
- [ ] Keep interactive login out of this task.

Proof:

- [ ] Expired access token plus refresh token refreshes before quota refresh provider egress.
- [ ] Expired access token plus refresh token refreshes before HTTP/SSE upstream egress.
- [ ] Expired access token plus refresh token refreshes before WebSocket upstream egress.
- [ ] Expired access token without refresh token fails locally before upstream auth egress.
- [ ] Concurrent serve request and quota refresh single-flight on the same account.
- [ ] Token canaries do not appear in logs/errors/status.
- [ ] Structural resolver-bypass proof shows zero direct runtime
      `read_secret`/token-key matches outside the resolver in quota refresh,
      HTTP/SSE, and WebSocket paths.
- [ ] Structural backend-neutrality proof shows runtime entrypoints no longer
      require `FileSecretStore` as an architectural type.
- [ ] Structural selector-boundary proof shows selector return types and
      replacement-state structs carry no raw provider token material.

Checkpoint:

- [ ] `feat: resolve provider credentials for runtime use`

### T5. Durable Per-Window Selector Source

Decision:

- [ ] Do not make human quota status rows the selector contract. Prefer a
      selector-owned projection/DTO backed by the same durable per-window data.
- [ ] If reuse is insufficient, add a schema version and migration for a new selector-input table before editing selector behavior.

Actions:

- [ ] Add repository methods needed by selector code for account + route-band window rows.
- [ ] Persist or expose enough input to distinguish short-window and
      weekly/long-window pressure, including window label/family, used percent,
      remaining headroom, reset time, limit window seconds, status, and
      effective/bottleneck marker.
- [ ] Add a selector-input projection table/API over durable per-window selector
      inputs. The selector-owned surface must be readable from migrated
      databases and must not require parsing human status output or depending on
      renderer-oriented `quota_status_rows`.
- [ ] Preserve existing readable status rows and effective bottleneck behavior.

Proof:

- [ ] Repository-backed selector-input test proves weekly-vs-short-window
      scoring data can be read from the chosen durable source without depending
      on human status rows.
- [ ] If schema changes, upgrade a v2 fixture DB and prove `serve` and `quota status` still work.
- [ ] If schema does not change, prove old DBs still open and status rows remain readable.

Checkpoint:

- [ ] `feat: expose per-window quota state to selection`

## Plan 1A Proof Matrix

Each row must run its preflight before its execution command. The stale-proof
guard fails if the preflight returns zero matches, more than one named match, or
does not list the exact expected test. Proof owner is task plus crate/module,
not a person.

| Done | ID | Requirement | Source | Task | Proof owner | Layer | Fixture/mock | Preflight list command | Execution command | Expected observation | Stale-proof guard | Red/green |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [ ] | 1A-00 | Boundary extraction preserves CLI profile-write guardrails | plan-review-after-460b51e T1 proof | T1 | T1 / `codex-router-cli` runtime boundary | integration | existing CLI profile fixture | `cargo test -p codex-router-cli tests::profile_write_command_requires_approval_flag -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::profile_write_command_requires_approval_flag --exact` | profile write approval guard still passes after extraction; no home mutation occurs | exact test listed once; extraction checkpoint also runs package compile gate | yes |
| [ ] | 1A-00a | Boundary extraction preserves account import behavior | plan-review-after-460b51e T1 proof | T1 | T1 / `codex-router-cli::account` runtime boundary | integration | codex auth.json fixture | `cargo test -p codex-router-cli tests::account_import_codex_auth_writes_router_owned_state_and_secrets -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::account_import_codex_auth_writes_router_owned_state_and_secrets --exact` | import writes router-owned account state and credential bundle without mutating home Codex auth | exact test listed once; fixture path is temp-owned | yes |
| [ ] | 1A-00b | Boundary extraction preserves quota status behavior | plan-review-after-460b51e T1 proof | T1 | T1 / `codex-router-cli::quota status` runtime boundary | integration | preseeded SQLite rows | `cargo test -p codex-router-cli tests::quota_status_reads_sqlite_rows_without_provider_io -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_status_reads_sqlite_rows_without_provider_io --exact` | quota status reads SQLite rows and performs zero provider I/O | exact test listed once; provider mock counter remains zero | yes |
| [ ] | 1A-00c | Boundary extraction preserves serve loopback behavior | plan-review-after-460b51e T1 proof | T1 | T1 / `codex-router-cli::serve` runtime boundary | integration | loopback router fixture | `cargo test -p codex-router-cli tests::serve_command_starts_runtime_and_forwards_one_loopback_request -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::serve_command_starts_runtime_and_forwards_one_loopback_request --exact` | serve starts runtime and forwards one authenticated loopback request through the extracted boundary | exact test listed once; runtime shutdown is bounded | yes |
| [ ] | 1A-01 | Secret-bearing auth DTOs do not leak | spec Security/Rust standards | T2 | T2 / `codex-router-auth::router_credentials` | unit | token canaries | `cargo test -p codex-router-auth tests::router_credentials_debug_redacts_secret_fields -- --exact --list` | `cargo nextest run -p codex-router-auth -- tests::router_credentials_debug_redacts_secret_fields --exact` | debug/error paths do not include access or refresh token canaries | new exact test listed once | yes |
| [ ] | 1A-02 | Import errors redact credentials | spec Security/Rust standards | T2 | T2 / `codex-router-cli::account` | integration | malformed auth.json with canaries | `cargo test -p codex-router-cli tests::account_import_codex_auth_redacts_refresh_token_in_error_paths -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::account_import_codex_auth_redacts_refresh_token_in_error_paths --exact` | stdout/stderr/error text omit refresh/access token canaries | new exact test listed once | yes |
| [ ] | 1A-03 | Token egress only after allowlist | spec Security | T2 | T2 / `codex-router-cli::quota` | integration | disallowed quota URL | `cargo test -p codex-router-cli tests::quota_refresh_rejects_non_provider_base_url_before_token_egress -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_refresh_rejects_non_provider_base_url_before_token_egress --exact` | local reject occurs before provider hit or secret egress | existing exact test listed once; assert secret/provider hit counts | yes |
| [ ] | 1A-04 | Audit JSONL uses allowlisted schema | spec Local Auth/Audit | T2 | T2 / `codex-router-proxy` audit sink | integration | temp audit sink plus canaries | `cargo test -p codex-router-proxy tests::assembled_loopback_router_runtime_redacts_http_and_websocket_audit_events -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::assembled_loopback_router_runtime_redacts_http_and_websocket_audit_events --exact` | allowed HTTP, rejected HTTP, and rejected WS audit JSONL contain only allowlisted keys and no secrets | new or renamed exact test listed once; parses JSONL key sets, not substring-only | yes |
| [ ] | 1A-04a | Audit append failure surfaces redacted diagnostic | plan-review-after-460b51e audit reliability | T2 | T2 / `codex-router-proxy` audit sink | integration | unwritable or injected-failure audit sink | `cargo test -p codex-router-proxy tests::audit_append_failure_surfaces_configured_policy_without_secret_leak -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::audit_append_failure_surfaces_configured_policy_without_secret_leak --exact` | best-effort allowed traffic records a structured in-process redacted local diagnostic warning instead of silent drop; local-auth failures still reject | new exact test listed once; helper returns or records append error and policy is named in test fixture | yes |
| [ ] | 1A-04b | Audit append failures are not silently discarded | plan-review-after-460b51e audit reliability | T2 | T2 / `codex-router-proxy` audit emission | structural | source search | `rg -n "audit_sink\\.append" crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs crates/codex-router-proxy/src/server.rs` | `bash -lc '! rg -n -e "let _result = audit_sink\\.append" -e "let _ = audit_sink\\.append" crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs crates/codex-router-proxy/src/server.rs'` | zero silent-drop append sites remain; allowlisted helper must return or record the structured redacted diagnostic warning | any ignored append result in proxy emission paths fails the row | yes |
| [ ] | 1A-05 | Partial credential writes fail closed | spec Secret Storage | T3 | T3 / `codex-router-cli::account` | integration | injected write failures | `cargo test -p codex-router-cli tests::account_import_codex_auth_partial_secret_write_disables_account_until_repair -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::account_import_codex_auth_partial_secret_write_disables_account_until_repair --exact` | partial account stays disabled/ineligible/unknown and healthy accounts remain selectable | new exact test listed once; inject every write boundary | yes |
| [ ] | 1A-06 | Credential mutation invalidates stale quota | spec Account/Quota | T3 | T3 / state + quota mutation contract | integration | credential repair fixture | `cargo test -p codex-router-cli tests::account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation --exact` | `responses`, `models`, `memories_trace_summarize`, `responses_compact` become unknown/ineligible/stale until successful refresh; `code_review` status invalidation is asserted separately | new exact test listed once; selector cannot use stale generation | yes |
| [ ] | 1A-06a | Credential mutation invalidates response-backed aliases atomically | plan-review-after-460b51e alias-family atomicity | T3 | T3 / state + credential mutation contract | integration | injected alias write failure | `cargo test -p codex-router-state tests::credential_mutation_invalidates_response_backed_alias_family_atomically -- --exact --list` | `cargo nextest run -p codex-router-state -- tests::credential_mutation_invalidates_response_backed_alias_family_atomically --exact` | injected failure cannot leave mixed-generation `responses`, `models`, `memories_trace_summarize`, or `responses_compact`; `code_review` remains status-only | new exact test listed once; reads all four routed aliases plus status-only row | yes |
| [ ] | 1A-06b | Resolver reads only active bundled credential generation | plan-review-after-460b51e credential atomicity | T3/T4 | T3 / secret-store + state credential bundle | integration | staged inactive bundle plus active pointer | `cargo test -p codex-router-auth tests::credential_resolver_reads_only_active_credential_bundle_generation -- --exact --list` | `cargo nextest run -p codex-router-auth -- tests::credential_resolver_reads_only_active_credential_bundle_generation --exact` | staged inactive credential material is never returned; active-generation pointer flip is the only runtime publication point | new exact test listed once; legacy split token keys are import-only inputs | yes |
| [ ] | 1A-07 | Resolver covers quota refresh egress | spec Secret Storage | T4 | T4 / credential resolver + quota refresh | integration | expired access + refresh token | `cargo test -p codex-router-cli tests::quota_refresh_resolver_refreshes_expired_access_token_before_provider_egress -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_refresh_resolver_refreshes_expired_access_token_before_provider_egress --exact` | refreshed token is used before quota provider egress | new exact test listed once; no stale bearer leaves router | yes |
| [ ] | 1A-08 | Resolver covers HTTP/SSE egress | spec Secret Storage/Protocol | T4 | T4 / `codex-router-proxy::http_sse` | protocol | expired access + refresh token | `cargo test -p codex-router-proxy tests::http_proxy_resolver_refreshes_expired_access_token_before_upstream_egress -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_resolver_refreshes_expired_access_token_before_upstream_egress --exact` | refreshed token is used before upstream HTTP/SSE egress | new exact test listed once; upstream mock rejects stale bearer | yes |
| [ ] | 1A-09 | Resolver covers WebSocket egress | spec Secret Storage/WebSocket | T4 | T4 / `codex-router-proxy::websocket` | protocol | expired access + refresh token | `cargo test -p codex-router-proxy tests::authenticated_websocket_router_refreshes_expired_access_token_before_upstream_open -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::authenticated_websocket_router_refreshes_expired_access_token_before_upstream_open --exact` | refreshed token is used before upstream WS open | new exact test listed once; upstream mock rejects stale bearer | yes |
| [ ] | 1A-10 | Expired quota token without refresh fails closed | spec Secret Storage/Security | T4 | T4 / credential resolver + quota refresh | integration | expired access, no refresh | `cargo test -p codex-router-cli tests::quota_refresh_missing_refresh_token_fails_closed_before_provider_egress -- --exact --list` | `cargo nextest run -p codex-router-cli -- tests::quota_refresh_missing_refresh_token_fails_closed_before_provider_egress --exact` | local failure; zero quota provider auth egress | new exact test listed once; bearer canary absent upstream | yes |
| [ ] | 1A-11 | Expired HTTP token without refresh fails closed | spec Secret Storage/Security | T4 | T4 / `codex-router-proxy::http_sse` | protocol | expired access, no refresh | `cargo test -p codex-router-proxy tests::http_proxy_missing_refresh_token_fails_closed_before_upstream_egress -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::http_proxy_missing_refresh_token_fails_closed_before_upstream_egress --exact` | local failure; zero HTTP upstream auth egress | new exact test listed once; bearer canary absent upstream | yes |
| [ ] | 1A-12 | Expired WebSocket token without refresh fails closed | spec Secret Storage/Security | T4 | T4 / `codex-router-proxy::websocket` | protocol | expired access, no refresh | `cargo test -p codex-router-proxy tests::authenticated_websocket_router_missing_refresh_token_fails_closed_before_upstream_open -- --exact --list` | `cargo nextest run -p codex-router-proxy -- tests::authenticated_websocket_router_missing_refresh_token_fails_closed_before_upstream_open --exact` | local failure; zero WS upstream auth egress | new exact test listed once; bearer canary absent upstream | yes |
| [ ] | 1A-13 | Concurrent resolver paths single-flight | spec Secret Storage | T4 | T4 / credential resolver | integration | concurrent serve + quota refresh | `cargo test -p codex-router-auth tests::credential_resolver_single_flights_concurrent_quota_refresh_and_serve_request -- --exact --list` | `cargo nextest run -p codex-router-auth -- tests::credential_resolver_single_flights_concurrent_quota_refresh_and_serve_request --exact` | one owner refreshes; followers use result or fail closed | new exact test listed once; deterministic concurrent test | yes |
| [ ] | 1A-14 | Runtime paths cannot bypass resolver | spec Secret Storage/Security | T4 | T4 / runtime egress call sites | structural | source search | `rg -n -e "read_secret" -e "upstream_access_token_key" -e "upstream_refresh_token_key" crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs` | `bash -lc '! rg -n -e "read_secret" -e "upstream_access_token_key" -e "upstream_refresh_token_key" crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs'` | zero matches in runtime egress paths; credential reads happen only through auth resolver APIs | any direct match in listed runtime files fails the row | yes |
| [ ] | 1A-14a | Runtime entrypoints are backend-neutral for Plan 2 | plan-review-after-460b51e OAuth readiness | T4 | T4 / auth resolver + runtime constructors | structural/compile | source search plus package compile | `bash -lc '! rg -n "FileSecretStore" crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/server.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs'` | `bash -lc '! rg -n "FileSecretStore" crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/server.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs' && cargo check -p codex-router-auth -p codex-router-proxy -p codex-router-cli` | runtime request/refresh entrypoints do not require concrete `FileSecretStore`; CLI/proxy compile through trait/factory-backed resolver path | any concrete runtime `FileSecretStore` match fails unless only file-backend construction edge is explicitly allowlisted in the receipt | yes |
| [ ] | 1A-14b | Resolver manifest direction is acyclic and explicit | plan-review-after-460b51e resolver ownership | T4 | T4 / workspace manifests | structural/compile | cargo metadata/tree | `cargo tree -p codex-router-auth -e normal` | `cargo check -p codex-router-auth -p codex-router-proxy -p codex-router-cli` | `codex-router-auth` depends on needed state/secret-store surfaces without creating an auth/proxy/CLI cycle; CLI/proxy consume auth resolver APIs | manifest diff is listed in A2 receipt; cargo check succeeds | yes |
| [ ] | 1A-14c | Selector decisions carry no raw provider token material | plan-review-after-460b51e selector/resolver separation | T4 | T4 / selection + proxy account decision structs | structural/compile | source search plus package compile | `rg -n -e "upstream_auth_token" -e "access_token" -e "refresh_token" crates/codex-router-selection/src crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs crates/codex-router-proxy/src/server.rs` | `bash -lc '! rg -n -e "upstream_auth_token" -e "provider_access_token" -e "provider_refresh_token" -e "refresh_token" crates/codex-router-selection/src crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs crates/codex-router-proxy/src/server.rs' && cargo check -p codex-router-selection -p codex-router-proxy` | selector-owned return types carry account/route decisions only; provider auth is produced by resolver output immediately before egress | any raw provider token field in selector-owned decisions fails the row | yes |
| [ ] | 1A-15 | Per-window selector source is durable and selector-owned | spec Account/Quota | T5 | T5 / `codex-router-state` + selector projection | integration | existing or migrated DB | `cargo test -p codex-router-state tests::selector_input_reads_durable_per_window_rows_without_status_renderer -- --exact --list` | `cargo nextest run -p codex-router-state -- tests::selector_input_reads_durable_per_window_rows_without_status_renderer --exact` | selector-input API returns short and weekly windows, status, headroom, reset, limit-window seconds, and effective marker without parsing human status rows | new exact test listed once; migrated DB fixture included if schema changes | yes |
| [ ] | 1A-16 | Route-band selector state remains partitioned | spec Account/Quota | T5 | T5 / `codex-router-state` | integration | route-band snapshots | `cargo test -p codex-router-state tests::quota_snapshots_are_partitioned_by_route_band_for_one_account -- --exact --list` | `cargo nextest run -p codex-router-state -- tests::quota_snapshots_are_partitioned_by_route_band_for_one_account --exact` | route-band snapshots remain separate for selector reads | exact test listed once; no alias collapse | yes |

## Validation Gates

- [ ] `cargo fmt --all --check`
- [ ] Exact proof-row preflights listed above.
- [ ] `cargo nextest run -p codex-router-auth`
- [ ] `cargo nextest run -p codex-router-cli`
- [ ] `cargo nextest run -p codex-router-proxy`
- [ ] `cargo nextest run -p codex-router-secret-store`
- [ ] `cargo nextest run -p codex-router-state`
- [ ] `cargo nextest run -p codex-router-selection`
- [ ] Matrix exact commands above, then relevant package gates.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `git diff --check`

## Merge Gate A1: Fail-Closed Credential Receipt

Required before T4 starts:

- [ ] T1-T3 matrix rows 1A-00 through 1A-06b pass or route back to planning.
- [ ] Dirty-tree isolation receipt proves only T1-T3 owned paths were staged.
- [ ] Checkpoint commit contains only T1-T3 paths.
- [ ] `git show --name-only <A1-checkpoint>` lists only T1-T3 owned paths.
- [ ] Any same-path pre-existing dirty hunks are accounted for by hunk
      fingerprint or moved to a precursor receipt.
- [ ] No baseline-only hunk is present in the staged diff or checkpoint commit.

## Merge Gate A2: Substrate-Complete Receipt

Required before Plan 1B starts:

- [ ] T4-T5 matrix rows 1A-07 through 1A-16 pass.
- [ ] Plan 1A validation gates pass with command, exit code, pass/fail count,
      stale-proof result, and red/green result.
- [ ] Dependency-direction receipt lists manifest changes and proves
      `codex-router-auth` owns the provider credential resolver.
- [ ] Resolver bypass search row 1A-14 passes.
- [ ] Backend-neutral runtime entrypoint row 1A-14a passes.
- [ ] Selector-owned durable input row 1A-15 passes.
- [ ] Dirty-tree isolation receipt proves only T4-T5 owned paths were staged.
- [ ] `git show --name-only <A2-checkpoint>` lists only T4-T5 owned paths.
- [ ] Same-path baseline hunks are accounted for and no baseline-only hunk is in
      the checkpoint commit.
- [ ] `implementation-review-swarm` completes with no unresolved blockers.
- [ ] Receipt states whether Plan 1A remains a stacked prerequisite or is a
      separately mergeable PR slice.

## Review Gate

- [ ] Run `implementation-review-swarm` with auth/secret/state/proxy credential lanes.
- [ ] Do not start Plan 1B until Plan 1A validation and implementation-review
      blockers are resolved and a Plan 1A completion receipt exists. A single
      PR stack does not waive this; it only means the Plan 1A receipt commit
      may exist earlier in the same stack.

## Replan Triggers

- [ ] Credential resolver cannot cover quota refresh, HTTP/SSE, and WebSocket without a broader trait redesign.
- [ ] Secret-store cannot provide fail-closed update semantics without delete/journal support.
- [ ] Durable per-window selector source requires a larger migration than this child plan can safely carry.
