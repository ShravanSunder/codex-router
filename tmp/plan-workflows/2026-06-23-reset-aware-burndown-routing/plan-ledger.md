# Plan Ledger: Reset-Aware Burn-Down Routing

Date: 2026-06-23
Workflow: `shravan-dev-workflow:plan-creation-swarm`
Branch: `main`
Head at plan creation: `0bde7ae`

## Source Coverage

- Accepted spec:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
  (`1971` lines), read in chunks 1-500, 501-1000, 1001-1500, and
  1501-1971.
- R20 review ledger:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r20/review-ledger.md`
  (`67` lines), verdict ready.
- Workflow state:
  `tmp/workflow-state/2026-06-23-quota-burndown-routing/details.md` and
  `events.jsonl`, latest transition to `plan-creation-swarm`.

## Baseline Proof

- `cargo check --workspace` passed on 2026-06-23 from repo root.
- No unit/integration/smoke/e2e proof for the R20 behavior has been claimed.

## Lane Outputs

| Lane | Agent | Status | Artifact |
| --- | --- | --- | --- |
| codebase-boundary | Schrodinger (`019ef65d-e376-76d3-b9b9-42d8e2fa3da5`) | answered | `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/lanes/codebase-boundary.md` |
| validation-proof | Pasteur (`019ef65e-233d-7912-9c9d-eeabccb6b514`) | answered | `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/lanes/validation-proof.md` |
| execution-order | Poincare (`019ef65e-577e-71f2-8b9d-5e7e94bf2110`) | answered | `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/lanes/execution-order.md` |
| security-reliability | Kuhn (`019ef65e-a357-7df3-bd97-032a39e9e6b9`) | answered | `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/lanes/security-reliability.md` |

## Parent Decisions

1. Accepted the R20 spec as the implementation source of truth.
2. Kept OAuth/keychain account-management out of this plan because the spec
   explicitly marks live OAuth/keychain work as non-goal for this burn-down
   routing goal.
3. Required route-native black-box proof and installed-Codex HTTP plus
   WebSocket e2e proof before any complete claim.
4. Required WebSocket proof to include installed-Codex generated-profile bearer
   auth, preselection call-order failures, selected-account pinning, and
   redacted first-frame transcript artifacts.
5. Required state refresh proof to demonstrate non-blocking startup, first
   request, and status render from persisted SQLite rows.
6. Required redaction proof over produced artifacts, not only source-level
   assertions.
7. Chose `plan-review-swarm` as the next workflow before implementation.

## Accepted Plan Artifact

- `tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/implementation-plan.md`

## Completion Receipt

Phase result: complete

Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`

Recommended transition reason: the R20 spec has a written implementation plan
with task sequence, write scopes, execution DAG, proof matrix, security gates,
and split/replan triggers; the plan has not yet been adversarially reviewed.
