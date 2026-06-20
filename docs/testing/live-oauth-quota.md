# Live OAuth And Quota Proof Runbook

Date: 2026-06-20
Status: implemented and run after explicit approval

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

This revision has a narrow live quota proof command:

```shell
cargo run -p codex-router-cli -- live quota --auth-json <path> --profile-label <label>
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root>
```

The command reads Codex/Prodex-style OAuth `auth.json`, calls the ChatGPT usage
endpoint, and prints only redacted quota window summaries. It rejects API-key
auth for quota because the ChatGPT quota endpoint requires Codex OAuth access
tokens.

The implemented CLI surface is intentionally limited to:

```shell
cargo run -p codex-router-cli -- profile print --port 8787
cargo run -p codex-router-cli -- profile doctor
cargo run -p codex-router-cli -- profile write --codex-home <temp-codex-home> --port 8787 --dry-run
cargo run -p codex-router-cli -- token export --router-root <router-secret-root> --shell posix
cargo run -p codex-router-cli -- serve --state-db <state.sqlite> --secret-root <router-secret-root> --upstream-base-url <url>
cargo run -p codex-router-cli -- live quota --auth-json <path> --profile-label <label>
cargo run -p codex-router-cli -- live quota --profiles-root <prodex-profiles-root>
```

The profile, token, and serve commands are not live OAuth proof by themselves.
They prove local profile, token export, and router/proxy behavior only. The
`live quota` command is live OAuth quota proof.

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

Approved live quota proof command:

```shell
cargo run -p codex-router-cli -- live quota --profiles-root <oauth-profiles-root>
```

Required properties:

- never prints OAuth refresh tokens, access tokens, auth headers, or raw JSON
- skips transient `.login-*` profile directories
- reports only profile label, status, quota remaining percentage, reset presence,
  window size, and additional-window count
- performs no writes to `~/.codex`, `~/.prodex`, or router state
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

## Current Gate Result

```text
live_oauth_quota_gate: run
approval: explicit user approval in transcript on 2026-06-20
command: cargo run -q -p codex-router-cli -- live quota --profiles-root <oauth-profiles-root>
result: 3 OAuth profiles returned status ok from the ChatGPT usage endpoint
redaction: no tokens, auth headers, emails, or raw response bodies printed
```
