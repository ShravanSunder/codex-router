Mode:
implementation

Review scope:
Current uncommitted working-tree diff in /Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.

Git range:
base_sha: 69ebd93157b6874a00180d4d46db0e9bc854734e
head_sha: working tree
branch: feature/initial-codex-router
diff_stat_command: git diff --stat
diff_command: git diff -- . ':!tmp'
changed_files:
- Cargo.toml
- Cargo.lock
- README.md
- crates/codex-router-auth/src/lib.rs
- crates/codex-router-auth/src/live_quota.rs
- crates/codex-router-auth/src/router_credentials.rs
- crates/codex-router-cli/Cargo.toml
- crates/codex-router-cli/src/account.rs
- crates/codex-router-cli/src/lib.rs
- crates/codex-router-cli/src/live.rs
- crates/codex-router-cli/src/quota.rs
- crates/codex-router-proxy/src/http_sse.rs
- crates/codex-router-proxy/src/lib.rs
- crates/codex-router-secret-store/src/account_tokens.rs
- crates/codex-router-secret-store/src/file_backend.rs
- crates/codex-router-secret-store/src/lib.rs
- crates/codex-router-secret-store/src/model.rs
- crates/codex-router-state/src/account.rs
- crates/codex-router-state/src/lib.rs
- crates/codex-router-state/src/quota_snapshot.rs
- crates/codex-router-state/src/repositories.rs
- crates/codex-router-state/src/sqlite.rs
- docs/specs/2026-06-20-codex-router-greenfield-spec.md
- docs/testing/live-oauth-quota.md

Intent:
Deliver codex-router account onboarding and quota status UX for real Codex routing. The key requirement being reviewed now is that quota refresh is not in the request hot path: serving should read cached SQLite quota snapshots, and provider quota fetch/write should run through a background periodic worker plus explicit manual refresh. Also review router-owned account import, local secret-state boundaries, compact/detailed quota status output, docs, and proof gates.

Constraints:
- Preserve Codex/Prodex auth.json as compatibility/import/live-proof input only; normal serving must use router-owned state and secret-store boundaries.
- Do not invent a working `codex-router account login` browser/device command in docs. Current implemented onboarding path is `account import-codex-auth`.
- File secret backend is plaintext fallback and must require explicit `--allow-plaintext-file-secrets` acknowledgement for imported OAuth material.
- Request-time routing must not perform broad provider quota refresh.
- Quota status should read SQLite state only and render compact default effective rows, with `--all-limits` for detailed provider windows.
- Live OAuth/quota proof is approval-gated and remains not run for this changed revision.
- Reviewers are read-only. Do not edit files, run formatters, stage changes, commit, or apply patches.

Security context: applicable
- Changed assets: imported OAuth/access token material, router local bearer token, SQLite quota/account state, local file secret backend, provider quota endpoint calls.
- Entry points: `account import-codex-auth`, `account list|enable|disable`, `quota status`, `quota refresh`, `serve`, `live quota`.
- Trust boundaries: source Codex/Prodex `auth.json` to router-owned secret store, router root filesystem paths, local loopback test-only quota endpoint allowance, provider-only quota endpoint policy by default, request auth token to upstream bearer token translation.
- Sensitive data that must stay redacted: OAuth refresh/access tokens, auth headers, raw account emails, local router bearer tokens, raw live responses/prompts.
- Forbidden broadening: no filesystem, network, subprocess, package-script, CI, MCP, plugin, agent, external-model, auth, or secret-boundary broadening beyond reviewed scope.

Implementation proof:
- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p codex-router-state`: exit 0, 13 passed.
- `cargo test -p codex-router-secret-store`: exit 0, 9 passed.
- `cargo test -p codex-router-auth`: exit 0, 14 passed.
- `cargo test -p codex-router-cli`: exit 0, 35 passed.
- `cargo test -p codex-router-proxy`: exit 0, 40 passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0.
- `cargo nextest run --workspace`: exit 0, 140 run, 140 passed, 2 skipped.
- `cargo deny check`: exit 0, advisories/bans/licenses/sources ok; warning only for duplicate `getrandom` and `windows-sys`.
- `cargo audit`: exit 0, no vulnerabilities printed.
- `actionlint .github/workflows/ci.yml`: exit 0.
- Live OAuth/quota gate: not run, approval required for this changed router-owned import/refresh revision.

Source-of-truth inputs:
- User requirement: quota should be saved in the background periodically to SQLite so requests are not blocked on quota fetches.
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:110`: quota snapshots are refreshed in the background; request-time routing reads existing snapshots and must not block the accept loop on broad quota refresh.
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:188`: normal serving, background quota refresh, account selection, and token refresh must read credentials through the router secret-store boundary.
- `README.md:18-20`: current OAuth onboarding is explicit import; no interactive account login yet.
- `README.md:53-55`: serving reads existing SQLite snapshots and background quota refresh periodically writes updated quota state back to SQLite.
- `docs/testing/live-oauth-quota.md:50-62`: quota refresh writes SQLite, quota status reads SQLite only, serve owns background quota refresh worker.
- `docs/testing/live-oauth-quota.md:108-116` and `docs/testing/live-oauth-quota.md:192-198`: live proof approval boundary and current not-run result.
- `crates/codex-router-cli/src/lib.rs:99-107`: serve starts `BackgroundQuotaRefreshWorker`.
- `crates/codex-router-cli/src/lib.rs:323-378`: worker lifecycle, interval wait, one-shot test hook, stop/join.
- `crates/codex-router-cli/src/quota.rs:282-385`: shared manual/background refresh path reads router root, secret store, provider quota endpoint, persists status rows, and records redacted failures.
- `crates/codex-router-state/src/sqlite.rs:496-528`: route quota state replacement is SQLite transaction-scoped.
- `crates/codex-router-cli/src/lib.rs:2974-3128`: serve test proves a router request succeeds while a separate quota mock receives only the background quota usage request and SQLite headroom changes to the refreshed value.

Focus:
Full implementation review, with special pressure on:
- background quota refresh is truly off the request path
- SQLite persistence and failure rows are robust enough for periodic background refresh
- router-owned auth/secret boundaries and redaction
- CLI/docs contract drift around onboarding, quota commands, and live proof
- implementation proof maps to the request rather than merely testing nearby behavior

Output contract:
Return candidate findings only. Do not edit files. For each finding include severity, evidence, failure scenario, smallest fix, proof/test, and confidence. Return an artifact path written or `chat-only/no-files exception: <reason>`. Return a completion receipt: answered | blocked, with source anchors and artifact path.
