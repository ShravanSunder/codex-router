# Lane: architecture-clean-boundary

Status: candidate evidence, parent-verified in `swarm-ledger.md`

Core finding:

Add a pure burn-down assessment boundary between persisted selector windows and both runtime selection and quota status UI.

Accepted boundary:

```text
persisted selector windows
  -> burn-down assessment
  -> proxy selection adapter
  -> weighted deficit selector

burn-down assessment
  -> quota status renderer
```

Forbidden edges:

- CLI must not own routing math.
- `WeightedDeficitSelector` must not know quota windows.
- burn-down assessment must not know terminal formatting or OAuth secrets.

Design implication:

Keep weighted fairness generic and feed it risk-adjusted scalar weights plus structured explanations.
