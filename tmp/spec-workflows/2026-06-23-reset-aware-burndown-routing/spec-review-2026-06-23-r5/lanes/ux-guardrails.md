# R5 Lane Receipt: UX + Guardrails

Agent: Carver
Status: answered
Verdict: needs revision

Coverage: read the full 1014-line spec and R4 ledger.

Candidate findings:

- Important: unknown-only fallback lacks a display contract.
- Important: unknown, missing-reset, and no-window rows can recreate fake
  `0% left` output.
- Important: JSON/debug schema omits fields needed to audit the table and
  routing contract.
- Question: e2e guardrail is named but not acceptance-shaped.

Parent disposition: accepted. The spec now defines fallback display, unknown
placeholders, JSON audit shape, and e2e acceptance.
