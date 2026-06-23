# Security + WebSocket Threat Model Lane

Status: answered
Verdict: needs revision
Coverage: full 837-line spec reviewed; security/WebSocket sections and current
proxy/state/auth anchors inspected.

## Findings

- Blocker: previous-response affinity does not define committed owner lookup,
  missing-owner behavior, disabled/unauthenticated owner behavior, restart
  behavior, or whether fallback is ever allowed.
- Important: unsupported/realtime/unknown WebSocket route taxonomy is
  underspecified. V1 should collapse all non-`/v1/responses` paths to
  `unsupported_path` or define distinct classes.
- Important: JSON schema still exposes `account_label` without guaranteeing it
  is sanitized. The schema should use `safe_account_label` or define identical
  sanitization semantics.

Completion receipt: answered, with target spec, R2 ledger, and checkpoint code
anchors for server, websocket, routes, headers, repositories, affinity, and
account metadata.
