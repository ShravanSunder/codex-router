# R19 WebSocket Security And Harness Lane

Verdict: ready

What held:

- Invalid WebSocket local auth and unsupported WebSocket paths require
  handshake/connect failure or non-101 local rejection.
- First-frame validation remains bounded and allowlisted.
- Installed-Codex e2e requires HTTP/SSE and WebSocket with transport-specific
  local-auth receipt fields.
- Redaction forbids raw token/header, raw `previous_response_id`, raw body, and
  full WebSocket frame leakage.

Receipt:

- Source anchors: spec lines 425-433, 1439-1486, 1535-1625, 1661-1683,
  1890-1945; R18 review ledger; R19 revision ledger; `server.rs`,
  `websocket.rs`, `http_sse.rs`, installed-Codex harness.
- Parent reducer wrote this lane summary from the subagent candidate output.
