# Live OAuth And Quota Proof Runbook

Date: 2026-06-22
Status: implemented; router-owned multi-account live proof is approval-gated

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

This revision has router-owned account onboarding plus a read-only diagnostic
quota command.

Router-owned account setup:

```shell
cargo run -p codex-router-cli -- account login --label <label> --device-auth --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account login --label <label> --auth-json <path> --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account list
cargo run -p codex-router-cli -- quota refresh
cargo run -p codex-router-cli -- quota status --all-limits
```

By default, router-owned state is under `$HOME/.codex-router`. Use
`--router-root <path>` only for tests or an alternate local router home.

`account login --device-auth` delegates the interactive OAuth device-code flow
to the installed `codex` binary in a temporary owner-only `CODEX_HOME`, then
imports the resulting OAuth `auth.json` into router-owned account state. Use
`--codex-bin <path>` when the test must pin a specific Codex binary.

`account login --auth-json` imports an existing Codex/Prodex-style OAuth
`auth.json` into router-owned account state. It is an explicit migration,
recovery, and test setup path, not implicit steady-state shared auth.

`quota refresh` uses the router-owned credential resolver and persists provider
quota windows to SQLite. `quota status` reads SQLite only and performs no
provider I/O.

Read-only diagnostic live quota:

```shell
cargo run -p codex-router-cli -- live quota --auth-json <path> --profile-label <label>
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root>
```

The diagnostic command reads Codex/Prodex-style OAuth `auth.json` as a
compatibility input, calls the ChatGPT usage endpoint, and prints only redacted
quota window summaries. It does not copy the file into router state, does not
make `auth.json` the router runtime source of truth, and rejects API-key auth
for quota because the ChatGPT quota endpoint requires Codex OAuth access tokens.

The implemented CLI surface for local router proof is:

```shell
cargo run -p codex-router-cli -- account login [--router-root <path>] --label <label> --device-auth --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account login [--router-root <path>] --label <label> --auth-json <path> --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account list [--router-root <path>]
cargo run -p codex-router-cli -- quota refresh [--router-root <path>]
cargo run -p codex-router-cli -- quota status [--router-root <path>] --all-limits
cargo run -p codex-router-cli -- profile print --port 8787
cargo run -p codex-router-cli -- profile doctor
cargo run -p codex-router-cli -- profile write --codex-home <temp-codex-home> --port 8787 --dry-run
cargo run -p codex-router-cli -- serve [--state-db <state.sqlite>] [--secret-root <secret-root>] [--upstream-base-url <url>]
cargo run -p codex-router-cli -- live quota --auth-json <path> --profile-label <label>
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root>
```

The account, quota, profile, token, and serve commands prove router-owned local
behavior. Real provider execution through `account login --device-auth`, `quota
refresh`, or `live quota` is live OAuth/quota proof and must follow the approval
boundary below.

Do not invent or run additional live commands such as `codex-router
live-proof`, `account logout`, `account remove`, or model-traffic live proof
commands unless that CLI surface has first been designed, implemented, tested,
and added to this runbook.

## Approval Boundary

Without explicit user approval in the transcript, the live gate result is:

```text
not-run: approval required
```

Approval must name live OpenAI OAuth/quota execution directly. Approval to run
mock tests, installed-Codex smoke tests, package installs, or local router
tests does not authorize this live gate.

## Exact Commands For This Revision

Approved live quota proof commands for this revision:

```shell
cargo run -p codex-router-cli -- account login --label <label> --device-auth --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account login --label <label> --auth-json <path> --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- quota refresh
cargo run -p codex-router-cli -- quota status --all-limits
cargo run -p codex-router-cli -- live quota --profiles-root <oauth-profiles-root>
```

Required properties:

- never prints OAuth refresh tokens, access tokens, auth headers, or raw JSON
- skips transient `.login-*` profile directories
- reports only profile label, status, quota remaining percentage, reset presence,
  window size, and additional-window count
- performs no writes to `~/.codex`, `~/.prodex`, or router state
- treats `auth.json` as read-only compatibility input, not durable router
  credential storage
- API-key auth is reported as quota-incompatible rather than sent to the quota
  endpoint

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

## Prior Diagnostic Evidence

```text
live_quota_diagnostic_gate: run
approval: explicit user approval in transcript on 2026-06-20
command: cargo run -q -p codex-router-cli -- live quota --profiles-root <oauth-profiles-root>
result: 3 OAuth profiles returned status ok from the ChatGPT usage endpoint
redaction: no tokens, auth headers, emails, or raw response bodies printed
```

## Current Gate Result

```text
live_oauth_quota_gate: not-run
reason: approval required for router-owned device-auth/import plus real quota refresh and cycling proof
```
