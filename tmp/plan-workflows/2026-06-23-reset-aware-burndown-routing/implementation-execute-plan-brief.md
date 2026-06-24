# Implementation Execute Plan Brief

Date: 2026-06-23
Workflow: `shravan-dev-workflow:implementation-execute-plan`
Plan:
`tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/implementation-plan.md`

## Coverage

- Implementation plan: 840 lines, loaded before execution.
- Focused plan review closure committed and pushed at `897df79`.
- Current execution starts at T0 because T0 is the serial gate for shared core
  primitives.

## T0 Core Contract Primitives

Plan rows:

- RP-09 previous-response affinity primitives and no raw key leakage.
- RP-12 shared safe labels for status/log/audit/smoke output.
- RouteBand support for `responses`, `responses_compact`, `models`, and
  `memories_trace_summarize`.

Files changed:

- `crates/codex-router-core/Cargo.toml`
- `crates/codex-router-core/src/lib.rs`
- `crates/codex-router-core/src/routes.rs`
- `crates/codex-router-core/src/affinity.rs`
- `crates/codex-router-core/src/redaction.rs`

Implemented:

- `RouteBand` enum with stable snake-case string, display, serde names.
- `PreviousResponseId`, `AffinityKeyHash`, `RouterAffinityHashSecret`, and
  `hash_previous_response_id`.
- `SafeAccountLabel`, `safe_account_label`, and unsafe-label predicate with
  deterministic `acct-<12 lowercase hex>` fallback from account id.

Proof:

- `cargo fmt --all -- --check` passed.
- `cargo test -p codex-router-core` passed: 15 tests.
- `cargo check --workspace` passed.

Notes:

- No downstream call sites were changed in T0.
- T1 will migrate selection to consume these core primitives.

## T1 Pure Burn-Down Assessment Contract

Plan rows:

- RP-01 reset-aware scoring fallback behavior.
- RP-02 known usable/reserve pools outrank unknown fallback.
- RP-03 unknown quota is a non-blocking fallback path, not startup failure.
- RP-12 safe account labels applied to selection/status output.

Files changed:

- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/lib.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/lib.rs`

Implemented:

- `BurnDownRouteBandAssessmentInput` now uses typed `RouteBand`; per-account
  route-band strings were removed from burn-down input.
- Assessment output is `BurnDownRouteBandAssessmentResult` with route band and
  route status fields.
- Unknown or missing quota evidence now produces an `Unknown` fallback pool with
  conservative weight `1` when no known usable/reserve accounts exist.
- Known usable and reserve pools still outrank unknown fallback accounts.
- Account labels emitted by assessment now use core `safe_account_label`.
- Proxy and quota CLI were migrated to the typed route-band contract.

Proof:

- `cargo test -p codex-router-selection` passed: 21 tests.
- `cargo fmt --all -- --check` passed.
- `cargo test -p codex-router-proxy -p codex-router-cli` passed:
  60 CLI tests, 71 proxy tests, and doc-test stubs.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 71, quota 4, secret-store 9,
  selection 21, state 13, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- Existing tests that previously expected "needs probe -> no route" were
  updated to the new startup contract: partial/missing window metadata remains
  visible as "needs probe" in the quota cells while routing uses the fallback
  account instead of blocking startup.

## T2a State Refresh Read Model

Plan rows:

- RP-01 startup/routing do not block on live quota refresh.
- RP-03 unknown and stale quota are distinct fallback/staleness states.
- Refresh read-model contract for `quota_refresh_status` and read-time stale
  overlay.

Files changed:

- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/lib.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-proxy/src/account_selection.rs`

Implemented:

- SQLite schema v5 adds `quota_refresh_status` keyed by account and route band.
- Added redacted `QuotaRefreshErrorClass`, `QuotaRefreshStatusSource`, and
  `QuotaRefreshStatusView`.
- `SelectorQuotaRepository::selector_inputs_for_route_band` now takes
  `now_unix_seconds` and overlays stale status at read time without mutating
  stored selector windows.
- Added atomic success/failure repository operations:
  `record_refresh_success_and_replace_selector_windows` and
  `record_refresh_failure_preserving_selector_windows`.
- Refresh success now writes selector windows plus refresh status through the
  atomic repository operation.
- Credential/provider refresh failures now preserve selector windows and record
  redacted refresh error metadata.

Proof:

- `cargo test -p codex-router-state` passed: 16 tests.
- `cargo test -p codex-router-cli` passed: 60 tests.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 71, quota 4, secret-store 9,
  selection 21, state 16, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- Current refresh success uses `observed_unix_seconds + 600` for
  `stale_after_unix_seconds`, matching the minimum grace in the spec. Threading
  the configured background interval into the refresh worker remains for the
  later non-blocking refresh slice.

## T2c Affinity Secret Store Contract

Plan rows:

- RP-09 previous-response affinity HMAC secret is router-owned and redacted.
- RP-15 artifacts must not expose affinity secrets or secret-store identifiers.

Files changed:

- `crates/codex-router-secret-store/src/affinity_secret.rs`
- `crates/codex-router-secret-store/src/lib.rs`

Implemented:

- Added stable `router_affinity_hash_secret.v1` secret-store key.
- Added `load_or_create_router_affinity_hash_secret`, returning lifecycle
  origin `loaded_existing` or `created_new`.
- Generated secret is 32 bytes of OS entropy encoded as 64 lowercase hex and
  validated by core `RouterAffinityHashSecret`.
- Malformed stored payloads fail with redacted `InvalidSecretPayload`.

Proof:

- `cargo test -p codex-router-secret-store` passed: 11 tests.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 71, quota 4, secret-store 11,
  selection 21, state 16, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- This was implemented before T2b hash-owner state because runtime hash
  production depends on the secret lifecycle. T2b remains the next state slice.

## T2b Affinity Owner Storage

Plan rows:

- RP-09 previous-response owner rows use HMAC hashes, safe metadata, route-band
  partitioning, and fail-closed ambiguous lookup.

Files changed:

- `crates/codex-router-core/src/routes.rs`
- `crates/codex-router-state/src/affinity_owner.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/lib.rs`

Implemented:

- Added `RouteBand::parse` for state round trips.
- SQLite schema v6 adds `previous_response_affinity_owners`.
- Added `PreviousResponseAffinityOwnerRecord`,
  `PreviousResponseAffinityOwnerLookup`, and `AffinitySourceTransport`.
- Added repository methods to write, load, and purge hashed owner records.
- Lookup is route-scoped and returns `missing`, `found`, or `ambiguous`.
- Tests prove the new table stores only `AffinityKeyHash` plus safe metadata,
  not raw previous-response IDs.

Proof:

- `cargo test -p codex-router-core -p codex-router-state` passed:
  core 15 tests, state 18 tests.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 71, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- The new hash-owner repository is present and proven. The legacy proxy
  affinity adapter still calls the old raw affinity methods; that runtime
  cutover requires the T3 proxy secret/selection adapter work so the proxy can
  compute `AffinityKeyHash` before lookup.

## T3a Proxy Runtime Affinity Secret And Hash Lookup Cutover

Plan rows:

- RP-09 previous-response affinity is HMAC hashed and fail-closed.
- RP-10 HTTP/SSE order gates affinity secret before selector advancement.
- RP-11 WebSocket continuation affinity can use the same hash-owner contract.

Files changed:

- `crates/codex-router-proxy/src/routes.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/lib.rs`

Implemented:

- `RouteKind` now exposes shared `RouteBand` and previous-response affinity
  capability metadata.
- `AccountDecisionSelector` accepts optional router affinity secret material.
- Repository-backed selection parses top-level `previous_response_id`, computes
  `AffinityKeyHash`, and loads `PreviousResponseAffinityOwnerRecord` instead of
  raw `AffinityKey` pins.
- Affinity owner hits no longer advance weighted fairness state.
- HTTP/SSE response-capable routes load/create the affinity secret before
  selector advancement, credential resolution, auth injection, or upstream open.
- Production runtime wires the router secret store into HTTP/SSE and WebSocket
  continuation affinity paths.
- WebSocket first-frame continuation affinity can use the same hash-owner lookup
  when `previous_response_id` is present.

Proof:

- `cargo test -p codex-router-proxy` passed: 72 tests.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 72, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- This slice does not yet write new owner rows from successful upstream
  HTTP/SSE or WebSocket response bodies. That remains the next T3/T6 runtime
  capture slice.
- Legacy raw affinity repository methods and SQLite table still exist for now,
  but proxy runtime selection no longer calls them.

## T3b HTTP/SSE Owner Record Capture

Plan rows:

- RP-09 successful response IDs become HMAC-hashed owner rows and raw response
  IDs are not persisted.
- RP-10 HTTP/SSE owner writes use already-selected account metadata and the
  resolved credential generation.

Files changed:

- `crates/codex-router-auth/src/resolver.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/lib.rs`

Implemented:

- `ResolvedProviderCredential` now carries the credential generation that
  actually produced the upstream access token, including refresh paths.
- HTTP/SSE service records owner rows from allowlisted top-level upstream
  response `id` fields only.
- Nested response/body IDs are ignored.
- Streaming/SSE responses are not pre-buffered; the response body is wrapped and
  records after EOF while bytes pass through to the client.
- Production loopback runtime uses an owned SQLite recorder that opens the
  state DB at record time, avoiding a long-lived SQLite connection inside the
  streaming body.
- Assembled runtime proof verifies a streamed SSE response creates a hashed
  `previous_response_affinity_owners` row.

Proof:

- `cargo test -p codex-router-auth -p codex-router-proxy` passed:
  auth 13 tests, proxy 75 tests.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 75, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- WebSocket owner-record writes from upstream `response.id` frames remain in
  the T6 WebSocket capture slice.
- Legacy raw affinity repository methods and SQLite table still exist, but the
  proxy HTTP/SSE selection and owner-write path use hash-owner records.

## T4a Quota Status Public Reasons And Human UX

Plan rows:

- T4 status renderer consumes shared burn-down assessment output instead of
  inventing separate routing phrases.
- Default table uses comfy-table, one logical row per account, Unicode bars,
  safe labels, no raw account ids, no `pp`, and no `bottleneck`.
- Plain output uses ASCII bars and the same stable wording.
- JSON output uses route-level fields, weighted candidates, stable routing
  reason codes, and `window_slots`.

Files changed:

- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/lib.rs`

Implemented:

- Replaced coarse public routing reasons with stable spec reasons such as
  `preferred_weekly_healthier`, `held_unknown`,
  `unknown_fallback_preferred`, `blocked_window_exhausted`, and
  `excluded_missing_credential`.
- Added shared `RoutingReason::as_str()` and `RoutingReason::human_phrase()` so
  proxy audit strings, JSON, table, and plain output use one vocabulary.
- Added deterministic routing-reason precedence in the selection crate for
  weekly reset, weekly health, short reset, same-pool availability, held
  reserve, held unknown, fallback, blocked, and excluded outcomes.
- Quota status table now shows `<percent>% left`, `no data`, `unknown`, `reset
  unknown`, `needs refresh`, `limiting window`, and spec `next use` values:
  `preferred`, `available`, `held`, `blocked`, or `fallback`.
- Plain mode now uses ASCII bars (`#`/`-`) instead of Unicode.
- Human status rows now display the assessment's safe account label rather than
  raw account metadata.
- JSON status now includes `route_result`, `selected_pool_reason`,
  `weighted_candidates`, stable `routing_reason`, `limiting_window`, pressure
  and salvage fields, `window_slots`, and per-window safe metadata.

Proof:

- `cargo test -p codex-router-selection` passed: 21 tests.
- `cargo test -p codex-router-cli quota_status -- --nocapture` passed:
  5 focused quota status tests.
- `cargo test -p codex-router-proxy -p codex-router-cli -p codex-router-selection`
  passed: CLI 60, proxy 75, selection 21.
- Live render inspection:
  `cargo run -q -p codex-router-cli -- quota status --format table
  --now-unix-seconds 1700000000` showed one account row plus one continuation
  line, Unicode bars, `left`, stable routing phrases, and no raw account ids in
  human output.
- Live render inspection:
  `cargo run -q -p codex-router-cli -- quota status --format plain
  --now-unix-seconds 1700000000` showed ASCII bars and the same phrases.
- Live render inspection:
  `cargo run -q -p codex-router-cli -- quota status --format json
  --now-unix-seconds 1700000000` showed route-level fields,
  `weighted_candidates`, stable reasons, and `window_slots`.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 75, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- The live render used the current local router root and a fixed historical
  clock, so reset durations are inspection-only and not a freshness claim.
- The table still preserves comfy-table box drawing; this slice intentionally
  did not remove or replace the table library.

## T5 Shared Local Auth Contract

Plan rows:

- Structured accepted-carrier extraction for `Authorization: Bearer` and
  `X-Codex-Router-Token` shared by HTTP/SSE and WebSocket.
- Mixed-carrier equality validation for every transport.
- Rejection for query, cookie, top-level HTTP JSON body, WebSocket
  subprotocol, and first-frame auth-smuggling carriers before selection or
  upstream open.
- Strip local auth carriers and hop-by-hop headers before upstream open.

Files changed:

- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/lib.rs`

Implemented:

- Added shared local-auth extraction that accepts either bearer or
  `X-Codex-Router-Token`, accepts equal mixed carriers, and rejects mismatched
  mixed carriers with `LocalAuthError::Wrong`.
- Added shared query, cookie, and top-level JSON auth-smuggling rejection.
- HTTP/SSE auth now uses the shared extractor before account selection,
  credential resolution, auth injection, or upstream open.
- WebSocket first-frame routing now uses the same extractor for handshake
  carriers and rejects top-level first-frame auth-smuggling before selection.
- Loopback WebSocket preflight now rejects forbidden query/cookie/subprotocol
  carriers before accepting the upgrade.
- Existing stripping proof remains in `sanitize_headers_for_upstream`; fixtures
  that previously used conflicting local auth canaries were updated because
  mismatched mixed carriers now intentionally fail closed.

Proof:

- `cargo test -p codex-router-proxy local_auth -- --nocapture` passed:
  9 focused tests before the full matrix was added.
- `cargo test -p codex-router-proxy` passed: 83 tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 83, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- `LocalAuthError::Wrong` is used for mismatched and forbidden carrier cases so
  existing audit shape stays stable while still failing before selector,
  credential, auth injection, and upstream open.
- T6 still needs WebSocket owner-record writes from upstream `response.id`
  frames and secret-loss/replacement recovery proof.

## T6 WebSocket Owner-Record Writes

Plan rows:

- WebSocket routing uses the same affinity hash secret as HTTP/SSE before
  account selection.
- WebSocket tunnels persist previous-response owner records from upstream
  top-level `response.id` frames using hashed ids only.
- Nested response-id canaries do not create affinity owners.
- Runtime WebSocket dispatch wires the production SQLite owner recorder into
  both normal and revocation-aware tunnel constructors.

Files changed:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/lib.rs`

Implemented:

- `AuthenticatedWebSocketRouter` now always loads the router affinity hash
  secret before selector evaluation, matching the shared assessment and
  previous-response-affinity contract.
- WebSocket first-frame decisions carry safe affinity-owner context for the
  selected account and credential generation.
- `BlockingWebSocketTunnel` can receive an `HttpAffinityOwnerRecorder` and
  records hashed owner rows while forwarding upstream frames.
- Owner extraction is restricted to top-level upstream `response.id`; nested
  canary ids and non-text/malformed frames are ignored.

Proof:

- `cargo test -p codex-router-proxy websocket -- --nocapture` passed:
  22 focused WebSocket tests.
- `cargo test -p codex-router-proxy` passed: 84 tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 84, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- This checkpoint proves WebSocket owner-record writes and runtime recorder
  wiring. Secret-loss/replacement recovery proof remains a later T6/T7-adjacent
  gate before final completion.

## T6 Secret Replacement Recovery Proof

Plan rows:

- If the affinity secret is missing, unreadable, or replaced during a
  continuation, fail closed and ignore stale owner rows.
- Never silently trust previous-response owner rows under a regenerated or
  replaced affinity hash secret.

Files changed:

- `crates/codex-router-proxy/src/lib.rs`

Implemented:

- Added a selector-level proof that an owner row written with the original
  affinity hash secret is ignored when continuation selection uses a replacement
  secret.
- Added a WebSocket router proof that missing affinity-secret provider fails
  before selector advancement and credential resolution.
- Added a WebSocket router proof that continuation with a replaced affinity
  secret fails closed before credential resolution.

Proof:

- `cargo test -p codex-router-proxy affinity -- --nocapture` passed:
  9 focused affinity tests.
- `cargo test -p codex-router-proxy websocket -- --nocapture` passed:
  24 focused WebSocket tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo test -p codex-router-proxy` passed: 87 tests.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 60, core 15, proxy 87, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- This closes the remaining T6 secret-loss/replacement proof row at the proxy
  and selector boundary. Installed-Codex transport e2e gates are still ahead in
  T9/T10.

## T7 Non-Blocking Refresh Black-Box Proof

Plan rows:

- Served router remains ready while background quota refresh is delayed.
- First routed HTTP and WebSocket requests use persisted SQLite quota state
  while refresh is blocked.
- `quota status` renders persisted state immediately with stale/needs-refresh
  notes.

Files changed:

- `crates/codex-router-cli/src/lib.rs`

Implemented:

- Added a blocking quota refresh provider test double that signals once it is
  inside provider fetch and waits on a release channel.
- Added HTTP served-router proof that a real loopback runtime routes
  `/v1/responses` from persisted quota state while the background refresh worker
  is blocked.
- Added WebSocket served-router proof that a real loopback runtime routes
  `/v1/responses` WebSocket from persisted quota state while the background
  refresh worker is blocked.
- Reused the existing quota status stale snapshot test for immediate persisted
  status rendering proof.

Proof:

- `cargo test -p codex-router-cli served_router_ -- --nocapture` passed:
  2 focused served-router tests.
- `cargo test -p codex-router-cli quota_status_snapshot_rows_show_unknown_pace_until_window_metadata_exists -- --nocapture`
  passed: 1 focused quota status test.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `cargo test -p codex-router-cli` passed: 62 tests.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed:
  auth 13, CLI 62, core 15, proxy 87, quota 4, secret-store 11,
  selection 21, state 18, test-support 6; 2 installed-Codex smoke tests remain
  ignored by design and are run through `tests/smoke/installed_codex_mock.sh`.

Notes:

- The proof uses bounded channel synchronization, not wall-clock sleeps, to
  prove the refresh worker is blocked while the served request succeeds.
- The test landed in `crates/codex-router-cli/src/lib.rs`, where the existing
  serve/runtime test harness lives; this is a narrow test-only expansion beyond
  the original T7 write-scope list.
