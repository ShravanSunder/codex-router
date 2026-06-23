# R19 Blocker-Closure Lane

Verdict: needs revision

Accepted by parent:

- Blocker: two stale public-surface references still used
  `BurnDownRouteBandAssessment.*` instead of the flat
  `BurnDownRouteBandAssessmentResult` envelope.

What held:

- `unsupported_path` versus `unsupported_route_band` separation is closed.
- Wrong-method black-box proof is explicit.
- Unsupported-route-band JSON is internal/test-only.

Receipt:

- Source anchors: spec lines 1-1963, R18 review ledger, R19 revision ledger,
  `routes.rs`, `http_sse.rs`, `account_selection.rs`, `burn_down.rs`.
- Parent reducer wrote this lane summary from the subagent candidate output.
