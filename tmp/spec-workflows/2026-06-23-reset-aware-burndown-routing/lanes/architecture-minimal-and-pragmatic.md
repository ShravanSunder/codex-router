# Lane: architecture-minimal-and-pragmatic

Status: partially accepted candidate evidence

Accepted:

- Do not build a forecasting engine.
- Do not model EWMA, token cost, or future session scheduling in this pass.
- Keep the first implementation to pre-request choice from known persisted quota state.

Rejected:

- Pure "earliest reset wins" as the primary policy.

Reason:

Earliest reset alone can choose a nearly empty or weekly-dangerous account. The accepted design is pressure-aware with bounded reset salvage.
