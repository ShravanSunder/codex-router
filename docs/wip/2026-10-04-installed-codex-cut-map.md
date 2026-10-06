# Installed Codex smoke harness cut map

Bounded mechanics-only physical decomposition under the owner continuation instruction. The harness already had retry and floor-switch children; this cut extends that responsibility structure across its smoke modes, process runtime, transcripts, websocket/http probes, quota/concurrency harnesses, and tests.

## Write set

- `crates/codex-router-test-support/src/installed_codex.rs`
- new `crates/codex-router-test-support/src/installed_codex/{smoke_modes,smoke_runtime,hostile_and_state,process_runtime,smoke_contracts,transcript_observation,s8_provenance,websocket_models,websocket_upstream,quota_reconnect_upstream,concurrent_websocket,http_probe,tests}.rs`

## Exact map

Before the cut, `installed_codex.rs` had 7385 physical lines. The parent now retains imports, constants, existing retry/floor-switch declarations, and same-module `include!` wiring (114 lines). Responsibility chunks are each below the 1000-line checker threshold: smoke modes 511, smoke runtime 552, hostile/state 505, process runtime 540, smoke contracts 349, transcript observation 662, S8 provenance 425, WebSocket models 798, WebSocket upstream 477, quota reconnect 82, concurrent WebSocket 633, HTTP probe 826, and tests 920 lines.

The chunks use same-module `include!` wiring so all existing private helper/type relationships and public exports remain unchanged. No smoke protocol, artifact schema, subprocess, auth/token, persistence, network, CI, or production behavior changes.

## Proof

- `cargo test -p codex-router-test-support --lib`: exit 0; 19 passed, 0 failed, 15 ignored.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p codex-router-test-support --locked`: exit 0.
- `cargo clippy -p codex-router-test-support --all-targets --locked -- -D warnings`: exit 0.
- Strict size checker: exit 1 with 1 remaining oversized Rust file; `installed_codex.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the parent, all new chunk files, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
