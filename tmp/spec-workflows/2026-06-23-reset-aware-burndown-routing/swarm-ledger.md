# Reset-Aware Burn-Down Routing Spec Ledger

Date: 2026-06-23
Status: revised parent synthesis after spec-review findings

## Source Inputs

- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-selection/src/weighted_deficit.rs`
- `crates/codex-router-selection/src/eligibility.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-cli/src/quota.rs`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- `MEMORY.md:2455-2541` for recovered prior context; treated as secondary to live repo evidence
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/review-ledger.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/*.md`

## Lanes Run

| Lane | Agent | Status | Accepted Evidence |
| --- | --- | --- | --- |
| codebase-explorer | Parfit | answered | Accepted: selector reads persisted per-window input but collapses it to minimum headroom before weighted selection. |
| architecture-clean-boundary | Banach | answered | Accepted: burn-down assessment should be pure and shared by selector and CLI; `WeightedDeficitSelector` should stay generic. |
| risk-and-tradeoff-design | Faraday | answered | Accepted: the spec must decide bounded reset salvage and must upgrade selection explanations beyond freshness labels. |
| ux-api-cli-surface | Curie | answered with local evidence gap | Partially accepted: product language and display constraints; local-code claims rejected until parent verification. |
| architecture-minimal-and-pragmatic | Boyle | answered with local evidence gap | Partially accepted: avoid forecasting engine; rejected "earliest reset wins" as too blunt. |

## Parent Verification

Accepted direct observations:

- `account_state_from_selector_input` rejects ineligible windows and reduces multi-window selector data to minimum remaining headroom. Source: `crates/codex-router-proxy/src/account_selection.rs:262-292`.
- `WeightedDeficitSelector` only consumes account id and scalar weight. Source: `crates/codex-router-selection/src/weighted_deficit.rs:66-98`.
- persisted selector windows contain reset time, observed time, window length, status, and remaining headroom. Source: `crates/codex-router-state/src/quota_snapshot.rs:91-200`.
- CLI status already computes pace and runout from reset time. Source: `crates/codex-router-cli/src/quota.rs:924-1007`.
- existing spec already constrains weekly quota protection and selection-visible reset timing. Source: `docs/specs/2026-06-20-codex-router-greenfield-spec.md:147-151`.
- existing plan already identifies long-window pressure ahead of reset urgency. Source: `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md:322-338`.
- spec review found the first draft under-specified across scoring, dependency
  ownership, thresholds, mixed-window collapse, human status output,
  non-blocking proof, and redaction proof. Source:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/review-ledger.md`.

Rejected or deferred evidence:

- "earliest reset wins" is rejected as the primary policy because it can over-route nearly empty or weekly-dangerous accounts.
- external README/issues from the UX/pragmatic lanes were not accepted as source of truth for this repo. They only support product intuition.
- the dedicated security review lane crashed; the revised spec adds
  surface-by-surface redaction expectations, but the next `spec-review-swarm`
  should rerun a dedicated security/trust-boundary lane.

## Accepted Design Decisions

1. Add a pure burn-down assessment layer over persisted selector windows.
2. Preserve request/startup behavior: no provider quota refresh on startup or request selection.
3. Keep `WeightedDeficitSelector` generic; feed it risk-adjusted weights.
4. Treat long-window pressure as dominant over short-window reset urgency.
5. Allow bounded reset salvage for soon-reset windows only when durable-budget risk is not dangerous, or when the long window itself is imminently resetting.
6. Use structured routing reasons shared by runtime audit and quota status display.
7. Keep default human quota output account-centric and avoid internal score jargon.
8. Put pure assessment in `codex-router-selection::burn_down`; proxy and CLI
   adapt state DTOs into pure assessment DTOs.
9. Use fixed v1 policy constants for near-reset thresholds, reserve thresholds,
   pressure multiplier, salvage caps, and weight clamps.
10. Classify mixed windows with any-window conservative collapse:
    ineligible/exhausted blocks, unknown or missing reset becomes fallback,
    stale marks stale, and `effective` is only an explanation hint.
11. Route by availability pool before weighted fairness:
    `usable`, then `reserve`, then `unknown`, never `blocked`.
12. Make default human status output strict: account label only, Unicode bars
    when supported, no `pp`, no `bottleneck`, no raw score, and selected-next
    explanation when routing is shown.
13. Require black-box non-blocking proof for boot/listen, first routed request,
    and quota status render.
14. Require end-to-end Codex-through-router proof, including WebSocket behavior,
    before implementation completion can be claimed.

## Open Decisions

No product decisions remain open before the next spec review. The next review
may still reject decisions, but plan creation must not reopen them silently.

## Next Route

Recommended next skill: `shravan-dev-workflow:spec-review-swarm`.

Only after review acceptance should orchestrator route to
`shravan-dev-workflow:plan-creation-swarm`.

phase_result: complete
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`, `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/swarm-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-review-swarm`
recommended_transition_reason: Revised spec folds in accepted review findings; next hard gate is adversarial spec review before planning.
