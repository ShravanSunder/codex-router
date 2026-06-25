# Reset-Aware Burn-Down Routing Implementation Plan

Date: 2026-06-23
Workflow: `shravan-dev-workflow:plan-creation-swarm`
Source spec:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Spec review:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r20/review-ledger.md`

## Status

The R20 spec is ready for implementation planning. The running system is not
yet fully proven against the R20 requirements.

Current proof completed before this plan:

- `cargo check --workspace` passed from repo root on 2026-06-23.
- R20 spec review returned `ready`.
- R20 review artifacts and workflow transition were committed and pushed at
  `0bde7ae`.

Current proof not yet complete:

- route-native black-box proof for all routed APIs
- installed-Codex HTTP/SSE e2e proof
- installed-Codex WebSocket e2e proof
- redaction proof over final status/log/transcript artifacts
- non-blocking refresh proof over served router startup, request, and status

Plan review status:

- First plan-review pass returned `needs revision`.
- This revision folds accepted blocker/important findings for affinity owner
  writes, affinity-secret fail-closed ordering, WebSocket local-auth coverage,
  route-native harness split, installed-Codex transport-specific proof, final
  redaction scope, and unsafe parallelism.

No implementation executor may claim this goal complete until all required
proof gates below pass or are explicitly blocked with evidence.

## Goal

Implement reset-aware quota burn-down routing and quota status so runtime
account selection and user-facing quota output share the same assessment
semantics, protect weekly quota, salvage soon-reset quota safely, route every
current Codex API path correctly, preserve WebSocket compatibility, and prove
the installed Codex HTTP and WebSocket paths end to end.

## Non-Goals

- No live OAuth/keychain account-management work in this plan.
- No forecasting engine, EWMA, token-cost model, or global future-session
  optimizer.
- No mid-stream account switching.
- No compatibility shim for raw previous-response affinity rows.
- No live provider quota polling on startup or the request path.

## Source Coverage

- Spec: 1971 lines, loaded in chunks 1-500, 501-1000, 1001-1500,
  1501-1971.
- R20 review ledger: 68 lines, loaded.
- Workflow details and transition log loaded after R20.
- Parent live repo inspection covered selection, state, proxy HTTP/SSE,
  WebSocket, local auth, CLI status, profile, secret-store, test-support, and
  smoke script surfaces.

## Current Repo Evidence

- Current repo builds: `cargo check --workspace` passed.
- `crates/codex-router-selection/src/burn_down.rs` still exposes the older
  `BurnDownRouteBandAssessment` shape, caller policy, string route bands, and
  per-account route band input.
- `crates/codex-router-state/src/repositories.rs` still exposes
  `selector_inputs_for_route_band(route_band)` without `now_unix_seconds`.
- `crates/codex-router-state/src/sqlite.rs` has no `quota_refresh_status`
  schema and still stores raw affinity pins.
- `crates/codex-router-proxy/src/account_selection.rs` computes assessment
  before affinity, but current affinity requires owner membership in
  `weighted_candidates` and records weighted fairness.
- `crates/codex-router-proxy/src/local_auth.rs` prefers one accepted auth
  carrier instead of validating mixed carrier equality.
- `crates/codex-router-proxy/src/websocket.rs` has bounded first-frame shape
  validation but not the full allowlist, auth-smuggling, affinity-secret, and
  transcript proof contract.
- `crates/codex-router-cli/src/quota.rs` uses comfy-table and Unicode bars, but
  still owns some older status wording and JSON shape instead of rendering the
  full selection-owned contract.
- Focused 2026-06-24 investigation found the burn-down selector already
  computes reset-aware 5h/weekly pressure and routing pools, but the human
  table/plain quota status path hid that pace/burn-down signal; JSON exposed
  pressure/surplus fields while the default table did not.
- `crates/codex-router-test-support/src/installed_codex.rs` still emits stale
  forbidden first-frame transcript fields.

## Requirements / Proof Matrix

| id | requirement / claim | source | owning task | proof layer | evidence |
| --- | --- | --- | --- | --- | --- |
| RP-01 | Startup and routing never block on live quota refresh | R1 | T2a, T7 | integration + black-box | delayed/failing refresh tests over bind/listen, first request, and status |
| RP-02 | Exhausted/ineligible relevant windows block account | R2 | T1 | unit | burn-down collapse tests |
| RP-03 | Unknown quota is fallback-only, not free capacity | R3 | T1, T3, T4 | unit + integration + status | unknown pool and status tests |
| RP-04 | Weekly pressure dominates 5h urgency unless weekly reset is near | R4, R5 | T1 | unit | scenario tests for weekly far/near reset |
| RP-05 | Runtime selection, runtime audit, status table, status plain, and status JSON share the same flat assessment/decision envelope | R6, R7 | T1, T3, T4 | integration + smoke | shared DTO tests feeding runtime audit and every status renderer |
| RP-06 | Generated profile auth uses `CODEX_ROUTER_TOKEN`; HTTP/SSE and WebSocket accept bearer/manual header, reject mismatched mixed carriers, and reject forbidden token smuggling | R8 | T5, T6, T9, T10 | integration + e2e | cross-transport local-auth matrix and installed-Codex transcript receipts |
| RP-07 | WebSocket preselects from allowlisted first-frame view | R9 | T6, T10 | integration + e2e | first-frame matrix and installed-Codex WebSocket proof |
| RP-08 | Route result and unknown fallback are first-class flat contracts | R10 | T1, T3, T4 | unit + integration | flat envelope JSON/status/runtime tests |
| RP-09 | Previous-response affinity is HMAC hashed, durable, fail-closed, owner rows are written from allowlisted upstream response IDs, and raw keys are never persisted | affinity contract | T0, T2b, T2c, T3, T6 | unit + integration + security | core/state/secret/proxy affinity tests, HTTP/SSE pin-write proof, WebSocket pin-write proof |
| RP-10 | HTTP/SSE order is local auth, route, assessment, affinity-secret gate when previous-response-capable, optional affinity, credential, auth injection, strip, upstream | HTTP/SSE contract | T3, T5 | integration/security | call-order counters and negative side-effect tests including `affinity_secret_unavailable` |
| RP-11 | WebSocket invalid auth, unsupported paths, and forbidden subprotocol token smuggling fail before local upgrade; invalid first frames fail after local upgrade but before selector, credential, auth injection, or upstream open | WebSocket contract | T6, T8b | black-box/security | non-101 pre-upgrade proof plus post-upgrade zero-side-effect first-frame matrix |
| RP-12 | Status table/plain/json use safe labels, one logical row per account, bars, reasons, next-use, no account ids, no raw scores, no `pp`, no `bottleneck`, and no unrelated route rows by default | status contract | T4 | unit + smoke | table/plain/json snapshots and negative searches |
| RP-13 | Every routed API has route-native success and fail-closed proof through a stable harness command | route inventory | T8a, T8b | route-native black-box | `cargo test -p codex-router-test-support route_native_ -- --ignored --nocapture` |
| RP-14 | Installed Codex HTTP/SSE and WebSocket both work through router with transport-specific proof commands | proof expectations | T9, T10 | installed-Codex e2e | `installed_codex_http_sse_` and `installed_codex_websocket_` ignored test receipts |
| RP-15 | Logs/traces/audit/status/smoke/review artifacts redact tokens, headers, raw body/frame, prompts, unsafe labels, affinity secrets, affinity secret-store identifiers, derived secret material, raw previous-response IDs, and shared JSON `account_id` leakage | security context | T11 | security + smoke | canary negative searches over captured artifacts and review/receipt paths |
| RP-16 | Human quota table and plain output expose visible reset-aware burn-down/pace for 5h and weekly from the shared assessment/window math, not only JSON debug fields; missing or unknown quota renders `needs refresh` instead of fake pace | status UX contract | T4 | unit + smoke | table/plain snapshots include a `pace` column with `on pace`, `% behind`, `% ahead`, or `needs refresh`, and negative searches still reject `pp`/`bottleneck` |

Freshness guard for every proof row: run from a clean worktree after the
corresponding task changes; record command, exit code, and relevant artifact
paths in the implementation receipt.

Red/green rule: behavior-changing tasks add or update the narrow failing proof
first, observe expected failure, then make it pass. If a failing proof cannot be
constructed inside the task boundary, split the task before implementation.

## Task Sequence

### T0. Core Contract Primitives

Write scope:

- `crates/codex-router-core/src/routes.rs`
- `crates/codex-router-core/src/affinity.rs`
- `crates/codex-router-core/src/redaction.rs`
- `crates/codex-router-core/src/ids.rs`
- `crates/codex-router-core/src/lib.rs`
- `crates/codex-router-core/Cargo.toml`

Implement:

- Core `RouteBand` enum for `responses`, `responses_compact`, `models`, and
  `memories_trace_summarize`.
- Shared safe account label/tag helper with the exact unsafe-label predicate and
  `acct-<12 lowercase hex chars>` output.
- Typed previous-response affinity primitives:
  `PreviousResponseId`, `AffinityKeyHash`, `RouterAffinityHashSecret`, HMAC
  helper, full 64-lowercase-hex digest.

Proof:

- Unit tests for `RouteBand` serialization/display/coverage.
- Unit tests for safe label allow/deny rules and deterministic tag.
- Unit tests for HMAC length, lowercase hex, no raw previous-response id in
  debug/display.

Checkpoint:

- Commit after tests pass for T0.

### T1. Pure Burn-Down Assessment Contract

Write scope:

- `crates/codex-router-selection/src/burn_down.rs`
- selection crate tests
- exports in `crates/codex-router-selection/src/lib.rs`

Implement:

- Replace public `BurnDownRouteBandAssessment` surface with flat
  `BurnDownRouteBandAssessmentResult`.
- Remove caller-supplied policy and per-account route band.
- Add selection-owned policy registry keyed by core `RouteBand`.
- Add `route_result`, `selected_pool_reason`, `preferred_next_account_id`,
  stable account fields, `window_slots`, all relevant `windows[]`, unknown
  fallback pool, and public reason precedence.
- Keep `WeightedDeficitSelector` quota-semantics-free.

Proof:

- Unit tests for every scenario in the spec.
- Unit tests for unknown fallback-only behavior.
- Unit tests for route policy registry and unsupported-route-band envelope.
- Unit tests for ordering of `accounts[]` and `weighted_candidates[]`.
- Unit tests for no caller policy and no per-account route-band input.

Checkpoint:

- Commit after T1 unit proof passes.

### T2a. State Refresh Read Model

Write scope:

- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/lib.rs`

Implement:

- `quota_refresh_status` schema and DTOs.
- `selector_inputs_for_route_band(route_band, now_unix_seconds)` with stale
  read overlay.
- `quota_refresh_statuses_for_route_band(route_band)` sorted by account id.
- Atomic refresh success/failure repository operations.

Proof:

- SQLite integration tests for migration, stale overlay, legacy missing status,
  success/failure atomic operations, sorted status read, and row preservation.

Checkpoint:

- Commit after T2a state refresh read-model proof passes.

### T2b. Affinity Owner Storage Cutover

Write scope:

- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/lib.rs`

Implement:

- Hard cutover from raw affinity pins to hash-only previous-response owner
  records with `credential_generation`, `route_band`, `source_transport`, and
  `created_unix_seconds`.
- Repository methods to write owner records only from validated
  `AffinityKeyHash` plus safe metadata.
- Repository methods to resolve missing/found/ambiguous owner rows without raw
  previous-response ids.
- Purge or ignore semantics for stale owner rows after a secret replacement or
  removal; never silently regenerate a new secret and trust old owner rows.

Proof:

- Affinity repository tests for hash-only methods, no raw keys, missing/found/
  ambiguous owner lookup, route-band isolation, source-transport metadata, and
  purge/ignore behavior after secret loss or replacement.

Checkpoint:

- Commit after T2b affinity owner storage proof passes.

### T2c. Affinity Secret Store Contract

Write scope:

- `crates/codex-router-secret-store/src/*`

Implement:

- Secret-store API for `router_affinity_hash_secret.v1`, 32-byte entropy,
  64-lowercase-hex storage, loaded/newly-created state, and redacted error
  classes.
- Explicit recovery behavior for missing, unreadable, or replaced
  `router_affinity_hash_secret.v1`: continuations fail closed, old owner rows
  are ignored or purged, and logs/status expose only a redacted repair signal.

Proof:

- Secret-store tests for stable key, lifecycle, encoding, entropy length,
  error redaction, no debug/display leakage.
- Stateful recovery test that seeds owner rows, swaps/removes the secret, then
  proves continuation requests fail closed while stale rows are ignored or
  purged.

Checkpoint:

- Commit after T2c secret-store proof passes.

### T3. Proxy Runtime Selection Adapter And Route Inventory

Write scope:

- `crates/codex-router-proxy/src/routes.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/server.rs`
- related proxy tests

Implement:

- Route classification emits core `RouteBand` and previous-response-capable
  metadata.
- Adapter builds `BurnDownRouteBandAssessmentInput` from state rows and admin
  credential-generation facts.
- Runtime decision wraps the shared assessment in
  `RuntimeSelectedAccountDecision`.
- Cooldown/hold reuse only if held account remains in current
  `weighted_candidates`.
- For HTTP/SSE response-capable routes, load or create
  `router_affinity_hash_secret.v1` before selector advancement, credential
  resolution, auth injection, or upstream open.
- For HTTP/SSE response-capable routes, fail `affinity_secret_unavailable`
  before selector advancement, credential resolution, auth injection, or
  upstream open when the secret cannot be loaded or created.
- Previous-response affinity uses hash owner records and can continue on a
  `usable` or `reserve` owner outside the selected pool without advancing
  weighted fairness.
- On successful HTTP/SSE upstream response creation, write previous-response
  owner records only from the allowlisted top-level upstream response `id`
  field and the already-selected account metadata.
- Non-capable routes pass top-level `previous_response_id` through as upstream
  payload after local auth/auth-smuggling.
- Unsupported path versus unsupported route-band reasons remain distinct.

Proof:

- Proxy integration tests for every route inventory row.
- Call-order tests for route, assessment, affinity/weighted, credential, auth
  injection, strip, upstream.
- HTTP/SSE `affinity_secret_unavailable` call-order test proving zero selector
  advancement, zero credential resolution, zero auth injection, and zero
  upstream open.
- HTTP/SSE owner-record write test proving allowlisted `id` extraction,
  persisted safe metadata, no raw previous-response id leakage, and no parsing
  from other response/body fields.
- Tests proving route-band partitioning of weighted state and holds.
- Tests proving affinity hit does not advance weighted state and failure never
  falls back to another account.
- Shared-envelope test feeding the same `RuntimeSelectedAccountDecision` /
  `BurnDownRouteBandAssessmentResult` into runtime audit and status JSON/human
  renderers.

Checkpoint:

- Commit after proxy selection and route inventory integration proof passes.

### T4. CLI Quota Status And Refresh Worker Integration

Write scope:

- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-cli/src/lib.rs`
- CLI tests and smoke fixture

Implement:

- Status renderer consumes `BurnDownRouteBandAssessmentResult` and does not
  recompute pressure, limiting-window, next-use, reason, or fallback semantics.
- Default table uses comfy-table with account-centric rows, Unicode bars, safe
  labels, no raw account ids, no raw scores, no `pp`, no `bottleneck`, one
  logical row per account, and at most one blank continuation line inside a
  cell.
- Default table and plain output expose a human `pace` signal derived from the
  same reset-aware window pressure/surplus math as JSON. The signal must name
  5h and weekly status as `on pace`, `<n>% behind`, `<n>% ahead`, or
  `needs refresh`; it must not use raw scores, `pp`, or `bottleneck`.
- Missing expected windows, unknown quota, or missing reset metadata render
  `needs refresh` in the human pace signal instead of inventing a burn-down
  value.
- Status persists provider-reported reset credits from quota refresh and renders
  them as a `resets available` column plus JSON `reset_credits_available`; this
  is display-only and does not affect route scoring in v1.
- Plain output uses ASCII bars and the same phrases.
- JSON output uses the normative flat route-level and account-level schema.
- Status joins refresh status metadata for stale/needs-refresh notes without
  recomputing stale semantics.
- Default status renders only the `responses` route band unless the user asks
  for a route or all routes.
- Generated profile proof remains `env_key = "CODEX_ROUTER_TOKEN"` and
  `supports_websockets = true`.

Proof:

- CLI unit/golden tests for table/plain/json.
- Negative text assertions for forbidden words and secret-like material.
- JSON schema tests for stable enums and `window_slots`.
- Table/plain snapshot tests proving the `pace` column renders 5h and weekly
  burn-down pressure/surplus for known quota, and `needs refresh` for missing
  or unknown quota evidence.
- Snapshot matrix for healthy multi-account, 5h/weekly disagreement,
  reset-aware preferred-next, unknown/partial data, blocked/reserve/usable/
  unknown accounts, plain mode, JSON mode, and default no-unrelated-route rows.
- Tests proving status uses the shared safe-label helper and the shared
  assessment/decision envelope rather than recomputing limiting-window or
  next-use.
- Smoke fixture over persisted state for table, plain, and json.

Checkpoint:

- Commit after CLI unit and smoke proof passes.

### T5. Shared Local Auth And HTTP/SSE Security Contract

Write scope:

- `crates/codex-router-core/src/local_auth.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/headers.rs`
- proxy tests

Implement:

- Structured accepted-carrier extraction for `Authorization: Bearer` and
  `X-Codex-Router-Token` shared by HTTP/SSE and WebSocket ingress.
- Mixed-carrier equality validation for every transport: equal carriers pass,
  mismatched carriers fail before selection.
- Rejection for query, cookie, and top-level HTTP JSON body forbidden fields.
- Narrow body check only for supported JSON POST routes.
- Strip local auth carriers and hop-by-hop headers before upstream open.
- Export the WebSocket-safe local-auth validation primitive consumed by T6;
  T6 may not reimplement or weaken this contract.

Proof:

- Shared local-auth primitive matrix for generated profile bearer path, manual
  `X-Codex-Router-Token` header path, equal mixed-carrier success, mismatched
  mixed-carrier failure, query, cookie, HTTP body token field, WebSocket carrier
  input normalization, and nested prompt/tool canaries.
- T5 proof must pass without editing `crates/codex-router-proxy/src/websocket.rs`
  or `crates/codex-router-proxy/src/server.rs`; end-to-end WebSocket ingress,
  non-101, subprotocol, and call-counter proof belongs to T6.
- Call counters proving failure before selector, credential, auth injection, and
  upstream open for HTTP/SSE paths owned by T5.

Checkpoint:

- May be included with T3 proxy checkpoint if implemented in same diff; otherwise
  commit separately after T5-owned shared local-auth primitive and HTTP/SSE
  security tests pass. Do not require T6 WebSocket ingress proof for the T5
  checkpoint.

### T6. WebSocket Preselection, Affinity, And Pinning

Write scope:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-test-support/src/mock_upstream.rs`
- `crates/codex-router-test-support/src/transcript.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- WebSocket-focused tests under `crates/codex-router-test-support/src/*`

Implement:

- Depend on T5's shared local-auth primitive. Do not duplicate carrier parsing
  in WebSocket-only code.
- Pre-upgrade local auth and unsupported path rejection for WebSocket.
- Pre-upgrade manual `X-Codex-Router-Token` success, equal mixed-carrier
  success, mismatched mixed-carrier failure, and
  `Sec-WebSocket-Protocol` token-smuggling rejection.
- First-frame max size, timeout, accepted shape, direct installed-Codex payload
  structural checks, and top-level auth-smuggling rejection.
- First-frame allowlist proof fields only; delete or replace stale
  `first_frame_model`, `first_frame_has_input`, and `first_frame_stream`.
- Affinity secret load/create before selector advancement for `/v1/responses`.
- Fail `affinity_secret_unavailable` before selector, credential, auth
  injection, and upstream open.
- Connection account pinning for WebSocket lifetime.
- On successful upstream WebSocket response creation, write previous-response
  owner records only from allowlisted top-level `response.id` frames and the
  already-selected pinned account metadata.
- If the affinity secret is missing, unreadable, or replaced during a
  continuation, fail closed and ignore or purge stale owner rows; never silently
  trust owner rows under a regenerated secret.

Proof:

- WebSocket preselection failure matrix with zero side-effect call counters.
- WebSocket ingress matrix rows for bearer success, manual header success,
  equal mixed-carrier success, mismatched mixed-carrier failure,
  `Sec-WebSocket-Protocol` token smuggling rejection, and first-frame
  auth-smuggling rejection.
- Non-101 proof for invalid local auth, unsupported WebSocket paths, and
  forbidden subprotocol token smuggling.
- Post-upgrade zero-side-effect proof for malformed, wrong-type, oversized,
  timed-out, first-frame auth-smuggling, and affinity-secret first-frame
  failures. These cases prove local upgrade accepted, then zero selector
  advancement, zero credential resolution, zero auth injection, and zero
  upstream open.
- Direct installed-Codex first-frame compatibility tests.
- Canary redaction tests over logs/audit/smoke transcript.
- WebSocket affinity owner hit and failure tests.
- WebSocket owner-record write test proving allowlisted `response.id`
  extraction, persisted safe metadata, no raw previous-response id leakage, and
  no parsing from other frame/body fields.
- Secret-loss/replacement recovery test proving continuation fail-closed and
  stale owner row ignore-or-purge behavior.

Checkpoint:

- Commit after WebSocket integration/security proof passes.

### T7. Non-Blocking Refresh Black-Box Proof

Write scope:

- `crates/codex-router-test-support/src/*` fake refresh/provider hooks
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-cli/src/quota.rs`

Scope gate:

- Any required change outside this allowlist stops T7 and routes back to plan
  revision. Do not change unrelated runner/tooling layers to make this proof
  pass.

Implement:

- Served-router test with delayed/failing refresh that proves bind/listen
  readiness.
- First routed HTTP and WebSocket requests use persisted state while refresh is
  delayed/failing.
- `quota status` renders persisted state immediately with stale/needs-refresh
  notes.

Proof:

- Black-box tests synchronize on readiness, request completion, and rendered
  output; no wall-clock sleep as proof.

Checkpoint:

- Commit after non-blocking proof passes.

### T8a. Harness Scaffolding And Safe Transcript Contract

Write scope:

- `crates/codex-router-test-support/src/mock_upstream.rs`
- `crates/codex-router-test-support/src/transcript.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `crates/codex-router-test-support/src/lib.rs`
- `tests/smoke/installed_codex_mock.sh`

Implement:

- Stable test-support APIs and redacted transcript schema for route-native and
  installed-Codex proof.
- Exact route-native ignored test prefix:
  `cargo test -p codex-router-test-support route_native_ -- --ignored --nocapture`.
- Exact installed-Codex ignored test prefixes:
  `cargo test -p codex-router-test-support installed_codex_http_sse_ -- --ignored --nocapture`
  and
  `cargo test -p codex-router-test-support installed_codex_websocket_ -- --ignored --nocapture`.
- Test-inventory preflight commands for each prefix. T8a must record
  `cargo test -p codex-router-test-support route_native_ -- --ignored --list`,
  `cargo test -p codex-router-test-support installed_codex_http_sse_ -- --ignored --list`,
  and
  `cargo test -p codex-router-test-support installed_codex_websocket_ -- --ignored --list`
  receipts proving the matched test names/counts are exactly the intended
  route-native, HTTP/SSE, and WebSocket suites before any grouped proof command
  may pass.
- Exact installed-Codex smoke commands:
  `tests/smoke/installed_codex_mock.sh --transport http-sse` and
  `tests/smoke/installed_codex_mock.sh --transport websocket`.
- Transport-specific receipt roots:
  `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/evidence/installed-codex/http-sse/`
  and
  `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/evidence/installed-codex/websocket/`.
- Remove stale forbidden WebSocket first-frame transcript fields before any
  e2e transcript can be used as proof.
- Transcript schema for selected safe label/hash, routing reason, local-auth
  carrier receipt, local-auth stripping, upstream auth injection, status
  agreement, and forbidden-canary scan inputs.

Proof:

- Harness API compiles.
- Unit tests or snapshot tests prove transcript redaction helpers reject token,
  header, raw body/frame, prompt, raw previous-response id, unsafe label, and
  affinity-secret canaries.

Checkpoint:

- Commit after T8a harness contract proof passes.

### T8b. Route-Native Black-Box Proof

Write scope:

- `crates/codex-router-test-support/src/mock_upstream.rs`
- `crates/codex-router-test-support/src/transcript.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- route-native tests under `crates/codex-router-test-support/src/*`

Implement:

- Served local router plus mock upstream proof for:
  - `POST /v1/responses`
  - WebSocket `/v1/responses`
  - `GET /v1/models`
  - `POST /v1/memories/trace_summarize`
  - `POST /v1/responses/compact`
  - unsupported HTTP paths and wrong methods
  - unsupported WebSocket paths and invalid auth
- Each route proves local auth, quota-based selection for route band, upstream
  auth injection, local-auth stripping, protocol/header/body preservation, and
  safe transcript fields.

Proof:

- `cargo test -p codex-router-test-support route_native_ -- --ignored --list`
  matches only the intended route-native ignored tests and records the matched
  names/count before execution.
- `cargo test -p codex-router-test-support route_native_ -- --ignored --nocapture`
  passes with redacted artifacts.

Checkpoint:

- Commit after T8b route-native black-box suite passes.

### T9. Installed-Codex HTTP/SSE E2E

Write scope:

- HTTP/SSE installed-Codex tests in
  `crates/codex-router-test-support/src/installed_codex.rs`
- HTTP/SSE smoke command handling in `tests/smoke/installed_codex_mock.sh`
- HTTP/SSE evidence under
  `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/evidence/installed-codex/http-sse/`

Implement:

- Generated Codex profile in temp `CODEX_HOME`.
- `CODEX_ROUTER_TOKEN` exported to installed Codex.
- Mock upstream response path for HTTP/SSE.
- Fixture with multiple persisted accounts that force reset-aware selected
  account.
- Transcript proves selected safe label/hash, routing reason, local auth carrier
  receipt, local auth stripping, and status agreement.

Proof:

- `cargo test -p codex-router-test-support installed_codex_http_sse_ -- --ignored --list`
  matches only the intended HTTP/SSE installed-Codex ignored tests and records
  the matched names/count before execution.
- `cargo test -p codex-router-test-support installed_codex_http_sse_ -- --ignored --nocapture`
  passes the HTTP/SSE path with redacted transcript.
- `tests/smoke/installed_codex_mock.sh --transport http-sse` invokes the same
  underlying HTTP/SSE path and records the command plus artifacts under the
  HTTP/SSE evidence root.

Checkpoint:

- Commit after installed-Codex HTTP/SSE e2e passes.

### T10. Installed-Codex WebSocket E2E

Write scope:

- WebSocket installed-Codex tests in
  `crates/codex-router-test-support/src/installed_codex.rs`
- WebSocket mock upstream and transcript helpers in
  `crates/codex-router-test-support/src/mock_upstream.rs` and
  `crates/codex-router-test-support/src/transcript.rs`
- WebSocket smoke command handling in `tests/smoke/installed_codex_mock.sh`
- WebSocket evidence under
  `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/evidence/installed-codex/websocket/`

Implement:

- Ensure installed Codex uses WebSocket transport through generated provider.
- Prove local router receives `Authorization: Bearer` on WebSocket upgrade with
  audit-safe receipt fields.
- Prove local auth carriers are stripped before the upstream WebSocket open.
- Prove selected account is pinned for connection lifetime.
- Prove status output agrees with the selected safe label/hash and routing
  reason used by the WebSocket session.
- Prove first-frame transcript contains only allowlisted safe fields.

Proof:

- `cargo test -p codex-router-test-support installed_codex_websocket_ -- --ignored --list`
  matches only the intended WebSocket installed-Codex ignored tests and records
  the matched names/count before execution.
- `cargo test -p codex-router-test-support installed_codex_websocket_ -- --ignored --nocapture`
  passes with:
  - `websocket.local_auth_carrier=authorization_bearer`
  - `websocket.local_auth_validated=true`
  - `websocket.local_auth_stripped_before_upstream=true`
  - no token/header/hash/length/prefix
  - selected safe label/hash and routing reason
  - status agreement with selected account
  - no forbidden first-frame fields
- `tests/smoke/installed_codex_mock.sh --transport websocket` invokes the same
  underlying WebSocket path and records the command plus artifacts under the
  WebSocket evidence root.

Checkpoint:

- Commit after installed-Codex WebSocket e2e passes.

### T11. Final Redaction And Whole-Repo Validation

Write scope:

- Files already touched by T0 through T10.
- Plan receipts and validation artifacts under
  `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/`.

Scope gate:

- Any product code, test-support, config, script, or tooling file not already
  touched by T0 through T10 stops T11 and routes back to plan revision.

Implement:

- Run final canary searches over status, JSON, audit, logs/traces if present,
  smoke transcripts, route-native artifacts, installed-Codex artifacts, shared
  JSON captures, and plan/review/implementation receipts.
- Fix only in-scope leaks or proof gaps.

Proof:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace`
- `cargo deny check`
- `cargo audit`
- `tests/smoke/quota_status_fixture.sh`
- `cargo test -p codex-router-test-support route_native_ -- --ignored --list`
- `cargo test -p codex-router-test-support route_native_ -- --ignored --nocapture`
- `cargo test -p codex-router-test-support installed_codex_http_sse_ -- --ignored --list`
- `cargo test -p codex-router-test-support installed_codex_http_sse_ -- --ignored --nocapture`
- `cargo test -p codex-router-test-support installed_codex_websocket_ -- --ignored --list`
- `cargo test -p codex-router-test-support installed_codex_websocket_ -- --ignored --nocapture`
- `tests/smoke/installed_codex_mock.sh --transport http-sse`
- `tests/smoke/installed_codex_mock.sh --transport websocket`
- Redaction negative search over produced artifacts for token/header/raw body/
  raw frame/prompt/raw previous-response id/unsafe label/affinity secret/
  `router_affinity_hash_secret.v1`/secret-store backend identifier/
  derived-secret-material/shared JSON `account_id` canaries.

Checkpoint:

- Commit final validation receipt.

## Execution DAG

```text
gate 0: source and repo freshness
  proof: git status, HEAD, spec line count, R20 ready ledger

  +-- T0 core primitives
      |
      +-- T1 pure assessment
      |
      +-- T2a state refresh read model
          |
          +-- T2b affinity owner storage cutover
              |
              +-- T2c affinity secret store contract

integration gate 1: lower contracts compile and prove
  requires: T0, T1, T2a, T2b, T2c

  +-- T3 proxy runtime route/selection
  +-- T4 CLI status/profile
  +-- T8a harness scaffolding safe transcript cleanup

integration gate 2: adapters compile and prove
  requires: T3, T4, T8a

  +-- T5 shared local auth + HTTP/SSE auth security
      |
      +-- T6 WebSocket preselection/security/pinning
          |
          +-- T7 non-blocking black-box

security integration gate
  requires: T5, T6, T7

route-native black-box gate
  requires: T3, T5, T6, T8a
  |
  T8b route-native black-box proof

installed-Codex e2e gate
  requires: T8b
  T9 HTTP/SSE
  |
  T10 WebSocket

T11 final validation
  |
implementation-review-swarm
  |
plan-review/implementation findings folded before PR-ready claim
```

## Parallel Work Rules

- T0 is a serial starting gate because it defines shared public types.
- T1 may proceed after T0.
- T2a, T2b, and T2c are serial because they define one state/secret recovery
  contract and share schema/repository surfaces.
- T3 and T4 may proceed in parallel after T1/T2a/T2b/T2c interfaces are stable
  because T3 owns runtime/proxy adapters while T4 owns CLI rendering.
- T8a may proceed after lower interfaces are stable and must freeze harness
  APIs before T8b, T9, or T10 edit transcript/test-support surfaces.
- T3, T5, and T6 are serial on proxy/auth surfaces: T3 establishes runtime
  decision and call-counter hooks, T5 establishes shared local auth, then T6
  consumes that shared auth for WebSocket.
- T7 starts after T6. The non-blocking WebSocket assertions must run against
  the final T6 WebSocket ingress, affinity, and pinning path.
- T8b starts after T3/T5/T6 and T8a pass.
- T9 and T10 are serial because they share the installed-Codex harness and
  smoke script. T9 proves HTTP/SSE first, then T10 proves WebSocket using the
  command and artifact contract frozen by T8a.

## Split / Replan Triggers

- If `RouteBand` or `BurnDownRouteBandAssessmentResult` causes broad compile
  churn across more than three crates, stop and split an interface-only
  checkpoint before behavior changes.
- If state schema migration, affinity owner cutover, or affinity secret
  lifecycle conflict, keep the T2a/T2b/T2c serial boundary and do not merge
  them into one proof gate.
- If T5 shared local-auth changes force WebSocket-specific divergence, stop and
  route back to plan revision rather than implementing two auth contracts.
- If WebSocket proof requires changing installed Codex behavior, stop and
  capture exact installed-Codex evidence before changing router assumptions.
- If an e2e gate fails because installed `codex` is unavailable, do not replace
  it with route-native proof. Mark installed-Codex e2e blocked and preserve all
  lower-layer proof.
- If the route-native, HTTP/SSE installed-Codex, or WebSocket installed-Codex
  exact test prefixes do not exist after T8a, stop before T9/T10 and fix the
  harness contract.
- If T7 or T11 needs to edit outside its allowlist, stop and route back to plan
  revision.
- If redaction canaries appear in captured artifacts, stop feature work and fix
  the leak before continuing.

## Security And Reliability Notes

- Secrets never enter selection.
- Affinity secret never enters SQLite, logs, status, JSON, traces, smoke
  transcript, or review artifacts.
- Raw previous-response ids and raw canonical affinity keys are never persisted.
- Unsupported and auth-failed routes must show zero selector advancement, zero
  credential resolver calls, zero upstream auth injection, and zero upstream
  open.
- Background quota refresh may fail or be delayed; startup, request selection,
  and status render must continue from persisted SQLite rows.
- Hard cutover is required for affinity storage. No raw-key fallback.

## Rollback / Recovery

- Before release or live manual testing, back up the local router SQLite state.
- New schema may be incompatible with an older binary; do not add a dual-schema
  compatibility path.
- If affinity secret is missing, unreadable, or replaced, fail closed and expose
  a redacted repair path; do not silently regenerate and trust old rows.
- Existing raw affinity rows are discarded or ignored by the hard cutover.

## Required Next Workflow

Run a second focused `shravan-dev-workflow:plan-review-swarm` pass against this
revised plan before any implementation execution. Implementation may start only
after accepted plan review findings are folded in or explicitly rejected with
evidence and the focused pass returns ready or no new blockers.
