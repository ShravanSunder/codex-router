# Implementation Plan: Quota Output And Account Onboarding

Date: 2026-06-21
Status: draft for plan review
Goal id: 2026-06-21-quota-output-account-onboarding

## Source Coverage

- Spec: `quota-output-account-onboarding-spec.md`, 419 lines, read fully.
- Spec review report: `spec-review-report.md`, 92 lines, read fully.
- Workflow log: `events.jsonl`, 2 events, current workflow advanced to plan creation.

Key repo evidence:

- CLI dispatch currently exposes `serve`, `profile`, `token`, `live`, no `account` or top-level `quota`.
- `live quota` fetches from `auth.json` and renders directly; it does not persist.
- SQLite currently has reduced `accounts` and `quota_snapshots` tables only.
- `SecretStore` currently has `write_secret` and `read_secret` only.
- Runtime opens state/secrets/listener; no periodic quota worker is wired into `serve`.
- Proxy selector reads local accounts, secrets, and route-band snapshots at request time.

## Goal

Implement router-owned account import/lifecycle and quota status/refresh UX, with local-only quota status backed by SQLite, periodic background quota refresh during `serve`, and compact/detailed quota table rendering that no longer uses `ahead`/`behind`.

## Non-Goals

- Do not implement full browser/device OAuth login in this slice; `account login` is absent or fail-closed.
- Do not implement `account logout` until delete-capable secret storage exists.
- Do not silently write `~/.codex`.
- Do not run live OAuth/quota proof without explicit approval.
- Do not implement JSON output unless a positive schema proof is added before execution.
- Do not implement 1Password storage.
- Do not implement macOS Keychain in this slice. This plan chooses the existing hardened file backend as an explicit plaintext-at-rest fallback, gated by CLI disclosure and acknowledgement. A Keychain-backed default remains a follow-up storage-backend slice before claiming encrypted normal-use OAuth storage.

## Plan Review Decisions Locked

1. Storage backend: this slice uses the existing `FileSecretStore` for upstream OAuth account material only as an explicit fallback. Account import and docs/help must say OAuth material is plaintext-at-rest under private filesystem permissions. Import of OAuth material requires an explicit `--allow-plaintext-file-secrets` acknowledgement.
2. Canonical router root: new account/quota/serve flows use `--router-root <path>`. State DB is `<router-root>/state.sqlite`; file secrets are `<router-root>/secrets`. Existing independent `--state-db`/`--secret-root` serve wiring is replaced in this slice rather than extended.
3. SQLite migration: add an explicit v2 migration from current v1. Do not edit v1 in place.
4. Refresh failure state: store it in the persisted quota status rows for this slice. A separate account-health table is out of scope.
5. Background refresh cadence: `serve` exposes `--quota-refresh-interval-seconds <seconds>` with default `300`, plus provider request timeout `--quota-refresh-timeout-seconds <seconds>` with default `30`.
6. Worker ownership: CLI `serve` constructs and owns the auth-backed quota worker handle. Proxy/runtime selection remains auth-agnostic and provider-agnostic.

## Requirements / Proof Matrix

| Req | Source | Owner Task | Proof Layer | Red/Green | Stale-Proof Guard |
| --- | --- | --- | --- | --- | --- |
| R1 parser accepts supported OAuth and rejects API-key auth | spec R1 | T3 | unit | yes | token/API-key/email canaries absent from parser errors |
| R1 import success writes SQLite metadata and secret-store token material without mutating source auth.json | spec R1 | T4 | integration/CLI | yes | fixture records source mtime/content before and after import |
| R1 API-key import rejection redacts key | spec R1 | T4 | CLI | yes | canary `sk-local-secret-canary` absent from stdout/stderr |
| R2 login reserved/fail-closed | spec R2, locked decision 1 | T6 | CLI | yes | help/parser asserts no fake login path or clear fail-closed message |
| R3 list/enable/disable lifecycle | spec R3 | T6 | CLI/integration | yes | enabled/disabled/missing-token/ambiguous-label fixtures |
| R3 logout reserved until delete support | spec R3, locked decision 2 | T6 | CLI negative | yes | `account logout` absent or delete-not-supported; secret remains intentionally untouched |
| R4 manual quota refresh persists selector and status rows | spec R4 | T5 | integration/CLI | yes | mock provider, temp SQLite, redacted failure fixture |
| R4 status reads seeded SQLite only | spec R4 | T7 | CLI integration | yes | provider mock panics/fails if touched |
| R4 persisted quota status schema supports compact and detailed rows | persisted quota schema | T2/T5/T7 | state integration | yes | seeded rows render both views without provider calls |
| R4 selector snapshot and status rows commit atomically | security review | T5 | state integration | yes | injected fault preserves previous good state or commits both surfaces |
| R4A serve schedules periodic background refresh | spec R4A | T8 | runtime integration | yes | fake scheduler/provider records schedule/fetch; bounded event wait, no sleep-only proof |
| R4A request path performs zero provider quota calls | spec R4A | T8 | protocol/integration | yes | provider call counter remains zero during request selection |
| R5 compact table includes route band and ASCII headroom | spec R5 | T7 | unit/CLI | yes | deterministic `now_unix_seconds` fixture |
| R6 detailed table expands families/windows | spec R6 | T7 | unit/CLI | yes | `--all-limits` fixture includes main, code review, provider additional |
| R7 pace wording avoids `ahead`/`behind` | spec R7 | T7 | unit/CLI | yes | negative assertions for both words |
| R8 plain output remains redacted; JSON out of scope | spec R8, locked decision 6 | T7 | CLI | yes | help/tests reject or omit json unless schema added |
| R9 storage disclosure and SQLite path guard | spec R9, security contract | T1/T4/T6/T9 | unit/CLI/docs | yes | `.codex`/symlink/out-of-root fixtures; help names plaintext file-backend fallback and import requires acknowledgement |
| R10 live proof gate | spec R10 | T10 | docs/proof | no for live unless approved | final report says `not-run: approval required` without approval |

## Task Sequence

### T0. Reconfirm Baseline And Protect Existing Work

Write scope: none.

- Record current `git status --short`.
- Record current baseline only; behavior proof starts with the first task-specific red tests.
- Confirm untracked `tmp/` artifacts remain separate from product code.

Proof:

- `git status --short`
- note existing dirty files and do not revert unrelated work.

### T1. State Path Guard And Router Root Helpers

Write scope:

- `crates/codex-router-state/src/*`
- `crates/codex-router-cli/src/lib.rs` or small helper module
- state tests

Changes:

- Add SQLite path validation before migration writes: reject symlink path components and `.codex` paths.
- Add one CLI router-root helper that derives:
  - state DB: `<router-root>/state.sqlite`
  - file secret root: `<router-root>/secrets`
- Hard-cut `serve` to `serve --router-root <path> --upstream-base-url <url>` for this slice. Account/quota commands use the same root helper. Independent `--state-db`/`--secret-root` support is removed from the main command surface instead of extended.

Proof:

- state unit tests for `.codex`, symlink, and allowed temp path.
- CLI parser/command tests proving account import, quota refresh/status, token, and serve derive the same state/secret paths from one router root.
- Serve rejects symlinked or `.codex` router roots before opening SQLite.

### T2. Persisted Quota Status Schema

Write scope:

- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/repositories.rs`
- state tests

Changes:

- Add quota status DTO/repository for normalized rows keyed by account id, route band, family, and window.
- Store observed timestamp, used percent, remaining percent/headroom, reset, window label/source, effective marker, stale/error/failure status, and redacted failure metadata.
- Preserve existing reduced selector snapshot table for request-time routing.
- Add explicit v2 migration from current v1. `CURRENT_SCHEMA_VERSION` becomes 2, v1 databases migrate to v2, and unsupported newer schemas still fail closed.
- Add a transaction API or repository method that writes selector snapshots and status rows atomically for one account/route-band refresh result.

Proof:

- migration/open tests, including a real v1 database upgraded to v2.
- upsert/load/list status row tests.
- malformed/oversized percentage normalization tests.
- injected-fault test proving selector snapshot and status rows commit together or preserve the previous good state.

### T3. Router-Owned Credential Import Model

Write scope:

- `crates/codex-router-auth/src/*`
- `crates/codex-router-secret-store/src/account_tokens.rs`
- auth/secret tests

Changes:

- Add structured router credential import model separate from access-token-only live quota parser.
- Parse supported Codex/Prodex `auth.json` token shape for access token, plus refresh token and expiry when those fields are present.
- Generate opaque non-PII account ids.
- Add secret key conventions for access token and optional refresh token only.
- Store non-secret expiry/refresh-health metadata in SQLite-owned account/quota status state, not in the secret store.
- Keep provider quota normalization out of T3; T5 owns the single quota normalization path.

Proof:

- auth parser fixtures for access-token-only, refresh-token-plus-expiry, API-key rejection, and malformed JSON.
- canary tests for raw email not becoming id/default label.
- secret key tests.
- state-only test proving account/list health metadata can be read without secret-store reads.

### T4. Account Import Service And CLI Namespace

Write scope:

- new `crates/codex-router-cli/src/account.rs`
- `crates/codex-router-cli/src/lib.rs`
- CLI tests

Changes:

- Add `CliCommand::Account`.
- Implement `account import-codex-auth --router-root --label --auth-json --allow-plaintext-file-secrets`.
- Write account metadata and credential secrets through state/secret boundaries.
- Print only redacted account id/label/status/import result.
- Reject email-like labels instead of treating labels as safe-to-print PII. The CLI should tell the user to choose a non-email local label.
- Partial-failure contract:
  - create or reuse a disabled account row first
  - write/verify secrets second
  - mark the account enabled only after secret writes succeed
  - retries with the same non-PII label repair disabled partial imports instead of duplicating accounts
  - if DB creation fails, no secret write occurs

Proof:

- import success writes SQLite and secret store.
- source `auth.json` unchanged.
- API-key rejection redacts key.
- raw email/token canaries absent.
- fixture with access token, refresh token, and expiry persists all expected secret/state fields.
- injected secret-write and SQLite-write failure tests; retry repairs partial import without duplicate drift.
- import without `--allow-plaintext-file-secrets` fails with plaintext-at-rest warning.

### T5. Quota Refresh Service

Write scope:

- `crates/codex-router-auth/src/live_quota.rs` or new auth quota adapter
- `crates/codex-router-quota/src/*`
- `crates/codex-router-state/src/*`
- new `crates/codex-router-cli/src/quota.rs`
- tests

Changes:

- Add service that reads router-owned account credentials, fetches provider quota, normalizes windows/families, and persists both selector snapshots and detailed status rows.
- Manual CLI: `quota refresh --router-root --state-db [--account] [--base-url]`.
- Persist redacted per-account refresh failure state.
- Depend on T2 status repositories and T3 token-key conventions.
- Use one quota normalization path for both selector snapshots and quota status rows.
- Normalization rules:
  - `used_percent` must be in `0..=100`; out-of-range values make that window invalid and prevent positive routing headroom from that window
  - missing or non-positive `limit_window_seconds` makes pace/runout unknown and the window invalid for bottleneck selection
  - past reset timestamps render as elapsed/stale and do not create fresh positive headroom
  - provider labels are sanitized and bounded before persistence/output
  - malformed provider families fail closed into redacted refresh failure status
- Persist selector snapshot and status rows in one SQLite transaction per account/route-band refresh.
- Provider calls use finite request deadlines from the configured quota refresh timeout.

Proof:

- mock provider refresh persists `responses` selector row plus detailed status rows for main, code-review, and additional provider families.
- `quota refresh` followed by `quota status --all-limits` renders the DB produced by the refresh, not hand-seeded status rows only.
- failure persists structured redacted status and does not store or print token/auth-header/email canaries.
- malformed provider quota fixtures cover negative, >100, missing, and past-reset values and fail closed.
- injected fault between selector snapshot and status-row writes preserves previous good state or commits both.

### T6. Account Lifecycle, Derived Status, And Reserved Commands

Write scope:

- `crates/codex-router-cli/src/account.rs`
- state repository helpers for account lookup by id/label and status update
- CLI tests

Changes:

- T6A, after T4: implement `account list`, `account enable`, `account disable` for account metadata and selector eligibility.
- T6B, after T2/T5: enrich `account list` with derived refresh/quota health from persisted quota status rows.
- Resolve account by id or label; reject ambiguous labels.
- Reserve `account login` as absent or fail-closed with import pointer.
- Reserve `account logout` as absent or fail-closed until `delete_secret`.
- Surface storage backend limitation if file backend is used.
- Never print email-like labels; reject them at import and include canary tests for stored labels.

Proof:

- list shows enabled/disabled/missing-token/refresh-limited status without secrets.
- enable/disable changes selector eligibility.
- ambiguous label fails.
- login/logout reserved behavior.
- shared lookup/status tests prove the same persisted status source feeds both `quota status` and `account list`.

### T7. Quota Status Renderer And Table UX

Write scope:

- new renderer module, likely `crates/codex-router-cli/src/quota_render.rs`
- `crates/codex-router-cli/src/live.rs`
- `crates/codex-router-cli/src/quota.rs`
- CLI tests

Changes:

- Extract table rendering from `live.rs` into a normalized row renderer.
- Define one CLI read model, `QuotaStatusRow`, as the sole renderer input. T2 stores/loads it, T5 writes it, T7 renders it. Compatibility `live quota` adapts provider responses into `QuotaStatusRow`; it must not keep a second table renderer.
- Default compact table columns: `Account | Route | Status | Headroom | Window | Reset | Pace | Runout | Notes`.
- Detailed `--all-limits` table expands families/windows.
- Replace `ahead`/`behind` with `steady`, `burn +N%`, `save N%`, `unknown`.
- Add ASCII headroom bar.
- Keep `live quota` compatibility path but label/document it as compatibility proof, using the same renderer model.

Proof:

- deterministic renderer unit tests with injected `now_unix_seconds`.
- CLI `quota status` from seeded SQLite with provider mock untouched.
- `live quota --format table --all-limits` updated assertions.
- negative token/raw JSON/email assertions.
- golden test proves seeded-SQLite `quota status` and a compatibility live adapter render identical compact/detailed tables for the same synthetic quota data.

### T8. Serve Background Quota Worker

Write scope:

- `crates/codex-router-quota/src/worker.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-cli/src/lib.rs`
- runtime tests

Changes:

- CLI `serve` owns the auth-backed quota worker handle. Proxy runtime remains auth-agnostic and provider-agnostic.
- Worker scans enabled accounts, refreshes on configurable interval, persists selector/status rows and redacted failure state.
- Add CLI options:
  - `--quota-refresh-interval-seconds <seconds>`, default `300`
  - `--quota-refresh-timeout-seconds <seconds>`, default `30`
- Use a joinable blocking-thread worker with:
  - cooperative stop flag
  - bounded provider request timeout
  - bounded test tick source / paused scheduler seam
  - per-account failure isolation
  - panic/error reporting that does not contain secrets
- Keep proxy selector read-only against local state and secrets; no provider quota calls.

Proof:

- serve startup schedules refresh without inline provider I/O.
- periodic refresh uses bounded event waits.
- runtime integration proves startup schedules without fetch, request handling keeps provider call counter at 0 while the worker is paused, then a driven tick increments the counter exactly once.
- healthy accounts continue when one account refresh fails.
- runtime drop/serve exit stops and joins the worker; no refresh runs after shutdown.
- mock transport that never returns proves timeout/failure handling does not hang shutdown or unrelated accounts.
- compile-time dependency check proves `codex-router-proxy` does not depend on `codex-router-auth`.

### T9. Docs And Runbook Updates

Write scope:

- `docs/testing/live-oauth-quota.md`
- README quick usage section
- plan/review artifacts if promoted later

Changes:

- Document actual account import, quota refresh/status, serve background behavior, and compatibility `live quota`.
- Keep live proof approval boundary.
- State file-backend plaintext-at-rest fallback clearly. Account import examples include `--allow-plaintext-file-secrets`; docs must not describe file backend as encrypted or normal secure storage.
- Document `serve --router-root`, `--quota-refresh-interval-seconds`, and `--quota-refresh-timeout-seconds`.

Proof:

- docs mention no nonexistent `codex-router login` as implemented unless command is fail-closed/reserved.
- exact command examples match help.

### T10. Validation And Lifecycle Closeout

Write scope:

- no product edits unless validation finds in-scope defects.

Commands:

- targeted red/green package tests before workspace gates:
  - `cargo test -p codex-router-state`
  - `cargo test -p codex-router-secret-store`
  - `cargo test -p codex-router-auth`
  - `cargo test -p codex-router-quota`
  - `cargo test -p codex-router-cli`
  - `cargo test -p codex-router-proxy`
- authoritative CI-aligned gates:
  - `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace`
  - `cargo deny check`
  - `cargo audit`
  - `actionlint .github/workflows/ci.yml`
- live gate: `not-run: approval required` unless explicit approval is given.

Then route to `shravan-dev-workflow:implementation-review-swarm`.

## Execution DAG

```text
gate 0: validate repo state and source artifacts
  |
  +-- lane A: state/path/schema/read model
  |     T1 -> T2
  |
  +-- lane B: auth/secret/account import
  |     T3 -> T4 -> T6A
  |
  +-- lane C: renderer shape
  |     waits for T2 read model -> T7
  |
  +-- lane D: quota normalization/refresh
  |     waits for T2/T3 -> T5
  |
  +-- lane E: serve background worker
        waits for T2/T3/T5 contracts -> T8
  |
  +-- lane F: account derived status
        waits for T2/T5 -> T6B

integration gate:
  parent verifies one QuotaStatusRow/read-model contract across T2/T5/T6/T7/T8
  |
targeted validation:
  state + auth + cli + quota + proxy focused tests
  |
full relevant validation:
  cargo test/clippy/security gates
  |
implementation-review-swarm
  |
implementation-pr-wrapup
```

Parallelization note: T1/T2 and T3 can start independently after plan review. T5, T6B, T7, and T8 wait on the shared state/read-model contracts. Implementation should be integrated by one owner or tightly coordinated because CLI tests, DTO names, and persistence semantics cross lanes.

## Write Surfaces

- `crates/codex-router-cli/src/lib.rs`
- new `crates/codex-router-cli/src/account.rs`
- new `crates/codex-router-cli/src/quota.rs`
- new or refactored quota renderer module
- `crates/codex-router-auth/src/live_quota.rs` and/or new credential/quota modules
- `crates/codex-router-secret-store/src/account_tokens.rs`
- `crates/codex-router-state/src/account.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-quota/src/worker.rs`
- `crates/codex-router-proxy/src/server.rs`
- `docs/testing/live-oauth-quota.md`
- README if usage is promoted

## Risks And Split Triggers

- If supported `auth.json` lacks refresh token/expiry, split durable refresh from import and make access-token-only limitations visible.
- If v2 migration is larger than expected, split T2 into migration/read-model first and status rendering later. Do not edit v1 in place.
- If serve worker lifecycle is too invasive, first land manual `quota refresh` and local-only `quota status`, then re-plan T8; do not claim runtime background refresh until wired.
- If reviewers reject the explicit plaintext file-backend fallback, split Keychain storage into its own reviewed plan before account import.
- If table renderer cannot share live and persisted rows cleanly, stop and re-plan the renderer/read-model boundary. Do not keep a second `live quota` table renderer while claiming unified quota UX.

## Closed Plan-Review Questions

1. Storage backend: explicit plaintext file-backend fallback with `--allow-plaintext-file-secrets`; Keychain is a follow-up.
2. Refresh interval: `--quota-refresh-interval-seconds`, default `300`.
3. Refresh failure status: quota status rows only; no separate account-health table in this slice.

## Recommended Next Workflow

`shravan-dev-workflow:plan-review-swarm`

phase_result: complete
evidence: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/implementation-plan.md
recommended_next_workflow: shravan-dev-workflow:plan-review-swarm
recommended_transition_reason: Draft implementation plan exists and needs adversarial plan review before code execution.
