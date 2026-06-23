# Implementation Execute Plan Brief

Date: 2026-06-20
Goal id: 2026-06-20-codex-router
Workflow: `shravan-dev-workflow:implementation-execute-plan`
Plan: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/plans/2026-06-20-codex-router-implementation-plan.md`

## Coverage

- Plan line count: 861.
- Plan chunks read: 1-220, 221-440, 441-660, 661-861.
- Required context loaded:
  - spec: 450 lines
  - research evidence: 94 lines
  - spec review: 66 lines
  - README: 15 lines
  - workflow details: 245 lines
  - events log: 3 JSONL events

## Current State

- Latest valid orchestrator event transitions from
  `shravan-dev-workflow:plan-review-swarm` to
  `shravan-dev-workflow:implementation-execute-plan`.
- Repo state: no commits yet on `main`; `README.md`, `docs/`, and `tmp/` are
  untracked.
- No remote is configured for this repo.
- Allowed write scope remains this repo only; any `~/.codex` write requires
  explicit approval.

## T0 Result

T0 source provenance was gathered read-only. See:

`/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/wip/implementation-provenance.md`

Host-bootstrap status:

- `rustup` is on `PATH`.
- `rustc` and `cargo` are not on `PATH`, but pinned toolchain `1.95.0` exists
  and works through `rustup run 1.95.0`.
- `cargo-nextest`, `cargo-deny`, `cargo-audit`, and `actionlint` are missing.
- Per the reviewed plan, do not start T1 or product code until explicit
  approval is granted for host-bootstrap mutation, or the plan is updated to a
  different proof strategy.
- Rechecked at 2026-06-20T15:30:45Z: `cargo-nextest`, `cargo-deny`,
  `cargo-audit`, and `actionlint` are still missing.
- User approved standard package installs and approved this Homebrew install.
  Bootstrap completed at 2026-06-20T18:00:21Z:
  `cargo-nextest 0.9.137`, `cargo-deny 0.19.9`, `cargo-audit 0.22.2`,
  `actionlint 1.7.12`.

## Next Checkpoint

T1 workspace baseline, T2 core primitives, T3 secret store / refresh lease,
T4 local router auth / token export, T5 OAuth account store / SQLite metadata /
quota snapshot model, T6 selection / routing state machine, and T7 background
refresh runtime, and T7.5 contract freeze before proxy integration are
complete. T8 protocol-transform partial is complete. The next implementation
slice is the remaining T8 WebSocket first-frame/handshake protocol and loopback
server runtime.

Before T4 edits, re-open the plan T4 section and preserve the T1 resolved-tool
invocation:

```shell
TOOLCHAIN_BIN="$(dirname "$(rustup which cargo --toolchain 1.95.0)")"
PATH="$TOOLCHAIN_BIN:$HOME/.cargo/bin:$PATH"
RUSTC="$TOOLCHAIN_BIN/rustc"
CARGO="$TOOLCHAIN_BIN/cargo"
```

Latest proof after T4:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 25 tests run, 25 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T5:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 32 tests run, 32 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T6:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 38 tests run, 38 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T7:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 41 tests run, 41 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T7.5:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 45 tests run, 45 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 48 tests run, 48 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 HTTP/SSE handler partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 51 tests run, 51 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 WebSocket first-frame partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 53 tests run, 53 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 loopback server bind partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 56 tests run, 56 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 loopback HTTP adapter partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 58 tests run, 58 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 network-bound auth/selection partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 60 tests run, 60 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 quota-aware selector adapter partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 62 tests run, 62 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 bounded loopback HTTP accept loop partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 63 tests run, 63 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 repository hydration foundation partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 65 tests run, 65 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 repository-backed selector hydration partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 66 tests run, 66 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 upstream endpoint URL assembly partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 67 tests run, 67 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 local HTTP upstream transport partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 68 tests run, 68 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 authenticated WebSocket selection partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 70 tests run, 70 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T9 Codex profile helper partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 72 tests run, 72 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T9 Codex profile command wiring partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 77 tests run, 77 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T9 token export command wiring partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 79 tests run, 79 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 loopback router runtime assembly partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 80 tests run, 80 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T9 CLI serve command wiring partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 81 tests run, 81 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 blocking WebSocket tunnel partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 82 tests run, 82 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T8 runtime WebSocket dispatch partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 83 tests run, 83 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T9 CLI mixed WebSocket serve partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 84 tests run, 84 passed, 0 skipped
cargo deny check: pass
cargo audit: pass
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Latest proof after T10 installed-Codex mock smoke:

```text
tests/smoke/installed_codex_mock.sh: exit 0, 2 ignored smoke tests passed
cargo fmt --all -- --check: pass, exit 0
cargo clippy --workspace --all-targets -- -D warnings: pass, exit 0
cargo nextest run --workspace: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
cargo deny check: advisories ok, bans ok, licenses ok, sources ok, exit 0
cargo audit: scanned 73 crate dependencies, exit 0
actionlint .github/workflows/ci.yml: pass, exit 0
forbidden-scope and dependency guard checks: pass, exit 0
```

T10 notes:

- installed Codex observed by smoke: `codex-cli 0.141.0`
- generated profile target: temp `CODEX_HOME/codex-router.config.toml`
- generated profile uses `model_provider = "codex-router"`,
  `[model_providers.codex-router]`, `name = "codex-router"`,
  `wire_api = "responses"`, `supports_websockets = true`,
  `requires_openai_auth = false`, and token env header mapping
- smoke activation uses the T4/T9 token export helper path, not bespoke env
  injection
- redacted transcript:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/smoke/installed-codex-mock-26264-1781989282085.json`
- hostile no-token smoke keeps the mock upstream connection count at zero

Latest proof after T11 gated live runbook:

```text
docs/testing/live-oauth-quota.md: 116 lines
live OAuth/quota gate: not-run, approval required
git diff --check: pass, exit 0
executable-surface fake live command guard: pass, exit 0
runbook required-status markers: present, exit 0
cargo fmt --all -- --check: pass, exit 0
cargo clippy --workspace --all-targets -- -D warnings: pass, exit 0
cargo nextest run --workspace: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
cargo deny check: advisories ok, bans ok, licenses ok, sources ok, exit 0
cargo audit: scanned 73 crate dependencies, exit 0
actionlint .github/workflows/ci.yml: pass, exit 0
tests/smoke/installed_codex_mock.sh: 2 ignored smoke tests passed, exit 0
```

T11 notes:

- current revision has no tested live OAuth/quota CLI command
- no live account command was run
- the runbook forbids placeholder `codex-router live-proof`, `login`, or
  `quota` commands before design/implementation/testing
- if live proof is required before a tested live CLI exists, replan first
