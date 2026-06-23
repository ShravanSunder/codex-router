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
