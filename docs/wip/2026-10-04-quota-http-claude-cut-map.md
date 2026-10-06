# Quota HTTP Claude test cut map

Bounded test-only pure reorganization under the owner continuation instruction. Existing HTTP/SQLite fixtures and credential-recovery semantics remain unchanged.

## Write set

- `crates/codex-router-cli/src/cli_contract_tests/quota_http_tests.rs`
- new `crates/codex-router-cli/src/cli_contract_tests/quota_http_claude_tests.rs`

## Exact map

Before the cut, `quota_http_tests.rs` had 1247 lines. Keep the shared HTTP providers and the existing usage/persistence/401-timeout tests through old line 989. Move old lines 990–1247 into the private child:

- `RecoveringClaudeQuotaCredentialResolver`
- `RecordingClaudeQuotaProvider`
- `claude_quota_401_recovers_credentials_and_retries_the_usage_request`
- `background_quota_refresh_polls_only_claude_accounts_idle_longer_than_interval`

The parent now has 991 lines and declares `quota_http_claude_tests` as a private path child. The child is 260 formatted lines and imports parent fixtures with `use super::*`. No HTTP response, credential generation, idle-account selection or SQLite assertion changes.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- Compiled test discovery identified both child tests under `tests::quota_http_tests::quota_http_claude_tests`.
- `cargo test -p codex-router-cli --features keychain-test-support tests::quota_http_tests::quota_http_claude_tests::`: exit 0; 2 passed, 0 failed, 420 filtered. The repository feature is required by its compiled CLI acceptance guard.
- `cargo check -p codex-router-cli --locked`: exit 0, 1.70 seconds.
- `cargo clippy -p codex-router-cli --all-targets --locked --features keychain-test-support -- -D warnings`: exit 0, 5.69 seconds.
- Strict size checker: exit 1 with 22 remaining oversized files; quota_http_tests.rs is no longer listed.

The first focused invocation used the wrong module path and ran zero matching tests; compiled discovery corrected the path before the passing run. Linker emitted the existing Apple `__eh_frame` warning only. No API/HTTP/schema/auth/SQLx/CI/production changes.
