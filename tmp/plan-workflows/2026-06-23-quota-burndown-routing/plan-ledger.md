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
weighted routing, even when every account is unknown. Request routing does not
schedule provider probes or wait for provider I/O. Prompt startup and periodic
background refresh/probe are the v1 mechanisms. Only later persisted successful
probe/refresh results can make the account usable.

The plan also adds route-band account-hold cooldown so adjacent normal requests
do not thrash between OAuth accounts. The v1 default hold is 120 seconds, and it
breaks immediately for affinity, exhausted quota, blocked/probe-required state,
disabled accounts, or missing active credentials.

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

- confirmed current spec is 1234 lines after probe-required and account-hold
  cooldown correction
- confirmed current plan is 406 lines after plan-review corrections
- inspected live file surfaces with `rg` and `rg --files`
- confirmed existing installed Codex/WebSocket smoke support exists in
  `crates/codex-router-test-support/src/installed_codex.rs`
- confirmed existing background refresh worker exists in
  `crates/codex-router-cli/src/quota.rs`
- plan-review-swarm lanes accepted blockers and parent folded them into the
  spec/plan:
  `tmp/plan-workflows/2026-06-23-quota-burndown-routing/plan-review/review-ledger.md`

## Next Route

Recommended next skill:
`shravan-dev-workflow:implementation-execute-plan`

First execution action:
run the T0 dirty target-file gate and stop if planned target files are neither
clean nor explicitly adopted.

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`, `tmp/plan-workflows/2026-06-23-quota-burndown-routing/plan-ledger.md`, `tmp/plan-workflows/2026-06-23-quota-burndown-routing/plan-review/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Plan review findings were folded back into the plan and spec; execution can start after the T0 dirty target-file gate.
