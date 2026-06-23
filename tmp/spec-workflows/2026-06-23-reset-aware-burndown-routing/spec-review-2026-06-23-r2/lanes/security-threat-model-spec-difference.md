# Lane: Security Threat Model And Spec Difference

Status: answered
Verdict: needs revision

Accepted candidate findings:

- WebSocket v1 behavior is not preserved as an explicit routing/security
  contract.
- Threat model lacks complete entry points, trust boundaries, and auth ordering
  proof rows.
- Default human account labels need a safe-display contract.
- Smoke/log redaction proof is too narrow for WebSocket and Codex payloads.
- Machine output lifecycle needs an opt-in/local-only boundary.

Parent disposition:

- Accepted and merged into R2-A3 and R2-A5.

Completion receipt: read-only; full 627-line spec coverage reported by lane.
