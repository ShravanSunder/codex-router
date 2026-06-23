# Contract + Architecture Lane R4

Status: answered
Verdict: needs revision

Findings:

- Blocker: `preferred_next` could disagree with neutral runtime when tie order
  was prose-only and not the exact ordered candidate list consumed by proxy.
- Important: current WebSocket delta needed to state current hardcoded
  `/v1/responses` selection and missing handshake-path classification.

Completion receipt: full 961-line spec read; selector, weighted deficit,
affinity, WebSocket, repository, and CLI code anchors inspected.
