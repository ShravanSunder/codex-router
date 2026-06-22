# Revision Lane: Security Reliability

Lane: `security-reliability`
Status: `answered`
Mode: read-only planning
Confidence: high on required plan edits, medium on operational policy choices

## Evidence Inspected

- Turn-state payload in `crates/codex-router-selection/src/turn_state.rs`
- WebSocket selection and audit paths in `crates/codex-router-proxy/src/websocket.rs`
- HTTP/SSE audit path in `crates/codex-router-proxy/src/http_sse.rs`
- Refresh lease helper in `crates/codex-router-secret-store/src/refresh_lease.rs`
- Quota refresh publication path in `crates/codex-router-cli/src/quota.rs`
- SQLite state and affinity storage surfaces.

## Accepted Candidate Evidence

- T9 needs an affinity design packet defining session/turn scope, expiry,
  nonce/generation, route binding, replay semantics, and previous-response
  commit/freshness lifecycle.
- Local bearer lifecycle proof must split HTTP missing/old token, WebSocket
  missing/old token before upstream open, and rotation-close redacted reason.
- Response-backed aliases must publish as one visible family or recover to the
  prior family across Plan 1A credential mutation and Plan 1B refresh
  success/failure.
- Cross-process quota refresh needs a persisted SQLite lease or cycle-generation
  fence proving owner/follower behavior, stale-owner reclaim, and stale-loser
  write rejection.
- Audit append failure should be best-effort with surfaced redacted diagnostics
  for allowed proxy traffic; local-auth failures still reject. Silent drop is
  not acceptable.

## Parent Synthesis

Folded into:

- Plan 1A T2 and rows `1A-04a`, `1A-04b`, `1A-06a`.
- Plan 1B T6, T7, T9, rows `1B-02a`, `1B-07a`, `1B-13a`, `1B-14a`,
  and `1B-16*`.

Completion receipt: answered; read-only; parent wrote this lane artifact.
