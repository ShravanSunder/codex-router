# R18 Contract, Boundaries, And Planning Lane

Verdict: needs revision

Accepted by parent:

- Blocker: shared assessment/result contract was still two-shaped, forcing a
  future planner to choose between an enum payload model and a flat envelope.
- Important: route-band ownership leaked through per-account `route_band`.
- Important/question: unsupported-route-band JSON needed to be marked
  internal/test-only or tied to a real v1 surface.

What held:

- Usable/reserve/unknown/blocked/excluded domains were much sharper.
- Cooldown, affinity, weighted-deficit advancement, and route-band partitioning
  were clearer than R17.
- `unsupported_path` versus `unsupported_route_band` was separated in the route
  inventory.

Receipt:

- Source anchors: spec lines 1-1992, `routes.rs`, `account_selection.rs`,
  `burn_down.rs`, `repositories.rs`, `sqlite.rs`.
- Parent reducer wrote this lane summary from the subagent candidate output.
