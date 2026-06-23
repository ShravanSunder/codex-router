# Live OAuth And Quota Proof Runbook

Date: 2026-06-21
Status: implemented; current changed revision live gate not run without approval

## Purpose

This runbook is the approval boundary for live OpenAI OAuth and quota proof.
It exists so mock implementation proof cannot accidentally become real account
execution.

Live proof covers only:

- real OAuth login or account credential import
- real quota fetch
- real account rotation
- real quota pooling across multiple accounts

Live proof must not inspect or print prompts, request bodies, response bodies,
memory traces, tool arguments, raw account emails, OAuth refresh tokens, access
tokens, or the local router bearer token.

## Current Executable State

This revision has a router-owned account and quota surface. Normal local use
starts from one router root:

```shell
cargo run -p codex-router-cli -- token init --router-root <router-root>
cargo run -p codex-router-cli -- account import-codex-auth --router-root <router-root> --label <local-safe-label> --auth-json <path-to-codex-auth.json> --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account list --router-root <router-root>
cargo run -p codex-router-cli -- quota refresh --router-root <router-root>
cargo run -p codex-router-cli -- quota status --router-root <router-root>
cargo run -p codex-router-cli -- quota status --router-root <router-root> --format plain
cargo run -p codex-router-cli -- quota status --router-root <router-root> --all-limits
cargo run -p codex-router-cli -- serve --router-root <router-root> --quota-refresh-interval-seconds 300 --quota-refresh-timeout-seconds 30
```

`account import-codex-auth` is the implemented OAuth setup path. There is not
yet an interactive `codex-router account login` browser/device flow. Import
copies compatible Codex/Prodex OAuth material into router-owned state and
secrets, leaves the source `auth.json` unchanged, rejects API-key auth, rejects
duplicate labels, accepts only the local-safe label alphabet `A-Z`, `a-z`,
`0-9`, `.`, `_`, and `-`, and prints only redacted account metadata.

`<router-root>/state.sqlite` is the SQLite state database. `<router-root>/secrets`
is the current file secret backend. This backend is plaintext at rest under
private filesystem permissions, so importing OAuth material requires the
explicit `--allow-plaintext-file-secrets` acknowledgement. Do not describe this
backend as encrypted storage. Router-owned state and secrets must not be placed
under `.codex` or `.prodex`.

`quota refresh` reads router-owned access tokens from `<router-root>/secrets`,
calls the provider quota endpoint, and persists selector snapshots plus
normalized status rows to SQLite. `quota status` reads SQLite only and performs
no provider I/O. Default quota status renders compact effective rows; `--all-limits`
renders detailed provider windows. Pace wording uses `steady`, `burn +N%`, and
`save N%`, not `ahead` or `behind`. `quota status --format plain` emits one
line per rendered row with percent-encoded values. `--format json` is not
implemented and is explicitly rejected. Provider response quota is also written
to the `models`, `memories_trace_summarize`, and `responses_compact` route bands
because the proxy selects those routes separately while the provider exposes one
shared response quota.

By default, provider quota fetches only allow
`https://chatgpt.com/backend-api`. Loopback/mock quota endpoints require the
explicit test-only `--allow-insecure-quota-base-url` flag on `quota refresh`,
`serve`, or `live quota`.

`serve` owns the background quota refresh worker. Request-time routing reads
existing SQLite snapshots and must not do broad provider quota polling. The
worker refreshes enabled accounts periodically using
`--quota-refresh-interval-seconds` and
`--quota-refresh-timeout-seconds`, then writes updated quota state back to
SQLite. Invalid quota-refresh endpoint configuration fails before listening;
later background refresh failures emit a redacted stderr line.

The compatibility live quota proof command remains available:

```shell
cargo run -p codex-router-cli -- live quota --auth-json <path> --profile-label <label>
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root>
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root> --format table --all-limits
```

`live quota` reads Codex/Prodex-style OAuth `auth.json` directly as a
compatibility proof input. It does not copy the file into router state and does
not make `auth.json` the runtime source of truth.

The implemented CLI surface is intentionally limited to:

```shell
cargo run -p codex-router-cli -- account import-codex-auth --router-root <router-root> --label <label> --auth-json <path> --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account list --router-root <router-root>
cargo run -p codex-router-cli -- account enable --router-root <router-root> --account <id-or-label>
cargo run -p codex-router-cli -- account disable --router-root <router-root> --account <id-or-label>
cargo run -p codex-router-cli -- quota status --router-root <router-root> [--format table|plain] [--all-limits]
cargo run -p codex-router-cli -- quota refresh --router-root <router-root> [--account <id-or-label>] [--base-url <url>] [--allow-insecure-quota-base-url]
cargo run -p codex-router-cli -- profile print --port 8787
cargo run -p codex-router-cli -- profile doctor
cargo run -p codex-router-cli -- profile write --codex-home <temp-codex-home> --port 8787 --dry-run
cargo run -p codex-router-cli -- token init --router-root <router-root>
cargo run -p codex-router-cli -- token export --router-root <router-root> --shell posix
cargo run -p codex-router-cli -- serve --router-root <router-root> [--upstream-base-url <url>] [--quota-refresh-base-url <url>] [--allow-insecure-quota-base-url] [--quota-refresh-interval-seconds <seconds>] [--quota-refresh-timeout-seconds <seconds>]
cargo run -p codex-router-cli -- live quota --auth-json <path> --profile-label <label> [--allow-insecure-quota-base-url]
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root> [--allow-insecure-quota-base-url]
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root> --format table --all-limits
```

The profile, token, account, quota, and serve commands are not live OAuth proof
by themselves when run against mock fixtures. They prove local profile, token
export, state, secret-store, quota persistence, and router/proxy behavior only.
The `live quota` command and any `quota refresh` against real imported accounts
are live OAuth quota proof.

Do not invent or run additional live commands such as `codex-router login`,
`codex-router live-proof`, or model-traffic live proof commands unless that CLI
surface has first been designed, implemented, tested, and added to this runbook.

## Approval Boundary

Without explicit user approval in the transcript, the live gate result is:

```text
not-run: approval required
```

Approval must name live OpenAI OAuth/quota execution directly. Approval to run
mock tests, installed-Codex smoke tests, package installs, or local router
tests does not authorize this live gate.

## Exact Commands For This Revision

Approval-gated live quota proof commands:

```shell
cargo run -p codex-router-cli -- quota refresh --router-root <router-root>
cargo run -p codex-router-cli -- quota refresh --router-root <router-root> --account <id-or-label>
cargo run -p codex-router-cli -- live quota --profiles-root <oauth-profiles-root>
cargo run -p codex-router-cli -- live quota --profiles-root <oauth-profiles-root> --format table --all-limits
```

Required properties:

- never prints OAuth refresh tokens, access tokens, auth headers, or raw JSON
- compatibility `live quota --profiles-root` skips transient `.login-*` profile
  directories
- reports only profile aliases such as `profile-1`, status, quota remaining
  percentage, reset presence, window size, and additional-window count
- with `--format table --all-limits`, expands main, code-review, and provider
  additional windows into redacted table rows and includes an effective
  bottleneck row for each quota window pair, plus reset-in, pace, and projected
  runout fields
- performs no writes to `~/.codex` or `~/.prodex`
- `live quota` performs no writes to router state
- `quota refresh` writes only router-owned SQLite quota state under
  `<router-root>/state.sqlite`
- treats `auth.json` as read-only compatibility/import input, not runtime
  credential storage
- API-key auth is reported as quota-incompatible rather than sent to the quota
  endpoint
- current runtime selection uses the persisted bottleneck `reset_unix_seconds`
  as a reset-aware weight input. Full-fidelity weekly-vs-short-window routing
  still requires a richer persisted per-window quota snapshot schema.

## Safe Local Evidence Already Covered Elsewhere

The current implementation proves non-live behavior through:

```shell
tests/smoke/installed_codex_mock.sh
cargo nextest run --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo audit
actionlint .github/workflows/ci.yml
```

Those commands are allowed local proof gates. They do not satisfy live OAuth or
live quota proof.

## Required Redaction For Future Approved Live Proof

Future approved live evidence must include only:

- timestamp
- command name and redacted arguments
- account index or hash, never raw email
- quota band or remaining-headroom band, never raw response bodies
- rotation decision reason
- pass/fail result

Future approved live evidence must exclude:

- OAuth refresh tokens
- access tokens
- local router bearer token
- raw auth headers
- request bodies
- response bodies
- prompts
- tool arguments
- memory traces
- raw account emails

## Current Gate Result

```text
live_oauth_quota_gate: not-run
reason: approval required for this changed router-owned import/refresh revision
previous_context: a narrower compatibility live quota proof was run on 2026-06-20, before the router-owned import/status/background-refresh surface changed
```
