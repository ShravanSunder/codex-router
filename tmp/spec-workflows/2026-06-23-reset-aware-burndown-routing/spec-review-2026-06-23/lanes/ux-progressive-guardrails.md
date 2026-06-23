# Lane: UX, Progressive Disclosure, And Guardrails

Status: answered
Verdict: revise

Accepted candidate findings:

- Default human output contract is too loose.
- Routing/status vocabulary is not frozen tightly enough.
- Human and machine surfaces are not separated hard enough.
- Proof expectations need exact historical bad-case goldens.
- Artifact structure should route downstream agents more explicitly.

Required revision:

- Add normative vocabulary for `blocked`, `reserve`, `usable`, `unknown`, `limiting window`, `pressure`, and `selected next`.
- Split human output, machine schema, mapping notes, forbidden output, proof expectations, and out-of-scope behavior.
- Add golden/snapshot proof for healthy multi-account, limiting-window disagreement, reset-aware selection, unknown/partial data, blocked/reserve/usable, colorless/plain, and negative assertions.
