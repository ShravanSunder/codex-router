# Quota Output And Account Onboarding Spec

Date: 2026-06-21
Status: draft for spec review
Goal id: 2026-06-21-quota-output-account-onboarding

## Product Intent

codex-router should be usable by a local Codex user without depending on an existing Prodex/Codex profile folder as the runtime credential source. The user should be able to add accounts, see which accounts are usable, refresh quota, understand pacing at a glance, and then point Codex at the router while Codex keeps owning sessions and transport behavior.

Success looks like:

- a user can onboard at least one OAuth account into router-owned storage
- a user can inspect accounts without exposing credentials or raw emails
- a user can refresh quota into router-owned state
- the default quota view answers "which account should I use and how much runway do I have?"
- detailed all-window quota remains available for debugging
- live execution is approval-gated and redacted

## Non-Goals

- Do not make Codex sessions, history, retries, timeouts, or provider transport state router-owned.
- Do not silently write to `~/.codex`.
- Do not use Codex/Prodex `auth.json` as the router runtime credential database.
- Do not print OAuth refresh tokens, access tokens, raw auth headers, raw auth JSON, raw account emails, prompts, request bodies, or response bodies.
- Do not implement 1Password storage in this slice unless explicitly selected later.
- Do not run live OAuth/quota/model-traffic proof without explicit approval.

## Current State

The current CLI has `serve`, `token`, `profile`, and `live` commands, but no top-level `account` or router-owned `quota` command. `live quota` reads `auth.json` files directly and calls the ChatGPT quota endpoint as a compatibility proof path.

Router-owned account metadata and quota snapshot infrastructure already exist:

- Account metadata: `AccountRecord { account_id, label, status }`
- Quota snapshots: `PersistedQuotaSnapshot { account_id, source, observed_unix_seconds, route_band, remaining_headroom, reset_unix_seconds, stale_penalty }`
- Secret-store trait: `write_secret` and `read_secret`
- Secret key convention: upstream access token only today

The existing table rendering already computes bottleneck/effective windows and duration labels, but the output is too diagnostic by default and still uses `ahead`/`behind` pace language.

## Requirements

R1. Router-owned account onboarding

The CLI must provide a top-level account surface for importing existing Codex/Prodex OAuth material into router-owned account state.

Required command shape:

```text
codex-router account import-codex-auth --router-root <path> --state-db <path> --label <label> --auth-json <path>
```

The import command must:

- accept only quota-compatible OAuth auth material
- reject API-key auth without printing API keys
- write non-secret account metadata to SQLite
- write token material through the secret-store boundary
- avoid writing to `~/.codex` or the source profile
- print only account label/id/status and redacted import result

R2. Login UX contract

The intended command surface must reserve:

```text
codex-router account login --router-root <path> --state-db <path> --label <label>
```

If full browser/device OAuth cannot be safely implemented in this slice, the command must fail closed with a clear message pointing to `account import-codex-auth` as the implemented onboarding path. It must not fake a login or silently read Codex home state.

R3. Account lifecycle UX

The CLI must support:

```text
codex-router account list --router-root <path> --state-db <path>
codex-router account enable --state-db <path> --account <id-or-label>
codex-router account disable --state-db <path> --account <id-or-label>
```

Lifecycle commands must preserve secret redaction. Disable must make an account ineligible without deleting metadata.

Full `account logout` is reserved until the secret-store trait/backend has explicit `delete_secret` support. The implementation must not ship a logout command that only overwrites blank secrets or leaves token material readable.

R4. Router-owned quota commands

The CLI must support router-owned quota operations:

```text
codex-router quota refresh --router-root <path> --state-db <path> [--account <id-or-label>] [--base-url <url>]
codex-router quota status --router-root <path> --state-db <path> [--format table|plain|json] [--all-limits]
```

`quota refresh` must read credentials from router-owned secret storage, fetch quota through the auth/quota boundary, and persist snapshots. `quota status` must render persisted/router-owned state by default. Compatibility live quota over `auth.json` may remain under `live quota`, but it must not be the normal status path.

R4A. Background quota refresh invariant

Normal serving must not block request-time routing on broad provider quota refresh. The runtime path must read existing SQLite snapshots, apply freshness/staleness policy, and select from local state. Provider quota fetches must run through scheduled background refresh work that periodically updates SQLite snapshots.

A request may trigger only a narrow account-scoped refresh when it is bounded and outside the committed upstream response path. It must not turn the accept loop or account selector into a live quota polling path.

The runtime contract must include:

- startup reads existing snapshots immediately and schedules refresh work without inline provider I/O
- periodic all-enabled-account refresh with a configurable interval
- per-account refresh failure state that is visible in status output and does not stop healthy accounts
- deterministic proof that request-time selection does not call the provider

R5. Compact quota table UX

Default table output must be optimized for decisions, not debugging. It should show one row per `(account, route band)` decision surface, not every raw provider window.

Recommended default columns:

```text
Account | Route | Status | Headroom | Window | Reset | Pace | Runout | Notes
```

Default semantics:

- `Headroom` is the effective bottleneck remaining percentage.
- `Route` is the persisted route band or quota family being summarized, such as `responses`.
- `Window` is the bottleneck window label such as `5h`, `daily`, `weekly`, or `monthly`.
- `Reset`, `Pace`, and `Runout` inherit from the bottleneck window.
- `Pace` must not use `ahead`/`behind`.
- Use a small ASCII bar for headroom, especially when the bottleneck is weekly.
- Missing/stale/error states must render as rows, not panics.

R6. Detailed quota table UX

Detailed output remains available with `--all-limits`. It may show all windows and provider families, but must still use stable labels and redacted values.

Detailed columns may include:

```text
Account | Status | Family | Window | Used | Left | Reset | Pace | Runout
```

The detailed table should avoid raw `limit_window_seconds` as a primary column; duration labels are the human-facing default, and raw seconds belong only in a debug/plain format if needed.

R7. Pace and runout wording

Pace must be phrased as burn-rate signal rather than blame language.

Allowed examples:

- `steady`
- `burn +25%`
- `save 12%`
- `unknown`

Runout may remain:

- `after reset`
- `in 2h 8m`
- `unknown`

R8. Plain and machine-readable output

Plain output must remain stable for smoke/debug use and redacted. If JSON is added, it must be schema-positive and exclude token/auth/raw response fields.

R9. Secret-store backend boundary

The design target remains OS keyring/macOS Keychain as the default real backend. The current hardened file backend may be used for tests, CI, deterministic local development, and explicit fallback, but docs and CLI help must not describe it as encrypted storage.

If a keychain backend cannot land in this implementation slice, the CLI must make the backend choice explicit and document the limitation. The implementation plan must decide this before code changes.

R10. Live proof boundary

Live OAuth/quota proof must remain explicit and approval-gated. Without approval, final proof reports:

```text
live_oauth_quota_gate: not-run
reason: approval required
```

## Technical Contract

### Account source of truth

SQLite owns non-secret account metadata. Secret-store owns token material. The router runtime reads account metadata and token material from router-owned stores.

`auth.json` is only an import/proof input. It is not a watched file, runtime source of truth, or fallback lookup path.

### Account identity

Account ids must be stable, opaque, non-PII, non-secret, and suitable for SQLite primary keys and secret-store key suffixes. Account ids must not be raw emails or directly derived from raw email/account fields. Labels are human-facing and may be mutable, but defaults must not copy raw email from imported source material. Commands that accept `<id-or-label>` must reject ambiguous labels with a clear error.

### Credential shape

The current code only models access-token extraction for quota proof. Real router-owned onboarding needs a credential shape that can eventually support refresh:

```text
account_id
access_token
refresh_token optional until parser support confirms source shape
expires_at optional until parser support confirms source shape
```

If imported `auth.json` lacks refresh material, imported accounts may be usable only until access token expiry. The CLI must say this plainly and account status should make refresh limitations visible.

The import parser must not reuse the current access-token-only live quota parser as the whole credential model. It may share validation helpers, but durable account onboarding needs a structured router credential model with key conventions for access token, refresh token, and expiry metadata.

### Quota source of truth

Quota status defaults to persisted router-owned snapshots. Background refresh periodically updates those SQLite snapshots from live provider responses. Manual `quota refresh` is an explicit user command for immediate refresh, not the normal per-request mechanism. `live quota --auth-json/--profiles-root` remains a read-only compatibility probe and should be clearly labeled as such in help/docs.

Request-time selection must consume persisted snapshots and classify them as fresh, stale-with-penalty, or unknown. When known healthy accounts exist, unknown quota must not be treated as free capacity.

### Persisted quota status schema

The current reduced route-band snapshot is sufficient for selector headroom, but not sufficient for local-only human quota status. This slice must either extend SQLite with richer quota status rows or add an adjacent table keyed by `(account_id, route_band, family, window)`.

Persisted quota status must include enough normalized data to render both compact and detailed output without provider I/O:

- account id
- route band
- quota family, such as `rate_limit`, `code_review`, or provider additional label
- window label, such as `5h`, `daily`, `weekly`, `monthly`, or `effective`
- observed Unix timestamp
- used percent when available
- remaining/headroom percent
- reset Unix timestamp when available
- limit window seconds or normalized label source
- bottleneck/effective marker
- pace/runout inputs or deterministic computed values
- freshness/staleness status
- redacted refresh failure status and last failure timestamp

Selector-facing reduced snapshots may remain optimized for routing, but `quota status` must not reconstruct detailed rows by calling the provider.

### Quota refresh runtime owner

`serve` owns the background quota refresh worker lifecycle. Startup must create a worker that scans enabled accounts, fetches through the auth/quota boundary, writes quota status and selector snapshots to SQLite, records redacted per-account failure status, sleeps on a configurable interval, and stops cleanly when serving stops.

The proxy selector owns request-time selection only. It reads SQLite snapshots and secret-store tokens; it must not schedule refresh work or call provider quota endpoints.

### Table rendering

The quota renderer must be factored away from direct live-fetch plumbing so both router-owned `quota status` and compatibility `live quota` can share formatting. The renderer contract takes normalized quota rows plus an injected `now_unix_seconds`; it must not call the provider, read files, or read SQLite itself.

Tests must cover at least:

- compact/default table as deterministic rendered output
- detailed/all-limits table as deterministic rendered output
- no `ahead` or `behind` pace wording
- no OAuth token, auth header, raw account email, or raw provider JSON in output
- weekly bottleneck row keeps the weekly window/reset/runout visible

## Spec Boundary / Separability Map

```text
Codex CLI
  owns: sessions, history, retries, transport behavior, custom provider config
  exposes: OpenAI-compatible HTTP/WebSocket traffic to local router

Codex custom provider config
  owns: base_url and X-Codex-Router-Token env-header reference
  exposes: loopback request with local bearer header

codex-router CLI
  owns: account onboarding UX, quota status UX, profile/token activation UX
  exposes: account import/list/enable/disable, quota refresh/status
  reserves: account login and account logout until their backing contracts exist

SQLite state store
  owns: non-secret account metadata, selector snapshots, quota status rows, route bands, status
  exposes: account and quota repository traits

Secret store
  owns: OAuth token material and local router bearer token
  exposes: read/write account-scoped secrets in this slice
  reserves: delete account-scoped secrets for future logout support

Auth/quota client
  owns: OAuth parsing/refresh/fetch classification and provider quota fetch
  exposes: redacted quota responses and persistence-ready summaries

Background quota worker
  owns: enabled-account scan, interval, provider quota fetch, SQLite quota status writes, per-account refresh failure state, shutdown
  exposes: persisted selector snapshots and persisted quota status rows

Proxy/runtime selector
  owns: per-request account selection from router-owned state
  consumes: enabled accounts, fresh/stale quota snapshots, access tokens
  prohibited: provider quota fetch or refresh scheduling
```

## Scope Decisions Locked For This Slice

1. Browser/device OAuth login command is reserved and must fail closed if exposed. The usable onboarding path for this slice is `account import-codex-auth`.
2. Full logout is reserved until `SecretStore::delete_secret` exists. This slice must expose `disable`; it must not claim logout if secret material remains readable.
3. Background quota refresh is runtime scope, not only CLI scope. `serve` must schedule periodic refresh and request-time selection must prove it reads SQLite snapshots without provider I/O.
4. Compact quota table rows are keyed by `(account, route band)`. A one-row-per-account summary is allowed only if it preserves route-band differences in explicit notes and cannot hide an exhausted route.
5. `quota status --refresh`, if added, is only a convenience wrapper around explicit refresh. Plain `quota status` reads persisted snapshots and performs no provider I/O.
6. `quota status --format json` is out of scope for this slice unless plan creation adds a positive-schema proof row before implementation.

## Plan-Creation Proof Matrix Inputs

`plan-creation-swarm` must produce one proof row per requirement R1-R10 plus R4A. Each row must include requirement id, proof layer, fixture or mock, exact command, expected observation, stale-proof guard, redaction canaries, and source reference.

Mandatory proof rows:

- R1 import success: OAuth `auth.json` fixture imports account metadata to SQLite and token material to secret store without mutating the source file.
- R1 import rejection: API-key `auth.json` fixture is rejected and the API key canary is absent from stdout/stderr.
- R2 login reserved: `account login` fails closed with an import command pointer, or is absent from help/parser if not exposed.
- R3 account lifecycle: list/enable/disable handle enabled, disabled, missing-token, and ambiguous-label cases without printing secrets.
- R3 logout reserved: `account logout` is absent or fails with a delete-secret-not-supported message until `SecretStore::delete_secret` exists; no test may pass by blanking a secret.
- R4 manual quota refresh: mock provider response persists SQLite snapshots for at least `responses` route band and records redacted per-account failure status.
- R4 persisted quota status: SQLite stores enough normalized compact and detailed quota rows to render status without provider I/O.
- R4 quota status local-only: seeded SQLite snapshots render status while a provider mock is configured to fail the test if called.
- R4A serve background refresh: `serve` startup reads existing snapshots immediately, schedules periodic refresh for enabled accounts, persists success/failure, and request handling performs zero provider quota calls.
- R5 compact table: deterministic rendered output has `Account | Route | Status | Headroom | Window | Reset | Pace | Runout | Notes`, ASCII headroom bar, and route-band identity.
- R6 detailed table: deterministic all-limits output expands quota families/windows without raw token/auth/provider JSON.
- R7 wording: output contains no `ahead` or `behind`; burn/runway terms are deterministic.
- R8 plain output: redacted stable plain output remains available; JSON is either explicitly out of scope or schema-proved.
- R9 storage backend disclosure: CLI/help/docs make file-backend plaintext-at-rest limitations visible if Keychain is not implemented in the slice.
- R10 live gate: without explicit transcript approval the gate reports `not-run: approval required`; approved live proof records only redacted command/result fields.

## Security Context

Sensitive assets:

- OAuth access tokens
- OAuth refresh tokens
- local router bearer token
- raw auth JSON
- raw account emails
- upstream auth headers
- prompts, request bodies, response bodies, tool arguments, memory traces

Trust boundaries:

- CLI arguments and filesystem paths are untrusted input.
- Imported `auth.json` is untrusted JSON and must be parsed through structured models.
- `~/.codex` and Prodex profiles are external source material, not router-owned state.
- Provider responses are untrusted network input.
- Table/plain/json output is an exfiltration surface.

Required controls:

- positive schema output only
- no raw Debug prints for secret-bearing types
- no source file mutation during import
- symlink/path protections preserved for file backend
- explicit live approval gate
- explicit home-write approval gate remains separate from account login/import

## Security Acceptance Contract

State DB writes:

- Commands that create or migrate SQLite state must either keep `--state-db` under `--router-root` or require an explicit approval flag for out-of-root paths.
- SQLite paths must reject symlinks and `.codex` paths before migration writes, matching the spirit of the file secret-store guards.
- Import must never mutate the source `auth.json`, source profile directory, `~/.codex`, or `~/.prodex`.

Account identity:

- Account ids must be generated as opaque router ids or hashes that cannot reveal raw email/account PII.
- Default labels must be caller-provided or redacted; imported raw email must not become a default label.
- CLI output, SQLite metadata, and future JSON output must treat raw email as sensitive.

Provider quota normalization:

- Provider-controlled percentages must be clamped to a safe 0-100 range before persistence, selection, or rendering.
- Provider-controlled labels must be sanitized and bounded before storage/output.
- Unknown, malformed, negative, oversized, or missing quota fields must fail closed into `unknown`, `missing`, or redacted failure status, not positive routing headroom.
- Selection must consume normalized persisted values only.

Network approval:

- Human-run commands whose purpose is provider interaction, such as `quota refresh`, are explicit operator intent and may perform network I/O.
- Review/proof/live-gate execution still requires explicit transcript approval before using real OAuth/provider services.
- Plain `quota status` and request-time selection are never implicit provider-network approval.

## Alternatives Considered

Alternative A: Keep using `live quota --profiles-root` as the user workflow.

Gain: smallest change.

Cost: violates router-owned credential source of truth, keeps Prodex/Codex profiles as runtime dependency, and does not solve real onboarding.

Alternative B: Implement only table polish first.

Gain: quick UX improvement.

Cost: still no usable account setup path; user cannot reliably route real Codex through codex-router.

Alternative C: Add import now, reserve browser OAuth login.

Gain: creates a usable, testable account onboarding path quickly while preserving the intended login command contract.

Cost: imported access-token-only accounts may have expiry limitations until refresh-token parsing/login lands.

Recommended direction: C, plus the quota table fix in the same goal because account onboarding and quota status are one user workflow.

## Open Design Decisions For Review

1. Should the first implementation require macOS Keychain as the default backend immediately, or explicitly ship file-backend onboarding as a development fallback while keeping Keychain as the spec target?
2. Does imported Codex/Prodex `auth.json` reliably include refresh token and expiry fields in the local source shapes we need to support?

## Next Workflow

Recommended next workflow: `shravan-dev-workflow:spec-review-swarm`

The review should pressure:

- whether the account import/login contract is too broad for one implementation slice
- whether secret-store delete/keychain support must be implemented before logout can be honest
- whether quota persisted schema is rich enough for the promised table UX
- whether the compatibility `live quota` path should be renamed or visually labeled to avoid confusing it with router-owned quota status

phase_result: complete
evidence: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/quota-output-account-onboarding-spec.md
recommended_next_workflow: shravan-dev-workflow:spec-review-swarm
recommended_transition_reason: Draft spec exists and needs adversarial review before implementation planning.
