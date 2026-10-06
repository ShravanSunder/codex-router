# MCP HTTP listener test cut map

Bounded mechanics-only split of the existing listener test child under the owner continuation instruction. The cut preserves parent-private listener access, real HTTP/SSE/control/ACP fixtures, and all listener behavior while separating initialization from response-loss scenarios.

## Write set

- `crates/collaboration-mcp/src/mcp_http_listener_tests.rs`
- new `crates/collaboration-mcp/src/mcp_http_listener_tests/initialization_tests.rs`
- new `crates/collaboration-mcp/src/mcp_http_listener_tests/response_loss_tests.rs`

## Exact map

Before the cut, the existing listener test child had 1696 physical lines. Keep bind/drop/cancellation tests, shared imports, and the schema/origin/IPv6 tests in the parent. Move old lines 338–743 (`real_http_initialization_discovers_typed_tools_without_authentication`) to `initialization_tests.rs`; move old lines 744–1516 (eight initialized response-loss/observation scenarios plus their local replay helper) to `response_loss_tests.rs`. The two children are 408 and 725 lines; the parent is 573 lines.

`protocol_response_json` and `initialize_mcp_session` are shared by parent tests and both children. They therefore remain parent-owned helpers; the response child no longer privately owns them. This correction preserves the original call graph without promoting visibility beyond the parent test module.

No MCP HTTP/API, cancellation, origin, session, response-loss, ACP, persistence, auth/security, CI, or production behavior changes. `mcp_http_listener.rs` keeps its existing `#[path = "mcp_http_listener_tests.rs"]` wiring; only private child modules are added inside that test module.

## Proof

- `cargo test -p collaboration-mcp mcp_http_listener_tests:: --lib`: exit 0; 17 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0 after formatter normalization.
- `cargo check -p collaboration-mcp --locked`: exit 0.
- `cargo clippy -p collaboration-mcp --all-targets --locked -- -D warnings`: exit 0.
- Parent prefix and moved test-name inventory were checked against `git show HEAD:crates/collaboration-mcp/src/mcp_http_listener_tests.rs`; all moved scenario names remain exactly once in their child and once in the pre-cut source.
- Shared-helper ownership was compile-validated after the first split exposed missing parent references; no public visibility was added.
- Strict size checker: exit 1 with 9 remaining oversized Rust files; `mcp_http_listener_tests.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the three Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
