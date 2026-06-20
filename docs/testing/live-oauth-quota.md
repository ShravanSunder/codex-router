# Live OAuth And Quota Proof Runbook

Date: 2026-06-20
Status: not-run: approval required

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

No live OAuth or live quota command exists in this revision.

The implemented CLI surface is intentionally limited to:

```shell
cargo run -p codex-router-cli -- profile print --port 8787
cargo run -p codex-router-cli -- profile doctor
cargo run -p codex-router-cli -- profile write --codex-home <temp-codex-home> --port 8787 --dry-run
cargo run -p codex-router-cli -- token export --router-root <router-secret-root> --shell posix
cargo run -p codex-router-cli -- serve --state-db <state.sqlite> --secret-root <router-secret-root> --upstream-base-url <url>
```

These commands are not live OAuth proof by themselves. They prove local profile,
token export, and router/proxy behavior only.

Do not invent or run a `codex-router live-proof`, `codex-router login`,
`codex-router quota`, or similar command unless that CLI surface has first been
designed, implemented, tested, and added to this runbook.

## Approval Boundary

Without explicit user approval in the transcript, the live gate result is:

```text
not-run: approval required
```

Approval must name live OpenAI OAuth/quota execution directly. Approval to run
mock tests, installed-Codex smoke tests, package installs, or local router
tests does not authorize this live gate.

## Exact Commands For This Revision

There are no approved live commands for this revision.

If live proof is requested before a tested live CLI exists, stop and replan.
The replan must add a narrow live-proof surface with:

- a command that never prints tokens or raw account labels
- an explicit dry-run or preview mode
- redacted account handles only
- isolated router root and temp `CODEX_HOME`
- no writes to `~/.codex` unless separately approved
- tests proving redaction and approval gating before any live execution

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
reason: approval required; no tested live OAuth/quota CLI exists in this revision
next_step_if_required: replan and implement a redacted approval-gated live-proof command before running real accounts
```
