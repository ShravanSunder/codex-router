# HTTP/SSE proxy decomposition cut map

Bounded mechanics-only decomposition under the owner continuation instruction. The cut separates public HTTP/SSE DTOs and transport traits, affinity/audit helpers, and the inline test suite while preserving crate-visible APIs.

## Write set

- `crates/codex-router-proxy/src/http_sse.rs`
- new `crates/codex-router-proxy/src/http_sse/http_types.rs`
- new `crates/codex-router-proxy/src/http_sse/http_affinity.rs`
- new `crates/codex-router-proxy/src/http_sse/tests.rs`

## Exact map

Before the cut, `http_sse.rs` had 2098 physical lines. Move the request/response DTOs, transport traits, proxy error and prepared completion types (old lines 240–859) to `http_types.rs` (619 formatted lines) and re-export them from the parent. Move affinity-owner body recording, audit-event mapping, response-id extraction, route/profile helpers and time utilities (old lines 1524–1803) to `http_affinity.rs` (283 lines), retaining crate-visible helper exports. Move the existing inline HTTP/SSE test module to `tests.rs` (290 lines). The parent is 916 formatted lines.

Fields and constructors required by the parent remain `pub(super)`; externally consumed helpers retain `pub(crate)` visibility. No HTTP/SSE API, routing, credential, audit, affinity, persistence, auth/security, CI, or production behavior changes.

## Proof

- `cargo test -p codex-router-proxy http_sse --lib`: exit 0; 4 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p codex-router-proxy --locked`: exit 0.
- `cargo clippy -p codex-router-proxy --all-targets --locked -- -D warnings`: exit 0.
- Strict size checker: exit 1 with 2 remaining oversized Rust files; `http_sse.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the four Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
