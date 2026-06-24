# Execution Receipt: T3 Hyper HTTP/SSE Upstream Transport

Timestamp: 2026-06-24T15:48:37Z
Base HEAD before commit: 7749909451313c659f9f28e1a209d0e73024956c

## Scope

This checkpoint moves the release `codex-router serve` HTTP/SSE upstream path
off the blocking/manual transport and onto a Hyper-backed async streaming
transport.

The release runtime now prepares auth, selection, credential injection, and
audit metadata on the blocking pool, then opens the upstream request through
`HyperHttpUpstreamTransport` and streams the Hyper response body directly back
to the local Hyper response body. The old `HttpUpstreamTransport` with
`TcpStream`, `httparse`, and `reqwest::blocking` remains available only under
`#[cfg(test)]` for legacy unit coverage.

HTTP/SSE response-owner affinity recording now observes async body frames and
spawns the SQLite write as a bounded side effect instead of blocking body
forwarding progress.

## Requirements Addressed

- T3/R2: release HTTP/SSE upstream path uses Hyper/hyper-util client transport.
- T3/R4: HTTP/SSE body is streamed through Hyper body types without buffering
  the full upstream response before local response commit.
- T3/R4/T5 seed: affinity side effect no longer gates body forwarding progress
  on the async path.
- G-02/G-04/G-05 partial: blocking upstream implementation is test-only rather
  than release-selected from `serve`.
- S/T7 preservation: installed Codex mock smoke still passes through the child
  `codex-router serve` path after the transport cutover.

## Changed Files

- `Cargo.lock`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-proxy/src/upstream.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/lib.rs`
- `crates/codex-router-cli/src/lib.rs`

## Notes On Test Fixture Changes

Some pre-existing mock upstream fixtures used `read_to_string`, which depended
on the old raw upstream transport half-closing its write side. Hyper-managed
connections do not require that EOF shape. Those fixtures now read the HTTP
request by headers and `Content-Length`. Local test clients that still read to
EOF send `Connection: close` explicitly.

## Proof Commands

```text
cargo check -p codex-router-proxy
exit: 0
```

```text
cargo test -p codex-router-proxy assembled_loopback_router_runtime_forwards_with_repository_state_and_secrets -- --nocapture
exit: 0
red/green: initially hung on stale EOF-based mock upstream read; then failed because async affinity recording was checked before the side-effect completed; fixed by Content-Length request read and bounded owner-row wait.
```

```text
cargo test -p codex-router-proxy -- --nocapture
exit: 0
result: 107 passed, 0 failed.
```

```text
cargo test -p codex-router-cli serve_command_starts_runtime_and_forwards_one_loopback_request -- --nocapture
exit: 0
```

```text
cargo test -p codex-router-cli serve_command_reloads_token_rotation_without_restart -- --nocapture
exit: 0
```

```text
cargo fmt --all -- --check && cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings
exit: 0
```

```text
cargo test --workspace -- --nocapture
exit: 0
result: workspace unit/doc tests passed; codex-router-test-support ignored/manual smoke rows remained ignored as intended.
```

```text
tests/smoke/installed_codex_mock.sh --transport all
exit: 0
result: 6 installed-Codex mock smoke tests passed for HTTP/SSE, WebSocket, hostile tokenless preflight, and harness inventory.
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent
exit: 0
result: three installed Codex websocket clients shared one router PID and overlapped successfully.
```

## Explicit Open Gaps

- T2 remains partial: state and credential prep still uses synchronous
  repository/resolver work behind `spawn_blocking`; full SQLx request-time
  contract cutover remains open.
- T5 remains open: registry child-process evidence export, revocation, close
  families, and slow-sink pump proof still need the remaining rows.
- T6 remains open: permanent structural guardrails must enforce release
  reachability, including test-only exceptions.
- T8 remains open: five-minute three-installed-Codex soak and cleanup artifact
  are still required before PR-ready.

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T3-hyper-http-sse-upstream-transport.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Continue implementation with T5/T6/T8 proof rows; HTTP/SSE upstream release transport is now Hyper-owned but final guardrails and soak remain open.
