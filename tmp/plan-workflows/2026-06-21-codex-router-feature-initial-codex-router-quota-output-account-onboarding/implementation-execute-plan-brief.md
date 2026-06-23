# Implementation Execute Plan Brief: Quota Output And Account Onboarding

Date: 2026-06-21
Branch: feature/initial-codex-router
Plan: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/implementation-plan.md

## Coverage

- Implementation plan loaded fully: 429 lines.
- Execution skill loaded fully, including controller packets and validation checklist.
- Current worktree recorded before implementation: existing dirty product files plus workflow tmp artifacts.

## Current Reality

- `serve` opens state, secrets, and proxy runtime; it does not start a quota refresh worker.
- `live quota` performs provider I/O and renders directly; it does not persist quota status.
- SQLite schema is v1 with accounts and reduced selector quota snapshots.
- There is no `account` or top-level `quota` CLI namespace yet.

## Execution Mode

Initial slice is inline because T1 changes state path validation and CLI router-root derivation in shared files that later slices also need. Parallel write subagents would contend on `crates/codex-router-cli/src/lib.rs` and `crates/codex-router-state/src/sqlite.rs`. Read-only or review subagents remain useful after each meaningful slice.

## Slice 1: T1 State Path Guard And Router Root Helpers

Write scope:

- `crates/codex-router-state/src/*`
- `crates/codex-router-cli/src/lib.rs` or a small helper module
- state and CLI tests

Required behavior:

- SQLite open rejects `.codex` path components before migration writes.
- SQLite open rejects symlink path components before migration writes.
- A single router-root helper derives `<router-root>/state.sqlite` and `<router-root>/secrets`.
- `serve` uses `--router-root`; independent `--state-db` and `--secret-root` are removed from the main command surface for this slice.
- Existing router token commands use the same secret-root derivation.

Proof:

- State unit tests for allowed temp path, `.codex` path, and symlink parent.
- CLI parser/command tests prove token and serve derive state/secrets from the same router root.
- Focused package tests: `cargo test -p codex-router-state` and targeted `cargo test -p codex-router-cli`.

## Open Proof Gaps

- Background periodic SQLite persistence is planned in T8 after T2/T3/T5 provide the durable status schema, credential import model, and refresh service.
- Live OAuth proof remains `not-run: approval required` unless explicitly approved.

## Slice 1 Proof

- Red gate: `cargo test -p codex-router-state` failed before implementation because `StateStoreError::CodexHomePath` and `StateStoreError::SymlinkPath` did not exist.
- Green gate: `cargo test -p codex-router-state` passed, 9 tests.
- Green gate: `cargo test -p codex-router-cli` passed, 28 tests.
- Implemented state path guard for `.codex` and symlink path components before SQLite open/migration.
- Implemented shared CLI router-root derivation: `<router-root>/state.sqlite` and `<router-root>/secrets`.
- Updated `serve` and router token commands to use the shared router-root derivation.

## Slice 2 Proof

- Implemented SQLite schema v2 with adjacent `quota_status_rows` table keyed by `(account_id, route_band, family, window_label)`.
- Added `PersistedQuotaStatusRow` and `QuotaStatusRepository` for local-only quota status output.
- Added atomic selector/status replacement through `replace_route_quota_state`.
- Green gate: `cargo test -p codex-router-state` passed, 13 tests.
- Green gate: `cargo test -p codex-router-cli` passed, 28 tests.
- Green gate: `cargo test -p codex-router-proxy` passed, 40 tests.
- Formatting gate: `cargo fmt --all` passed.

## Slice 3 And 4 Proof

- Added router-owned credential import parser separate from `live_quota`.
- Parser accepts access-token-only auth, access+refresh+expiry auth, rejects API-key auth, and redacts token canaries on malformed JSON.
- Added access-token and refresh-token secret key conventions.
- Added `account import-codex-auth --router-root --label --auth-json --allow-plaintext-file-secrets`.
- Import rejects email-like labels, requires explicit plaintext file-backend acknowledgement, leaves source `auth.json` unchanged, writes SQLite account metadata, writes access/refresh secrets under `<router-root>/secrets`, and stores non-secret refresh/expiry metadata in SQLite.
- Green gate: `cargo test -p codex-router-auth` passed, 14 tests.
- Green gate: `cargo test -p codex-router-secret-store` passed, 9 tests.
- Green gate after metadata wiring: `cargo test -p codex-router-state` passed, 13 tests.
- Green gate after account CLI wiring: `cargo test -p codex-router-cli` passed, 32 tests.

## Slice 6A And Quota Wording Proof

- Added `account list`, `account enable`, and `account disable`.
- `account list` reports id, label, lifecycle status, refresh-token presence, and expiry without printing secrets.
- Updated compatibility `live quota --format table` pace wording from `ahead`/`behind`/`on pace` to `burn +N%`/`save N%`/`steady`.
- Green gate: `cargo test -p codex-router-cli` passed, 33 tests.

## Slice 5 And 7 Partial Proof

- Added `quota status --router-root [--all-limits]` as a local-only SQLite renderer over persisted quota status rows.
- Added `quota refresh --router-root [--account <id-or-label>] [--base-url <url>]` as an explicit provider-I/O command.
- Manual refresh reads router-owned access tokens from `<router-root>/secrets`, fetches usage with a finite request timeout, normalizes selector snapshots plus detailed status rows, and commits each route atomically through state.
- Account-specific credential/provider refresh failures persist a redacted failed status row and do not stop healthy accounts from refreshing.
- `quota status` renders compact effective rows by default and detailed rows with `--all-limits`; default columns are `Account | Route | Status | Headroom | Window | Reset | Pace | Runout | Notes`.
- Provider additional labels are shown in Notes without adding a second compact table shape.
- Green gate: `cargo test -p codex-router-cli` passed, 35 tests.
- Green gate: `cargo clippy --workspace --all-targets -- -D warnings` passed.
- Later slice wired serve-owned background periodic refresh.

## Slice 8 Proof

- Wired `serve` to own a background quota refresh worker handle.
- Added serve options:
  - `--quota-refresh-base-url <url>`
  - `--quota-refresh-interval-seconds <seconds>`, default `300`
  - `--quota-refresh-timeout-seconds <seconds>`, default `30`
- The worker uses the same `quota::refresh_quota_state` path as manual `quota refresh`, writes SQLite selector/status rows, and is stopped/joined on drop.
- `--quota-refresh-interval-seconds 0` is a test seam that runs one immediate refresh and returns.
- Added `serve_command_runs_background_quota_refresh_without_request_path_quota_io`.
- The serve test seeds a router account, starts separate upstream and quota mock servers, serves one normal request, verifies upstream traffic uses the upstream token, verifies the quota mock is called by the background worker, and verifies SQLite headroom changes from the seeded value to the refreshed value.
- Removed stale duplicate background refresh code from `quota.rs` after `cargo nextest run --workspace` exposed it.

## Docs Proof

- Updated `README.md` with the current router-root setup flow:
  - `token init`
  - `account import-codex-auth`
  - `account list`
  - `quota refresh`
  - `quota status`
  - `serve`
- Updated `docs/testing/live-oauth-quota.md` to replace stale `--state-db`/`--secret-root` examples with `--router-root`.
- The docs now state plainly that `account import-codex-auth` is the implemented OAuth onboarding path and that there is not yet an interactive `account login` browser/device flow.
- The docs identify the file secret backend as plaintext at rest under private filesystem permissions and keep live quota execution approval-gated.

## Review Fixes Proof

- Provider quota fetches now fail closed to `https://chatgpt.com/backend-api` by default. Loopback/mock quota endpoints require explicit `--allow-insecure-quota-base-url` on `quota refresh`, `serve`, or `live quota`.
- Provider/credential terminal failures replace selector snapshots with failed zero-headroom rows instead of leaving stale positive snapshots eligible.
- Provider 200 responses with missing usable windows and past-reset/malformed quota windows fail closed and persist zero-headroom failed state.
- State and secret-store roots reject `.codex` and `.prodex` path components.
- `account list` and `quota status` open existing SQLite read-only and do not create a missing state database.
- `quota status --format plain` is implemented; `--format json` is explicitly rejected until there is a designed schema.
- Enabled imported accounts with no quota rows render as `unknown` instead of disappearing.
- `live quota` now adapts into the shared quota renderer, so live and persisted quota output use the same table/plain vocabulary.
- Serve routing uses a dynamic quota clock by default, while tests can pass `--now-unix-seconds` for a fixed clock. The background refresh worker uses the same fixed/dynamic clock choice.
- Review swarm fixes added response-quota route-band fan-out for `models`, `memories_trace_summarize`, and `responses_compact`, including failure replacement so stale aliases are cleared.
- Review swarm fixes added percent-encoded `quota status --format plain` values, local-safe account label validation, display sanitization, duplicate-label import rejection, live quota `profile-N` aliases, and pre-listen quota refresh endpoint validation.
- Focused regression gate after review fixes: `cargo test -p codex-router-cli` passed, 47 tests.

## Current Validation Proof

- `cargo fmt --all -- --check`: passed.
- `cargo test -p codex-router-state`: passed, 14 tests.
- `cargo test -p codex-router-secret-store`: passed, 10 tests.
- `cargo test -p codex-router-auth`: passed, 17 tests.
- `cargo test -p codex-router-cli`: passed, 47 tests.
- `cargo test -p codex-router-proxy`: passed, 40 tests.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo nextest run --workspace`: passed, 155 tests passed, 2 skipped.
- `cargo deny check`: passed; duplicate dependency warnings only for `getrandom` and `windows-sys`.
- `cargo audit`: passed; scanned 199 dependencies.
- `actionlint .github/workflows/ci.yml`: passed.
- `tests/smoke/installed_codex_mock.sh`: passed, 2 tests.

## Current Live Gate

- `live_oauth_quota_gate`: not-run.
- Reason: approval required for this changed router-owned import/refresh/background-refresh revision.
- Previous 2026-06-20 live quota proof was for the narrower compatibility `live quota` surface before these changes and is not being reused as proof for router-owned import/refresh.
