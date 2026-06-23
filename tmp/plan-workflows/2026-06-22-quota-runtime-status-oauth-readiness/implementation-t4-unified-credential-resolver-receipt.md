# T4 Unified Credential Resolver Receipt

Date: 2026-06-22
Worktree: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.plan1a-quota-substrate-05bf755`
Branch: `plan1a-quota-substrate-05bf755`

## Scope

Implemented the unified runtime provider credential resolver for quota refresh,
HTTP/SSE proxying, and WebSocket proxying. Runtime egress paths now consume
resolver APIs instead of directly reading provider token secrets.

## Implemented Rows

- 1A-06b: resolver reads only the active bundled credential generation.
- 1A-07: quota refresh resolves/refreshes provider credentials before provider egress.
- 1A-08: HTTP/SSE resolves/refreshes provider credentials before upstream egress.
- 1A-09: WebSocket resolves/refreshes provider credentials before upstream open.
- 1A-10: quota refresh fails closed when an expired bundle has no refresh token.
- 1A-11: HTTP/SSE fails closed when provider credentials cannot refresh.
- 1A-12: WebSocket fails closed when an expired bundle has no refresh token.
- 1A-13: concurrent resolver calls single-flight refresh and followers re-read the published generation.
- 1A-14: runtime egress paths cannot bypass the resolver.
- 1A-14a: runtime entrypoints stay file-backend neutral for request/refresh paths.
- 1A-14b: auth/proxy/CLI manifest direction is acyclic and compiles.
- 1A-14c: selector decision names no longer carry legacy provider-token DTO names.

## Proof

- `cargo test -p codex-router-auth tests::credential_resolver_reads_only_active_credential_bundle_generation -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo test -p codex-router-cli tests::quota_refresh_resolver_refreshes_expired_access_token_before_provider_egress -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo test -p codex-router-proxy tests::http_proxy_resolver_refreshes_expired_access_token_before_upstream_egress -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo test -p codex-router-proxy tests::authenticated_websocket_router_refreshes_expired_access_token_before_upstream_open -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo test -p codex-router-cli tests::quota_refresh_missing_refresh_token_fails_closed_before_provider_egress -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo test -p codex-router-proxy tests::http_proxy_missing_refresh_token_fails_closed_before_upstream_egress -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo test -p codex-router-proxy tests::authenticated_websocket_router_missing_refresh_token_fails_closed_before_upstream_open -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo test -p codex-router-auth tests::credential_resolver_single_flights_concurrent_quota_refresh_and_serve_request -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo nextest run -p codex-router-auth`
  - exit 0; 12 passed.
- `cargo nextest run -p codex-router-proxy`
  - exit 0; 44 passed.
- `cargo nextest run -p codex-router-cli`
  - exit 0; 39 passed.
- `cargo nextest run --workspace`
  - exit 0; 138 passed, 2 skipped.
- `cargo fmt --all --check`
  - exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit 0.
- `bash -lc '! rg -n -e "read_secret" -e "upstream_access_token_key" -e "upstream_refresh_token_key" crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs'`
  - exit 0; zero matches.
- `bash -lc '! rg -n -e "FileSecretStore" -e "file_backend::SecretStore" crates/codex-router-cli/src/quota.rs crates/codex-router-proxy/src/server.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs'`
  - exit 0; zero matches.
- `bash -lc '! rg -n -e "SelectedUpstreamAccount" -e "UpstreamAccountSelector" -e "upstream_auth_token" -e "provider_access_token" -e "provider_refresh_token" crates/codex-router-selection/src crates/codex-router-proxy/src/account_selection.rs crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs'`
  - exit 0; zero matches.
- `cargo tree -p codex-router-auth -e normal`
  - exit 0; auth depends on core, secret-store, state, reqwest, serde, serde_json, thiserror and does not depend on proxy or CLI.
- `cargo check -p codex-router-auth -p codex-router-proxy -p codex-router-cli`
  - exit 0.

## Notes

- Production `quota refresh` now has the resolver/provider seam, but the real
  provider quota HTTP implementation remains explicitly unavailable in Plan 1A.
  The implemented proof covers token freshness/fail-closed behavior before
  provider egress.
- Resolver refresh uses a shared per-account refresh lease registry. Followers
  re-read active generation after the owner publishes refreshed credentials.
