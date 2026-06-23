# R15 Lane: Auth/Profile Compatibility

Agent: Turing
Status: accepted by parent

## Finding

The generated Codex profile contract must match installed Codex's provider-auth
path. Current router helpers generate `env_key = "CODEX_ROUTER_TOKEN"`, and the
installed Codex model-provider path turns that into `Authorization: Bearer`.
That carrier is used for both HTTP/SSE requests and WebSocket upgrades.

## Accepted Spec Change

- Generated profile: `env_key = "CODEX_ROUTER_TOKEN"`.
- Accepted generated-profile ingress: `Authorization: Bearer`.
- Accepted manual/compatibility ingress: `X-Codex-Router-Token`.
- Mixed accepted carriers must match and validate; mismatches fail before
  selection.
- Query, cookie, request-body, and WebSocket subprotocol token carriers remain
  forbidden.

## Evidence Anchors

- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/model-provider-info/src/lib.rs`
