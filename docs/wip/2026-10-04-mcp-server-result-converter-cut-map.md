# MCP server result-converter cut map

Bounded same-crate mechanics-only split under the owner continuation instruction. The cut moves structured MCP success/error projection into a private sibling while leaving the macro-heavy tool-router implementation and handler boundary intact.

## Write set

- `crates/collaboration-mcp/src/mcp_server.rs`
- new `crates/collaboration-mcp/src/mcp_server/result_converters.rs`

## Exact map

Before the cut, `mcp_server.rs` had 1257 physical lines. Move the complete structured-result, provider/board/domain error-converter, message/reply projection, validation, and wake-wait conversion block (old lines 908–1207) into `result_converters.rs` (303 formatted lines). Keep `CollaborationMcpServer` construction, schema normalization, all `#[tool_router]` methods, and `ServerHandler` in the parent. The parent is 959 formatted lines.

The child exposes only `pub(super)` converter functions; the parent imports them privately. Existing sibling modules and nested tests continue resolving the same names through the parent scope. No MCP API/schema/tool behavior, persistence, auth/security, CI, or production behavior changes.

## Proof

- `cargo test -p collaboration-mcp --lib`: exit 0; 87 passed, 0 failed, 2 ignored.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p collaboration-mcp --locked`: exit 0.
- `cargo clippy -p collaboration-mcp --all-targets --locked -- -D warnings`: exit 0.
- Converter inventory/source comparison against `git show HEAD:crates/collaboration-mcp/src/mcp_server.rs`: all structured-result/message/reply/domain converter functions remain in the private child; tool-router and handler sections remain parent-owned.
- Strict size checker: exit 1 with 5 remaining oversized Rust files; `mcp_server.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the Rust parent, child, map, and work trace are staged together. No CI integration or final workspace gate is included in this slice.
