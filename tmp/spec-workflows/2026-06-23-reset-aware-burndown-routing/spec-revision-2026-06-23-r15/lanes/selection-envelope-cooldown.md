# R15 Lane: Selection Envelope And Cooldown

Agent: Avicenna
Status: accepted by parent

## Finding

The previous spec allowed several route-level shapes to coexist. That made it
too easy for runtime routing, status JSON, unsupported route bands, and unknown
fallback to drift.

## Accepted Spec Change

- Route-level assessment exposes `route_result`, `selected_pool`,
  `selected_pool_reason`, `preferred_next_account_id`, `weighted_candidates`,
  and `accounts`.
- Unsupported route bands use the same envelope with empty candidates/accounts
  and `selected_pool_reason=unsupported_route_band`.
- Unknown quota enters `weighted_candidates` only when known usable/reserve
  pools are empty.
- Runtime cooldown and previous-response affinity are proxy-owned wrappers over
  pure assessment and survive only when the account remains in current
  `weighted_candidates`.

## Evidence Anchors

- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-selection/src/weighted_deficit.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
