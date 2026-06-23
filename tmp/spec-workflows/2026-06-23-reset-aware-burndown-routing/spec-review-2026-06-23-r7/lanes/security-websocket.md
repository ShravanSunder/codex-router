# R7 Lane: Security + WebSocket Protocol

Verdict: ready
Agent: Nash

## What Held

- `/v1/responses` fixes the WebSocket route band to `responses`.
- `/v1/realtime` and unknown WebSocket paths fail closed as `unsupported_path`
  before selection, credential resolution, or upstream open.
- First-frame preselection reads only top-level `type` and top-level
  `previous_response_id` before selection.
- Non-allowlisted first-frame canaries in `model`, `input`, `metadata`,
  `tools`, prompt text, and body content are required to prove no influence on
  route-band selection, logs, traces, or audit before upstream validation.
- Previous-response owner failures fail closed with no weighted fallback.
- WebSocket failure matrix covers zero selector advance, zero credential
  resolver calls, zero upstream auth injection, and zero upstream open.
- Redaction surfaces forbid tokens, auth headers, keychain ids, raw payloads,
  prompts, tool args, and unsafe labels where required.

Completion receipt: answered with anchors.
Confidence: high
