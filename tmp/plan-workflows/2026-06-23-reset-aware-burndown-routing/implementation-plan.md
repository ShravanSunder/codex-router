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
- R20 review ledger: 67 lines, loaded.
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
- `crates/codex-router-test-support/src/installed_codex.rs` still emits stale
  forbidden first-frame transcript fields.

## Requirements / Proof Matrix

| id | requirement / claim | source | owning task | proof layer | evidence |
| --- | --- | --- | --- | --- | --- |
| RP-01 | Startup and routing never block on live quota refresh | R1 | T2, T7 | integration + black-box | delayed/failing refresh tests over bind/listen, first request, and status |
| RP-02 | Exhausted/ineligible relevant windows block account | R2 | T1 | unit | burn-down collapse tests |
| RP-03 | Unknown quota is fallback-only, not free capacity | R3 | T1, T3, T4 | unit + integration + status | unknown pool and status tests |
| RP-04 | Weekly pressure dominates 5h urgency unless weekly reset is near | R4, R5 | T1 | unit | scenario tests for weekly far/near reset |
| RP-05 | Runtime selection and status share assessment semantics | R6, R7 | T1, T3, T4 | integration + smoke | shared DTO tests and quota status snapshots |
| RP-06 | Generated profile auth uses `CODEX_ROUTER_TOKEN` and local `Authorization: Bearer` | R8 | T5, T9, T10 | integration + e2e | profile tests and installed-Codex transcript receipts |
| RP-07 | WebSocket preselects from allowlisted first-frame view | R9 | T6, T10 | integration + e2e | first-frame matrix and installed-Codex WebSocket proof |
| RP-08 | Route result and unknown fallback are first-class flat contracts | R10 | T1, T3, T4 | unit + integration | flat envelope JSON/status/runtime tests |
| RP-09 | Previous-response affinity is HMAC hashed, durable, fail-closed, and never raw-key persisted | affinity contract | T0, T2, T3, T6 | unit + integration + security | core/state/secret/proxy affinity tests |
| RP-10 | HTTP/SSE order is local auth, route, assessment, optional affinity, credential, auth injection, strip, upstream | HTTP/SSE contract | T3, T5 | integration/security | call-order counters and negative side-effect tests |
| RP-11 | WebSocket invalid auth and unsupported paths fail before local upgrade | WebSocket contract | T6, T8 | black-box/security | non-101 pre-upgrade proof |
| RP-12 | Status table/plain/json use safe labels, account-centric rows, bars, reasons, and no forbidden text | status contract | T4 | unit + smoke | table/plain/json snapshots and negative searches |
| RP-13 | Every routed API has route-native success and fail-closed proof | route inventory | T8 | route-native black-box | mock upstream transcript per route |
| RP-14 | Installed Codex HTTP/SSE and WebSocket both work through router | proof expectations | T9, T10 | installed-Codex e2e | `installed_codex_mock.sh` transcript |
| RP-15 | Logs/traces/audit/smoke artifacts redact tokens, headers, raw body/frame, prompts, unsafe labels, affinity secrets | security context | T11 | security + smoke | canary negative searches over captured artifacts |

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
- exports in `crates/codex-router-selection/src/lib.rs` if needed

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

### T2. State Refresh Read Model, Affinity Owner Store, And Secret Store

Write scope:

- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/lib.rs`
- `crates/codex-router-secret-store/src/*`

Implement:

- `quota_refresh_status` schema and DTOs.
- `selector_inputs_for_route_band(route_band, now_unix_seconds)` with stale
  read overlay.
- `quota_refresh_statuses_for_route_band(route_band)` sorted by account id.
- Atomic refresh success/failure repository operations.
- Hard cutover from raw affinity pins to hash-only previous-response owner
  records with `credential_generation`, `route_band`, `source_transport`, and
  `created_unix_seconds`.
- Secret-store API for `router_affinity_hash_secret.v1`, 32-byte entropy,
  64-lowercase-hex storage, loaded/newly-created state, and redacted error
  classes.

Proof:

- SQLite integration tests for migration, stale overlay, legacy missing status,
  success/failure atomic operations, sorted status read, and row preservation.
- Affinity repository tests for hash-only methods, no raw keys, missing/found/
  ambiguous owner lookup, and purge behavior.
- Secret-store tests for stable key, lifecycle, encoding, entropy length,
  error redaction, no debug/display leakage.

Checkpoint:

- Commit after state and secret-store integration proof passes.

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
- Previous-response affinity uses hash owner records and can continue on a
  `usable` or `reserve` owner outside the selected pool without advancing
  weighted fairness.
- Non-capable routes pass top-level `previous_response_id` through as upstream
  payload after local auth/auth-smuggling.
- Unsupported path versus unsupported route-band reasons remain distinct.

Proof:

- Proxy integration tests for every route inventory row.
- Call-order tests for route, assessment, affinity/weighted, credential, auth
  injection, strip, upstream.
- Tests proving route-band partitioning of weighted state and holds.
- Tests proving affinity hit does not advance weighted state and failure never
  falls back to another account.

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
  labels, no raw account ids, no raw scores, no `pp`, no `bottleneck`.
- Plain output uses ASCII bars and the same phrases.
- JSON output uses the normative flat route-level and account-level schema.
- Status joins refresh status metadata for stale/needs-refresh notes without
  recomputing stale semantics.
- Generated profile proof remains `env_key = "CODEX_ROUTER_TOKEN"` and
  `supports_websockets = true`.

Proof:

- CLI unit/golden tests for table/plain/json.
- Negative text assertions for forbidden words and secret-like material.
- JSON schema tests for stable enums and `window_slots`.
- Smoke fixture over persisted state for table, plain, and json.

Checkpoint:

- Commit after CLI unit and smoke proof passes.

### T5. Local Auth And HTTP/SSE Security Contract

Write scope:

- `crates/codex-router-core/src/local_auth.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/headers.rs`
- proxy tests

Implement:

- Structured accepted-carrier extraction for `Authorization: Bearer` and
  `X-Codex-Router-Token`.
- Mixed-carrier equality validation.
- Rejection for query, cookie, and top-level HTTP JSON body forbidden fields.
- Narrow body check only for supported JSON POST routes.
- Strip local auth carriers and hop-by-hop headers before upstream open.

Proof:

- Local auth matrix for generated profile bearer path, manual header path,
  mixed-carrier failure, query, cookie, body token field, and nested prompt/tool
  canaries.
- Call counters proving failure before selector, credential, auth injection, and
  upstream open.

Checkpoint:

- May be included with T3 proxy checkpoint if implemented in same diff; otherwise
  commit separately after security tests pass.

### T6. WebSocket Preselection, Affinity, And Pinning

Write scope:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-test-support/src/*` as needed for WebSocket proof

Implement:

- Pre-upgrade local auth and unsupported path rejection for WebSocket.
- First-frame max size, timeout, accepted shape, direct installed-Codex payload
  structural checks, and top-level auth-smuggling rejection.
- First-frame allowlist proof fields only; delete or replace stale
  `first_frame_model`, `first_frame_has_input`, and `first_frame_stream`.
- Affinity secret load/create before selector advancement for `/v1/responses`.
- Fail `affinity_secret_unavailable` before selector, credential, auth
  injection, and upstream open.
- Connection account pinning for WebSocket lifetime.

Proof:

- WebSocket preselection failure matrix with zero side-effect call counters.
- Non-101 proof for invalid local auth and unsupported WebSocket paths.
- Direct installed-Codex first-frame compatibility tests.
- Canary redaction tests over logs/audit/smoke transcript.
- WebSocket affinity owner hit and failure tests.

Checkpoint:

- Commit after WebSocket integration/security proof passes.

### T7. Non-Blocking Refresh Black-Box Proof

Write scope:

- test-support fake refresh/provider hooks if needed
- proxy/CLI startup plumbing only if current design blocks proof

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

### T8. Route-Native Black-Box Harness

Write scope:

- `crates/codex-router-test-support/src/mock_upstream.rs`
- `crates/codex-router-test-support/src/transcript.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- test-support route-native tests

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

- Ignored or dedicated route-native black-box command with redacted artifacts.

Checkpoint:

- Commit after route-native black-box suite passes.

### T9. Installed-Codex HTTP/SSE E2E

Write scope:

- installed-Codex test-support harness
- smoke script if needed

Implement:

- Generated Codex profile in temp `CODEX_HOME`.
- `CODEX_ROUTER_TOKEN` exported to installed Codex.
- Mock upstream response path for HTTP/SSE.
- Fixture with multiple persisted accounts that force reset-aware selected
  account.
- Transcript proves selected safe label/hash, routing reason, local auth carrier
  receipt, local auth stripping, and status agreement.

Proof:

- `tests/smoke/installed_codex_mock.sh` or exact equivalent e2e command passes
  HTTP/SSE path with redacted transcript.

Checkpoint:

- Commit after installed-Codex HTTP/SSE e2e passes.

### T10. Installed-Codex WebSocket E2E

Write scope:

- installed-Codex test-support harness
- WebSocket mock upstream
- smoke transcript writer

Implement:

- Ensure installed Codex uses WebSocket transport through generated provider.
- Prove local router receives `Authorization: Bearer` on WebSocket upgrade with
  audit-safe receipt fields.
- Prove selected account is pinned for connection lifetime.
- Prove first-frame transcript contains only allowlisted safe fields.

Proof:

- Installed-Codex WebSocket e2e passes with:
  - `websocket.local_auth_carrier=authorization_bearer`
  - `websocket.local_auth_validated=true`
  - no token/header/hash/length/prefix
  - selected safe label/hash and routing reason
  - no forbidden first-frame fields

Checkpoint:

- Commit after installed-Codex WebSocket e2e passes.

### T11. Final Redaction And Whole-Repo Validation

Write scope:

- narrowly scoped fixes only from validation output
- plan receipts under `tmp/plan-workflows/...` if needed

Implement:

- Run final canary searches over status, JSON, audit, logs/traces if present,
  and smoke transcripts.
- Fix only in-scope leaks or proof gaps.

Proof:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace`
- `tests/smoke/quota_status_fixture.sh`
- `cargo test -p codex-router-test-support route_native_ -- --ignored --nocapture`
- `tests/smoke/installed_codex_mock.sh`
- Redaction negative search over produced artifacts.

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
      +-- T2 state + secret substrate

integration gate 1: lower contracts compile and prove
  requires: T0, T1, T2

  +-- T3 proxy runtime route/selection
  +-- T4 CLI status/profile
  +-- T8 harness scaffolding safe transcript cleanup

integration gate 2: adapters compile and prove
  requires: T3, T4, T8 scaffold

  +-- T5 HTTP/SSE auth security
  +-- T6 WebSocket preselection/security/pinning
  +-- T7 non-blocking black-box

security integration gate
  requires: T5, T6, T7

route-native black-box gate
  requires: T3, T5, T6, T8

installed-Codex e2e gate
  +-- T9 HTTP/SSE
  +-- T10 WebSocket

T11 final validation
  |
implementation-review-swarm
  |
plan-review/implementation findings folded before PR-ready claim
```

## Parallel Work Rules

- T0 is a serial starting gate because it defines shared public types.
- T1 and T2 may proceed in parallel after T0 if their write scopes remain
  disjoint.
- T3, T4, and T8 may proceed in parallel after T1/T2 interfaces are stable.
- T5 and T6 can proceed in parallel only after T3 establishes runtime decision
  and call-counter hooks.
- T9 and T10 can proceed in parallel only after route-native black-box proof
  is passing.

## Split / Replan Triggers

- If `RouteBand` or `BurnDownRouteBandAssessmentResult` causes broad compile
  churn across more than three crates, stop and split an interface-only
  checkpoint before behavior changes.
- If state schema migration and affinity owner cutover conflict, keep them in
  one state checkpoint but split into serial commits inside that checkpoint.
- If WebSocket proof requires changing installed Codex behavior, stop and
  capture exact installed-Codex evidence before changing router assumptions.
- If an e2e gate fails because installed `codex` is unavailable, do not replace
  it with route-native proof. Mark installed-Codex e2e blocked and preserve all
  lower-layer proof.
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

Run `shravan-dev-workflow:plan-review-swarm` against this plan before any
implementation execution. Implementation may start only after accepted plan
review findings are folded in or explicitly rejected with evidence.
