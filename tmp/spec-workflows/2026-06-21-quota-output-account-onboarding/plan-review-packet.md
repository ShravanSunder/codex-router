# Plan Review Packet: Quota Output And Account Onboarding

Repo: /Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router
Branch/worktree: feature/initial-codex-router, dirty working tree with prior live quota/router changes plus untracked workflow artifacts
Review target: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/implementation-plan.md
Coverage from controller: 356 lines read in chunks 1-140, 141-280, 281-356
Mode: read-only plan review; do not implement code

## Threat Model / Security Context

Sensitive assets:

- OAuth access tokens
- OAuth refresh tokens
- local router bearer token
- raw auth JSON
- raw account email/PII
- upstream auth headers
- prompts, request bodies, response bodies, tool arguments, memory traces

Entry points and untrusted inputs:

- CLI args: `--router-root`, `--state-db`, `--auth-json`, `--label`, `--base-url`, future refresh interval
- imported Codex/Prodex `auth.json`
- provider quota responses and labels
- SQLite paths and migrations
- CLI stdout/stderr and future docs/runbooks

Hard invariants:

- Codex owns sessions, transport behavior, retries, home config unless explicitly preview/approved.
- Router owns account credentials, quota snapshots/status rows, account selection, and upstream auth injection.
- `quota status` and request-time selection must not call provider quota endpoints.
- Broad quota refresh happens in background into SQLite.
- `account logout` is reserved until real secret deletion exists.
- Live OAuth/quota proof requires explicit approval.

## Plan Summary

The plan proposes T1-T10: state path guards, richer quota status schema, credential import model, account import CLI, quota refresh CLI, lifecycle/reserved commands, quota renderer/table UX, serve background worker, docs, and validation.

## Major Claims To Verify

1. The plan satisfies the accepted spec without silently shrinking account onboarding, quota status, background refresh, or table UX.
2. The execution order is safe across shared state/auth/CLI/proxy surfaces.
3. The proof matrix can catch missing persistence, request-path provider calls, secret leaks, and fake login/logout.
4. The plan has enough detail for implementation without hidden user decisions.
5. The chosen storage/backend story is honest enough for "real Codex routing".

## Relevant Files And Docs

- implementation plan: primary target
- revised spec: source requirements
- spec review report: accepted findings
- `crates/codex-router-cli/src/lib.rs`: CLI dispatch/help/tests
- `crates/codex-router-cli/src/live.rs`: current live quota fetch/render coupling
- `crates/codex-router-state/src/sqlite.rs`: current SQLite schema and migration
- `crates/codex-router-state/src/quota_snapshot.rs`: reduced selector snapshot model
- `crates/codex-router-secret-store/src/file_backend.rs`: current secret-store trait/backing guards
- `crates/codex-router-secret-store/src/account_tokens.rs`: current access-token-only key convention
- `crates/codex-router-quota/src/worker.rs`: current scheduler facade only
- `crates/codex-router-proxy/src/server.rs`: runtime startup, no background worker
- `crates/codex-router-proxy/src/http_sse.rs`: request-time local selector
- `Cargo.toml`: workspace dependencies; currently blocking `reqwest`, `rusqlite`, no Keychain dependency observed

## Return Format

- Lane
- Backend
- Verdict: ready | needs revision | blocked
- Findings grouped as blocker | important | question | nit
- For each finding: evidence, failure scenario, smallest plan edit, proof/test
- For security findings: validation status as validated | unvalidated with proof gap | rejected
- Do not include speculative findings without evidence
