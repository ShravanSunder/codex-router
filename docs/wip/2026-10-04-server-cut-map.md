# Loopback server decomposition cut map

Bounded same-module mechanics-only decomposition under the owner continuation instruction. The server file had accumulated listener/runtime, Hyper protocol, response-body, diagnostics, test, and adapter responsibilities; this cut preserves its existing namespace and exports while chunking each responsibility physically.

## Write set

- `crates/codex-router-proxy/src/server.rs`
- new `crates/codex-router-proxy/src/server/{loopback_bind,runtime_config,runtime_lifecycle,connection_diagnostics,protocol_handler,auth_reloader,hyper_request,response_body,db_affinity,errors_and_adapters,tests_part1,tests_part2,tests_part3}.rs`

## Exact map

Before the cut, `server.rs` had 4931 physical lines. The parent now retains imports/constants and same-module `include!` wiring (179 lines). Responsibility chunks cover loopback binding, runtime configuration/lifecycle, connection diagnostics, protocol handler, auth reloader, Hyper request/response processing, DB affinity integration, errors/adapters, and three test groups; each chunk is below 1000 physical lines (largest: protocol handler 674, runtime config 736, tests part 3 735).

Same-module includes preserve existing private cross-function/type relationships and public exports. Error derives/doc comments were kept with their enums. No server API, HTTP/WebSocket behavior, persistence, auth/security, CI, or production behavior changes.

## Proof

- `cargo test -p codex-router-proxy server::tests:: --lib`: exit 0; 24 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p codex-router-proxy --locked`: exit 0.
- `cargo clippy -p codex-router-proxy --all-targets --locked -- -D warnings`: exit 0.
- Strict checker: exit 0; `Rust file size check passed: 1706 files (maximum 1000 physical lines)`.

This checkpoint is intentionally uncommitted until the parent, all new server chunks, this map, and the work trace are staged together. CI integration and the final workspace gate remain separate work.
