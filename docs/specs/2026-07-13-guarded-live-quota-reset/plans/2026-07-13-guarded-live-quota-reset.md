# Guarded Live Quota Reset Implementation Plan

## Goal

Add an interactive `codex-router quota reset` workflow that selects one router-owned account,
uses only live provider data to authorize redemption, selects the available reset credit with the
earliest finite expiry, and consumes it only after explicit user confirmation.

## Requirements

1. The command is interactive and reuses the quota screen's top account-list visual language.
2. `Up` and `Down` change the selected account; `Enter` starts the live eligibility check.
3. The footer documents shortcuts. `Esc`, `Ctrl-C`, and `Ctrl-R` cancel without redemption.
4. SQLite is opened read-only and supplies only account metadata plus the active credential
   generation. It never supplies usage, eligibility, credit details, or expiry data.
5. The command is natively async from the top-level dispatcher through provider I/O. It creates no
   nested runtime and calls no `block_on`.
6. No SQLite transaction, application mutex, or credential lease spans terminal input or network
   I/O. The command performs no SQLite writes.
7. Expired credentials fail closed. The reset workflow does not refresh credentials or write a new
   secret generation.
8. The command fetches live usage for the selected account and refuses unless weekly remaining is
   strictly less than 1 percent. With whole-percent provider data, only 0 percent passes.
9. Only after the live guard passes, the command fetches live reset-credit details. It keeps only
   `available` credits, accepts the provider's UTC `Z` RFC 3339 expirations, sorts finite
   expirations ascending, places non-expiring credits
   last, and selects exactly the first credit.
10. Confirmation defaults to `No`. Only explicit selection of `Yes` permits the consume request.
    After `Yes`, the command rechecks live weekly remaining immediately before consume and refuses
    if the strict guard no longer passes.
11. The consume request contains the selected credit ID and one idempotency key for the logical
    attempt. The key is reused if that attempt is retried.
12. Missing or malformed live evidence fails closed. Secrets and full credit IDs are not rendered.
13. Implementation validation must never contact a real provider endpoint. Provider behavior is
    tested only with injected fakes or loopback mock servers.

## Non-goals

- Migrating existing quota refresh, live quota, account, sessions, or serve commands to async.
- Refreshing expired OAuth credentials.
- Persisting fetched usage or reset-credit details.
- Adding `--force`, `--yes`, threshold overrides, or non-interactive redemption.
- Replacing or restarting the production router process.

## Architecture and write surfaces

```text
async CLI dispatcher
  -> quota_reset/mod.rs             orchestration and composition
       -> credentials.rs            read-only SQLx + secret read
       -> provider.rs               async live GET / GET / POST
       -> domain.rs                 pure guard and credit selection
       -> presentation.rs           iocraft account picker and confirmation
```

Likely modified files:

- `crates/codex-router-cli/src/main.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/presentation/mod.rs`
- `crates/codex-router-cli/Cargo.toml` only if an already-workspace-managed dependency is needed
- new files under `crates/codex-router-cli/src/quota_reset/`
- `crates/codex-router-secret-store/src/file_backend.rs` for a non-creating read-only opener

The existing untracked `docs/specs/2026-07-12-shared-codex-app-server-host.md` is unrelated and
must remain untouched.

## Requirements and proof matrix

| Requirement | Owning task | Proof | Layer | Freshness guard | Red/green |
| --- | --- | --- | --- | --- | --- |
| Strict live `<1%` guard | T1 | pure boundary tests for missing, 0, 1, and higher | unit | current source | required |
| Earliest-expiring available credit | T1 | pure ordering/status/malformed-expiry tests | unit | current source | required |
| Async read-only state and no refresh | T2 | source contract plus read-only fixture test; expired token refusal | integration | temporary DB/secret fixture | required |
| No consume on refusal/cancel | T3/T4 | fake provider call ledger and loopback request ledger | integration | new provider per test | required |
| Correct exact-account headers and payload | T3 | loopback HTTP assertions | integration | bound ephemeral loopback listener | required |
| Default-No interactive behavior and shortcuts | T4 | iocraft mock-terminal key tests and render assertions | unit/integration | deterministic event streams | required |
| Async CLI dispatch without nested runtime | T5 | parser/dispatcher tests and source contract | integration | current source | required |
| No real provider validation | all | all HTTP tests use injected fake or loopback URL | safety gate | command history/test configuration | n/a |

## Task sequence

### T1: Pure eligibility domain

Add failing tests first, then implement typed live usage/credit facts, strict weekly eligibility, and
earliest-expiry selection. Split if timestamp validation requires a new cross-workspace dependency.

### T2: Read-only account and credential source

Open `AsyncSqliteStateStore::open_read_only`, load enabled accounts and their active generation,
close/drop database access, and read the selected bundle through a non-creating read-only secret
store opener without invoking the refresh resolver.
Reject expired credentials. Never write SQLite or secrets.

### T3: Async provider protocol and guarded orchestration

Use async Reqwest for live usage, detailed credits, and consume. Attach the resolved bearer token
and selected account's `ChatGPT-Account-ID`. Fetch credits only after the weekly guard passes.
Expose injected traits so tests prove exact call ordering and zero POSTs on refusal.

### T4: Iocraft interaction

Reuse the quota account-list presentation language. Add a bottom workflow pane and shortcut footer.
Keep pure state transitions separate from rendering. Confirmation begins on `No`; cancellation
keys exit without invoking consumption.

### T5: Async-capable CLI integration

Make the top-level CLI entry and dispatch seam capable of awaiting the reset command. Preserve all
existing command behavior and avoid migrating unrelated synchronous commands.

### T6: Validation

Run formatting, targeted unit/integration tests, Clippy for the CLI crate, and the relevant CLI test
suite. Do not run the reset command, quota refresh, live quota, or any provider-backed smoke test.
An actual redemption smoke test is intentionally deferred to the user.

## Execution DAG

```text
repo/status validation
  -> T1 pure domain red/green
  -> T2 read-only credentials red/green
  -> T3 async provider/orchestration red/green
  -> T4 iocraft interaction red/green
  -> T5 dispatcher integration
  -> targeted validation
  -> CLI crate validation
  -> diff and safety audit
```

The work is serial because domain types, orchestration contracts, presentation outcomes, and the
dispatcher build on one another and share the same CLI crate surfaces.

## Security and recovery

- Bearer tokens remain inside redacted secret types and request headers.
- Account and credit identifiers are truncated or safely labelled in presentation and diagnostics.
- Any unknown provider status or response shape refuses redemption.
- A failed or interrupted implementation can be reverted file-by-file; there is no data migration.
- If async top-level integration forces unrelated command rewrites, stop and split the dispatcher
  foundation into a separate prerequisite rather than widening this change.

## Proof boundary

Completion means the mock-only unit and integration gates pass. A real provider E2E redemption is
not run because it would consume a scarce user credit; the user owns that final manual action when
their weekly quota expires.
