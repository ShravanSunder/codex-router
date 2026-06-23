# Quota Burn-Down Routing Plan Ledger

Date: 2026-06-23
Status: parent plan synthesis

## Source Inputs

- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/swarm-ledger.md`
- `tmp/workflow-state/2026-06-23-quota-burndown-routing/details.md`
- live repo evidence from `crates/codex-router-selection`,
  `crates/codex-router-proxy`, `crates/codex-router-cli`,
  `crates/codex-router-state`, and `crates/codex-router-test-support`

## Current Product Correction

Unknown quota is not fallback capacity.

Unknown/no-data/missing-reset accounts are `probe_required`. They never enter
weighted routing, even when every account is unknown. The router may schedule or
signal background probe work, but startup and request routing must not wait for
that provider I/O. Only later persisted successful probe/refresh results can
make the account usable.

## Plan Artifact

Primary plan:
`tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`

## Plan Lanes

This plan was written by the parent without launching fresh subagents because
the user explicitly requested forward movement after repeated long review loops.
The plan still names the substantial lanes and write scopes:

- pure burn-down assessment
- proxy adapter and runtime routing
- WebSocket routing contract
- quota status UX
- background probe/refresh persistence
- installed Codex e2e proof

## Verification Performed During Planning

- confirmed current spec is 1174 lines after probe-required correction
- inspected live file surfaces with `rg` and `rg --files`
- confirmed existing installed Codex/WebSocket smoke support exists in
  `crates/codex-router-test-support/src/installed_codex.rs`
- confirmed existing background refresh worker exists in
  `crates/codex-router-cli/src/quota.rs`

## Next Route

Recommended next skill:
`shravan-dev-workflow:plan-review-swarm`

If the plan review has no accepted blockers, route to:
`shravan-dev-workflow:implementation-execute-plan`

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`, `tmp/plan-workflows/2026-06-23-quota-burndown-routing/plan-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:plan-review-swarm`
recommended_transition_reason: Corrected spec is now mapped to concrete implementation lanes and proof gates.
