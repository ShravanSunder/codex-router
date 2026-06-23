# R18 Whole-Spec Coverage Lane

Verdict: needs revision

Accepted by parent:

- Blocker: HTTP/SSE routing order contradicted the route inventory by using
  `unsupported_route_band` for raw classifier misses and by not covering all
  routed HTTP APIs.
- Important: routes marked not previous-response capable lacked an explicit
  `previous_response_id` behavior.

What held:

- The R18 route inventory matched current route classification.
- The spec was no longer `/v1/responses`-only for proof.
- Runtime account-selection side effects were materially sharper than R17.
- WebSocket direct-payload validation matched the current safe shape.

Receipt:

- Source anchors: spec lines 1-1992, R17 ledger, R18 ledger, `routes.rs`,
  `account_selection.rs`, `burn_down.rs`, `weighted_deficit.rs`.
- Parent reducer wrote this lane summary from the subagent candidate output.
