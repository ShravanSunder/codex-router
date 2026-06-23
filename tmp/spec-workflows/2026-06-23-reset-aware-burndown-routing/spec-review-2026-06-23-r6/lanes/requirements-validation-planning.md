# R6 Lane Receipt: Requirements + Validation + Planning Readiness

Agent: Pauli
Status: answered
Verdict: needs revision

Coverage: read the full 1106-line spec and R5 ledger.

Candidate finding:

- Important: WebSocket first-frame local-field allowlist proof is required, but
  the allowed local fields are under-specified.

Parent disposition: accepted. The spec now says the path fixes the route band
and only top-level `type` plus top-level `previous_response_id` may be read
before selection.
