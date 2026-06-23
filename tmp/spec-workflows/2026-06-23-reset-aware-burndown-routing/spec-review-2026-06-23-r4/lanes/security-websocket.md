# Security + WebSocket Lane R4

Status: answered
Verdict: needs revision

Findings:

- Blocker: WebSocket first-frame contract needed explicit pre-selection
  guardrails: byte cap, wait bound, accepted type, local fields, and upstream
  ownership of full schema validation.
- Important: proof needed a WebSocket-specific canary proving first-frame and
  request-body content are not leaked to audit/log/smoke artifacts.

Completion receipt: full 961-line spec read; R3 ledger and WebSocket code/test
anchors inspected.
