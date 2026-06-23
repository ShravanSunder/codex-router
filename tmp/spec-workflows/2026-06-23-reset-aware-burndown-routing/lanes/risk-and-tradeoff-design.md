# Lane: risk-and-tradeoff-design

Status: candidate evidence, parent-verified in `swarm-ledger.md`

Accepted risks:

- Raw minimum headroom loses reset geometry.
- "Earliest reset wins" is too blunt.
- Weekly quota far from reset must dominate short-window salvage.
- Unknown quota must not look better than known healthy quota.
- Runtime and status UI must not use different definitions of limiting window or pressure.

Required scenario probes:

- low 5h, healthy weekly, 5h resets soon
- low weekly, weekly resets soon
- weekly empty
- same weekly, different short reset pressure
- unknown versus known healthy

Design implication:

The spec needs bounded reset salvage and structured selection explanations.
