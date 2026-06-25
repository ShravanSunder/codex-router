# Codex WebSocket Lifecycle And Router Invariant Sweep

Date: 2026-06-25

## Question

The router must behave like an account-selection proxy for Codex, not a second
Codex protocol runtime. The immediate questions were:

1. How does Codex CLI establish and reuse WebSockets?
2. Which codex-router invariants are invented rather than required for account
   routing?

## Codex CLI Lifecycle Findings

Research lane: `019efc1f-de7d-7731-b5db-fb28075003c6`

Source checkout inspected:
`/Users/shravansunder/Documents/dev/open-source/ai-harness/codex`

Findings:

- Codex owns the client/session WebSocket lifecycle.
- WebSocket transport is cached on `ModelClient`; one socket can span multiple
  turns.
- `prewarm_websocket()` opens a real WebSocket and sends a real
  `response.create` with `generate=false`, then waits for completion.
- One active request is sent on a WebSocket at a time through a client-side
  mutex.
- 426 during connect falls back to HTTP for that turn; exhausted retry budget
  makes HTTP fallback sticky for the Codex session.
- Codex treats binary frames or server close before `response.completed` as
  client-visible errors.
- Codex, not the router, owns idle/request send/receive timeout policy.
- No Codex-side invariant supports a router-side 250ms first-frame timeout.

## Router Invariant Findings

Research lanes:

- `019efc20-04c2-73f0-a48c-087556dbfba7`
- `019efc20-3388-7080-8adf-a5f410409479`
- `019efc20-5b3b-7b50-a0d9-d6b05efd3ccf`

Acceptable router-owned boundaries:

- Local auth/path/header checks before accepting or proxying traffic.
- Account selection once per HTTP request or once per WebSocket, after the first
  application data frame needed to route.
- Upstream credential injection and local auth header stripping.
- Previous-response affinity and quota/account selection at request or
  WebSocket creation boundaries.

Invented release-path invariants to remove or forbid:

- Fixed pre-upstream first-frame timeout.
- Synthetic app-level failure semantics for transport reset after a prior
  `response.completed`.
- Router-owned closure of the WebSocket merely because a
  `response.completed` frame was forwarded.
- Release CLI/runtime `max-websocket-upstream-messages` cap.
- Re-selection/reconnect loops inside one established WebSocket.

## Implementation Decision

The release router must use the async Hyper/Tokio WebSocket path as the
production path. The old blocking tunnel is test-only.

The async tunnel now:

- Waits indefinitely for the first data frame unless the client closes or router
  shutdown is requested.
- Responds to pre-upstream ping with pong and ignores pong/frame control traffic
  without selecting an account or opening upstream.
- Opens upstream only after the first text/binary data frame is received.
- Forwards both directions until a peer closes, revocation fires, or runtime
  shutdown fires.
- Records `response.completed` for metrics only; it does not close the socket.
- Treats reset-without-close as clean transport closure rather than router-owned
  Codex request failure.

## Proof Gates Run During Patch

- `cargo check -p codex-router-proxy`
- `cargo check -p codex-router-cli`
- `cargo check -p codex-router-test-support`
- `cargo test -p codex-router-proxy --lib`
- `cargo test -p codex-router-proxy async_forwarding_tests -- --nocapture`
- `cargo test -p codex-router-proxy async_websocket_tunnel_handles_control_frames_before_first_data_without_selection -- --nocapture`
- `cargo test -p codex-router-proxy async_websocket_tunnel_forwards_first_frame_and_second_local_frame -- --nocapture`
- `cargo test -p codex-router-proxy loopback_router_runtime_keeps_websocket_preconnect_open_until_client_close -- --nocapture`
- `cargo test -p codex-router-proxy loopback_router_runtime_drains_affinity_tasks_after_handler_error -- --nocapture`
- `cargo test -p codex-router-cli quota_status -- --nocapture`
- `tests/smoke/quota_status_fixture.sh`
- `cargo fmt --all -- --check`

Open proof note:

- `python3 scripts/check-release-runtime-guardrails.py G-23` currently fails
  before commit because that guard requires guarded source paths to be clean
  relative to `HEAD`.
