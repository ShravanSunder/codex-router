# Permission entry path test cut map

Bounded mechanics-only integration-test split under the owner continuation instruction. Real broker, CLI and Streamable HTTP authorization/effect proofs remain unchanged.

## Write set

- `crates/collaboration-mcp/src/permission_entry_path_tests.rs`
- new `crates/collaboration-mcp/src/permission_entry_path_tests/real_entry_tests.rs`

## Exact map

Before the cut, the crate-root test module had 1313 lines. Keep imports, `ApprovalFixture`, delivery captures, native schema, broker/control setup and shared helpers through old line 617. Move the complete seven real entry tests old lines 618–1313 into the private child. Parent is619 lines; child is698 formatted lines and uses `use super::*` for shared real fixtures.

Moved scenarios: CLI response-loss/authorization, provider legacy approval, Streamable HTTP authorization/question, initialized HTTP response-loss and manifest preflight. No broker route, CLI effect, HTTP protocol, actor/generation, or permission semantics changed.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p collaboration-mcp --lib permission_entry_path_tests::`: exit 0; 7 passed, 0 failed, 82 filtered, 0.42 seconds.
- `cargo check -p collaboration-mcp --locked`: exit 0, 11.52 seconds.
- `cargo clippy -p collaboration-mcp --all-targets --locked -- -D warnings`: exit 0, 26.94 seconds.
- Strict size checker: exit 1 with 17 remaining oversized files; permission_entry_path_tests.rs is no longer listed.

No MCP API/schema, broker storage, auth/security, SQLx/migration, CI or production process changes.
