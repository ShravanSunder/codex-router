# MCP server test cut map

Bounded mechanics-only split of the existing macro-adjacent server test child under the owner continuation instruction. The cut keeps all route, catalog, schema, inspection, and snapshot assertions unchanged while assigning each responsibility family to a private child module.

## Write set

- `crates/collaboration-mcp/src/mcp_server/tests.rs`
- new `crates/collaboration-mcp/src/mcp_server/tests/route_tests.rs`
- new `crates/collaboration-mcp/src/mcp_server/tests/catalog_tests.rs`
- new `crates/collaboration-mcp/src/mcp_server/tests/schema_tests.rs`
- new `crates/collaboration-mcp/src/mcp_server/tests/inspect_snapshot_tests.rs`

## Exact map

Before the cut, the existing `mcp_server/tests.rs` child had 2313 physical lines. Keep shared fixture constants/helpers and the `expected_tool_name` catalog helper in the parent (186 lines). Move the complete route/effect suite (old lines 132–888) to `route_tests.rs` (769 formatted lines), catalog/description suite (old lines 889–1490) to `catalog_tests.rs` (603 lines), native-schema/boolean-validation suite (old lines 1491–2008) to `schema_tests.rs` (519 lines), and inspect plus golden-snapshot tests (old lines 2009–2313) to `inspect_snapshot_tests.rs` (263 lines).

Nested child references to production-private server functions were adjusted from `super::` to `super::super::` to preserve the original parent (`mcp_server`) target. The snapshot `include_str!` moved one directory deeper and now uses `../snapshots/main_success_schemas.json`, preserving the same checked-in fixture. No MCP API/schema/tool behavior, persistence, auth/security, CI, or production behavior changes.

## Proof

- `cargo test -p collaboration-mcp mcp_server::tests:: --lib`: exit 0; 36 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p collaboration-mcp --locked`: exit 0.
- `cargo clippy -p collaboration-mcp --all-targets --locked -- -D warnings`: exit 0.
- Test-name and fixture inventory preserved across the four child modules; snapshot path compile/runtime proof passed.
- Strict size checker: exit 1 with 6 remaining oversized Rust files; `mcp_server/tests.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the five Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
