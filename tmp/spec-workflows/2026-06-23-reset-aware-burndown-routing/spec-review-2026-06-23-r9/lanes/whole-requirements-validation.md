# R9 Lane: Whole Spec + Requirements + Validation

Verdict: ready
Agent: Einstein

## What Held

- R8-A1 held for algorithm and shape: `affinity_key_hash` is full 64-character
  lowercase hex HMAC-SHA-256 with router-owned secret, shared helper ownership,
  no raw persistence, hard schema cutover, no raw-key fallback, and
  duplicate/ambiguous owner rows fail closed.
- R8-A2 held: previous-response owner continuation is valid only for
  `availability=usable` or `availability=reserve`; `unknown`, `blocked`, and
  `excluded` fail closed.
- Prior R7-ready surfaces still held: all supplied accounts for status/proof,
  JSON envelope/redaction, account-centric status, WebSocket first-frame
  allowlist, and local Codex-through-router e2e.

Completion receipt: answered with anchors.
Confidence: high
