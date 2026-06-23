# R20 Blocker-Closure And Selector-Order Lane

Verdict: ready

Accepted by parent:

- No failure-path findings were accepted.
- R20 closes the R19 flat-envelope and assessment-before-affinity blockers.

What held:

- Active spec references now use `BurnDownRouteBandAssessmentResult` instead
  of the stale `BurnDownRouteBandAssessment.*` public surface.
- HTTP/SSE builds a shared `BurnDownRouteBandAssessmentResult` before
  route-scoped affinity handling.
- WebSocket builds the reset-aware `responses` assessment before route-scoped
  affinity handling.
- The selector ordering is consistent with the current flow where route-band
  assessment computes the candidate pool before previous-response affinity is
  considered.
- Affinity reuse remains a routing hold and does not advance weighted fairness.

Implementation proof still required:

- Add contract tests for the flat selector envelope.
- Add integration tests proving assessment happens before affinity for HTTP/SSE
  and WebSocket paths.
- Add black-box tests for route-native behavior and installed-Codex HTTP plus
  WebSocket traffic.

Receipt:

- Source anchors: spec lines 1-1971, R19 review ledger, R20 revision ledger,
  `routes.rs`, `http_sse.rs`, `account_selection.rs`, `burn_down.rs`, and
  WebSocket tunnel/server anchors.
- Parent reducer wrote this lane summary from the focused subagent candidate
  output.
