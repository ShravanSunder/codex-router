# Implementation Review Packet

Date: 2026-06-20
Goal id: 2026-06-20-codex-router
Mode: implementation

## Review Scope

Review the current greenfield worktree for `codex-router`.

There is no initial commit yet, so there is no meaningful base commit or branch
diff. Treat every repo file outside `.git/` and `target/` as in scope.

Primary source files:

- `Cargo.toml`
- `.cargo/audit.toml`
- `.github/workflows/ci.yml`
- `deny.toml`
- `rust-toolchain.toml`
- `rustfmt.toml`
- `crates/codex-router-auth/`
- `crates/codex-router-cli/`
- `crates/codex-router-core/`
- `crates/codex-router-proxy/`
- `crates/codex-router-quota/`
- `crates/codex-router-secret-store/`
- `crates/codex-router-selection/`
- `crates/codex-router-state/`
- `crates/codex-router-test-support/`
- `tests/smoke/installed_codex_mock.sh`
- `docs/testing/live-oauth-quota.md`

Planning and proof context:

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- `docs/plans/2026-06-20-codex-router-implementation-plan.md`
- `docs/plans/reviews/2026-06-20-codex-router-plan-review.md`
- `docs/wip/implementation-provenance.md`
- `tmp/workflow-state/2026-06-20-codex-router/details.md`
- `tmp/workflow-state/2026-06-20-codex-router/events.jsonl`
- `tmp/smoke/installed-codex-mock-26264-1781989282085.json`

## Intent

Build `codex-router` as a narrow local Rust proxy in front of the real OpenAI
Codex CLI.

Codex remains the CLI, protocol client, session owner, installer, config owner,
hook runner, MCP owner, and log/session/history owner. The router owns only
local router auth, upstream OpenAI/ChatGPT OAuth account credentials, quota
snapshots, account selection, upstream auth injection, and byte-preserving
forwarding of Codex custom-provider traffic.

## Non-Goals And Constraints

- No Codex fork, bundle, launcher, installer, command-line rewrite, or session
  manager.
- No Prodex multi-provider gateway, provider-core architecture, gateway admin,
  virtual keys, billing, metrics, guardrails, SCIM, SSO, tenant, Redis,
  Postgres, OpenAPI, or route-strategy surfaces.
- No Realtime/WebRTC support in v1.
- No Codex-owned gating/timeouts/context policy/retry/circuit/health layer.
- No silent `~/.codex` writes. Any Codex profile write must be explicit,
  preview-first, and approval-gated.
- Router binds loopback-only in v1.
- Missing or invalid local router token must reject before account selection or
  upstream connection.
- Secrets, tokens, raw account emails, request bodies, response bodies, prompts,
  memory traces, and tool arguments must not appear in logs, debug output,
  transcripts, or runbooks.
- Live OAuth/quota proof is gated by explicit approval and was not run.

## Implementation Proof Claimed

Fresh proof from implementation execution:

```text
cargo fmt --all -- --check: pass, exit 0
cargo clippy --workspace --all-targets -- -D warnings: pass, exit 0
cargo nextest run --workspace: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
cargo deny check: advisories ok, bans ok, licenses ok, sources ok, exit 0
cargo audit: scanned 73 crate dependencies, exit 0
actionlint .github/workflows/ci.yml: pass, exit 0
tests/smoke/installed_codex_mock.sh: 2 ignored smoke tests passed, exit 0
git diff --check: pass, exit 0
```

Installed-Codex mock smoke transcript:

```text
tmp/smoke/installed-codex-mock-26264-1781989282085.json
codex_status: exit status: 0
codex_stdout_contains_smoke_text: true
codex_version: codex-cli 0.141.0
profile_path: .../codex-home/codex-router.config.toml
upstream.handshake_count: 1
upstream.http_probe_count: 0
upstream.authorization_header: <redacted-present>
upstream.local_router_header_present: false
upstream.first_frame_type: response.create
upstream.first_frame_model: gpt-5.5
upstream.first_frame_stream: true
```

Live gate:

```text
live_oauth_quota_gate: not-run
reason: approval required; no tested live OAuth/quota CLI exists in this revision
```

## Reviewer Output Contract

You are a read-only reviewer. Do not edit files, run formatters, stage changes,
commit, or apply patches.

Review the provided scope against the intent and constraints. Return only
findings that are grounded in the repository, diff, tests, or cited plan text.

Do not trust implementation summaries, test claims, previous agent reports, or
other reviewer output. Verify by reading the actual artifacts in scope.

For each finding use this shape:

- severity: blocker | important | follow-up | nit
- title:
- evidence: exact file:line, symbol, command output, or plan section
- scenario: concrete failure, exploit, regression, or maintenance path
- smallest_fix:
- proof: test, check, or manual reproduction that would prove the fix
- confidence: high | medium | low

If you have no high-confidence findings, say `No findings.` Do not pad.
