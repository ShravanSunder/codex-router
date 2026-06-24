# Execution Receipt: T4e Hyper/Tokio Runtime Cutover And SSE Streaming

Timestamp: 2026-06-24T15:26:18Z
Base HEAD before commit: 0d08350b8bc62aff9e6389d3de118203333610fa

## Scope

This checkpoint removes the release-selected hand-written local WebSocket
upgrade helper from the production server path and routes local WebSocket
upgrades through Hyper plus `hyper-tungstenite`.

It also changes the assembled loopback runtime to own a Tokio runtime and async
listener directly, and changes the local Hyper HTTP/SSE response path to stream
blocking upstream chunks through a bounded Tokio channel instead of buffering the
full response before returning a Hyper response.

## Requirements Addressed

- R1/T1: production `serve` is owned by Tokio runtime machinery.
- R3/T4: local WebSocket validation and `101 Switching Protocols` response are
  owned by Hyper/`hyper-tungstenite`, not a router accept-key helper.
- R4/T3 partial: local Hyper HTTP/SSE path streams bytes before upstream EOF.
- T7 smoke preservation: installed Codex mock smoke still traverses the child
  `codex-router serve` path for HTTP/SSE and WebSocket.

## Changed Files

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/lib.rs`

## Proof Commands

```text
cargo test -p codex-router-proxy assembled_loopback_router_runtime_streams_sse_before_upstream_eof -- --nocapture
exit: 0
```

```text
cargo test -p codex-router-proxy assembled_loopback_router_runtime_forwards_with_repository_state_and_secrets -- --nocapture
exit: 0
```

```text
cargo test -p codex-router-proxy authenticated_http_proxy_records_streaming_sse_response_id_after_body_read -- --nocapture
exit: 0
```

```text
cargo fmt --all -- --check && cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings
exit: 0
```

```text
cargo test --workspace -- --nocapture
exit: 0
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent
exit: 0
observation: three installed Codex websocket clients shared one router PID and overlapped successfully.
```

```text
tests/smoke/installed_codex_mock.sh --transport all
exit: 0
observation: 6 installed-Codex mock smoke tests passed for HTTP/SSE, WebSocket, hostile tokenless preflight, and harness inventory.
```

## Explicit Open Gaps

- T3 is not complete: upstream HTTP/SSE transport still uses the blocking
  `HttpUpstreamTransport` implementation and `httparse`.
- T5 is not complete: registry counters exist, but final close/revocation and
  child-process evidence export still need the remaining matrix rows.
- T6 is not complete: permanent structural guardrails still need real
  release-reachability checks.
- T8 is not complete: five-minute three-installed-Codex soak and final cleanup
  artifact are still required before PR-ready.

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T4e-hyper-tungstenite-runtime-cutover-and-sse-streaming.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Continue implementation with the next unproven proof slice; pure Hyper upstream and final structural/soak gates remain open.
