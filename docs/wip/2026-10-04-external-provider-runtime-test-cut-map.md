# External provider runtime test cut map

Bounded mechanics-only split of the existing `#[cfg(test)]` runtime child under the owner continuation instruction. The cut preserves all ACP subprocess fixtures and runtime behavior while separating admission/lifecycle tests from MCP/shutdown tests.

## Write set

- `crates/codex-router-host/src/external_provider_runtime/tests.rs`
- new `crates/codex-router-host/src/external_provider_runtime/tests/admission_lifecycle_tests.rs`
- new `crates/codex-router-host/src/external_provider_runtime/tests/mcp_shutdown_tests.rs`

## Exact map

Before the cut, the existing test child had 1717 physical lines. Keep imports, shared fixture builders, safe-error tests, steering/error classification tests, and early session-load tests through old line 584 in `tests.rs`. Move old lines 585–1153 (admission/lifecycle/process/permission tests plus their local process-id helper) to `admission_lifecycle_tests.rs`; move old lines 1154–1717 (MCP observation, shutdown, cancellation, output/replay, and ignored live-provider tests) to `mcp_shutdown_tests.rs`. The parent is 589 lines; the children are 571 and 566 lines and use `use super::*` for the shared fixture surface.

No external-provider runtime API, ACP protocol, subprocess fixture behavior, persistence, auth/security, CI, or production behavior changes. `external_provider_runtime.rs` keeps its existing `#[path = "external_provider_runtime/tests.rs"] mod tests;` wiring; only private child modules are added inside that test module.

## Proof

- `cargo test -p codex-router-host external_provider_runtime::tests:: --lib`: exit 0; 35 passed, 0 failed, 2 ignored (the pre-existing authenticated external-provider tests).
- `cargo fmt --all -- --check`: exit 0 after removing the formatter-only trailing blank line from the admission child.
- `cargo check -p codex-router-host --locked`: exit 0.
- `cargo clippy -p codex-router-host --all-targets --locked -- -D warnings`: exit 0.
- Parent prefix comparison against `git show HEAD:crates/codex-router-host/src/external_provider_runtime/tests.rs`: exit 0; first 584 lines byte-equivalent.
- Child-body comparisons: MCP child exact; admission child exact modulo the old terminal blank line removed by formatting.
- Strict size checker: exit 1 with 10 remaining oversized Rust files; `external_provider_runtime/tests.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the three Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
