# Codex Router Quota/OAuth Runtime Goal Details

Goal id: `2026-06-22-codex-router-quota-oauth-runtime`

## Objective

Deliver the Codex Router quota runtime, status UX, credential substrate, and OAuth readiness work through the full lifecycle, starting with adversarial plan review and ending at PR-ready proof, not merge.

## Current Workflow

- Current workflow: `shravan-dev-workflow:orchestrator-goal`
- Latest completed workflow: `shravan-dev-workflow:plan-review-swarm`
- Phase result: `complete`
- Next workflow: `shravan-dev-workflow:implementation-execute-plan`
- First implementation target: Plan 1A Gate 0 dirty-tree/fresh-worktree receipt,
  then Plan 1A T1 credential/state substrate execution.

## Required Reading

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-restructure-ledger.md`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/restructure-scope-and-proof-fit.md`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/restructure-execution-order.md`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/restructure-validation-proof-and-security.md`

## Scope

- Review and harden the plan before code implementation.
- If plan review accepts findings, route back to `plan-creation-swarm` for plan revision.
- If plan review is ready, route to `implementation-execute-plan`.
- Implementation remains in scope after review unless the user explicitly narrows the goal.

## Non-Goals

- Do not implement during plan review.
- Do not merge.
- Do not perform live OAuth/quota proof without explicit approval.
- Do not implement Plan 2 OAuth/device-code login until a separate reviewed Plan 2 exists.
- Do not remove or defer WebSocket proof from v1 without reviewed scope change.

## Requirements/Proof Matrix

- Spec/design gate: proof source: spec file and post-restructure child plans; evidence source: parent-run plan review; freshness guard: current branch and full file coverage.
- Plan matrix gate: proof source: `plan-review-swarm` verdict; evidence source: review report and lane artifacts; freshness guard: current reviewed plan commit `918db95`.
- Implementation proof gate: proof source: Plan 1A and Plan 1B validation gates; evidence source: future `implementation-execute-plan`; freshness guard: command outputs from current checkout.
- Implementation review gate: proof source: `implementation-review-swarm`; evidence source: review report and accepted/rejected findings; freshness guard: current diff/commit under review.
- PR readiness gate: proof source: `implementation-pr-wrapup`; evidence source: PR checks, comments, review-thread state, mergeability; freshness guard: fresh GitHub/CI state.

## Stop Condition

Goal completes only when implementation is complete, required proof gates pass or are explicitly not-applicable, implementation review findings are resolved or explicitly rejected, PR is created/updated and proven ready, and merge is not performed unless explicitly authorized.

## Blocked Condition

Blocked only if the same material blocker recurs under host blocked-state rules, such as missing/contradictory goal pointers, inaccessible repo/remote preventing required PR-ready proof, or user approval required for live proof that is necessary for the next transition.

## Checkpoint Rhythm

- Record orchestrator transitions in `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/events.jsonl`.
- Commit scoped durable artifacts at verified lifecycle checkpoints.
- Keep `tmp/` lane artifacts local unless explicitly promoted.
- Phase skills must return `phase_result`, `evidence`, `recommended_next_workflow`, and `recommended_transition_reason`.

## Latest Review Receipt

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-post-restructure.md`
- Verdict: `needs_revision`
- Accepted direction: revise plans before implementation.
- Highest-risk repairs: OAuth/device-code/keyring scope, same-turn affinity, exact runnable proof rows, smoke-test reality, dirty-tree isolation, Plan 1A/1B sequencing, WebSocket write scope, credential-resolver bypass guards, audit JSONL redaction, route-band invalidation, quota-refresh one-writer behavior, and local bearer-token lifecycle proof.

## Latest Plan-Creation Receipt

- Plan revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`
- Revised artifacts:
  - `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
  - `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Verdict: `complete`
- Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`
- Accepted direction: adversarially review the revised plans before implementation.

## Latest Plan-Review Receipt

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-revision.md`
- Reviewed commit: `e9e9e4a0ddf9e9c33d0bbbfdcc6f09079008f9c8`
- Verdict: `needs_revision`
- Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`
- Accepted direction: revise plans before implementation.
- Highest-risk repairs: exact runnable proof rows, per-scenario installed smoke including hostile no-token, supported-route protocol proof, credential resolver ownership, replay-safe affinity substrate, selector-owned durable quota input, quota runtime ownership, dirty-tree checkpoint receipts, tmp-state carry-forward, family-atomic response alias publication, cross-process quota refresh stale-owner recovery, local bearer lifecycle proof, and audit append-failure policy.

## Latest Plan-Creation Receipt After Review

- Plan revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`
- Lane artifacts:
  - `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/revision-codebase-boundary.md`
  - `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/revision-validation-proof.md`
  - `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/revision-execution-order.md`
  - `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/revision-security-reliability.md`
- Revised artifacts:
  - `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
  - `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Commit: `460b51e docs: tighten quota runtime proof plans`
- Verdict: `complete`
- Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`
- Push status: blocked by remote `https://github.com/shravan-agent/codex-router.git/` returning repository not found.

## Latest Plan-Review Receipt After `460b51e`

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-460b51e.md`
- Reviewed commit: `460b51ec1c85dac3bbb0392ed72d78584740ff04`
- Verdict: `needs_revision`
- Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`
- Accepted direction: revise plans before implementation.

## Latest Plan-Creation Receipt After `460b51e` Review

- Plan revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`
- Revised artifacts:
  - `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
  - `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Commit: `8fb965f docs: close quota plan review gaps`
- Verdict: `complete`
- Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`
- Push status: blocked by remote `https://github.com/shravan-agent/codex-router.git/` returning repository not found.
- Accepted direction: adversarially review the revised plans before implementation.

## Latest Plan-Review Receipt After `8fb965f`

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-8fb965f.md`
- Reviewed commit: `8fb965fa80b0067f6bf062051b8c3726aa913511`
- Verdict: `needs_revision`
- Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`
- Accepted direction: revise plans before implementation.
- Highest-risk repairs: account/quota source-base story, precommit auth/quota rotation, resolver-bypass allowlists, selector/resolver token boundary, `/v1/models` query proof, WebSocket invalid-token proof, live-proof stale gate, smoke transcript redaction, restart replay safety, audit diagnostics, backend-neutral trait ownership, core package gate, and Plan 1B task-to-file ownership.

## Latest Plan-Creation Receipt After `8fb965f` Review

- Plan revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`
- Revised artifacts:
  - `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
  - `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Commit: `734554d docs: resolve quota plan review blockers`
- Verdict: `complete`
- Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`
- Push status: blocked by remote `https://github.com/shravan-agent/codex-router.git/` returning repository not found.
- Accepted direction: adversarially review the revised plans before implementation.

## Latest Plan-Review Receipt After `734554d`

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-734554d.md`
- Reviewed commit: `734554df3329682d40d8495ca091e6ca6e326cf5`
- Verdict: `needs_revision`
- Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`
- Accepted direction: revise plans before implementation.
- Highest-risk repairs: exact-test preflight false-greens, installed-Codex generated-profile/token/WebSocket smoke gate, profile activation/write proof, selector projection ownership, quota provider-client ownership, quota lease owner, old proxy selector token carrier proof, proxy module write ownership, previous-response restart/unknown-owner contract, unsupported WebSocket route fail-closed proof, WebSocket first-frame non-affinity proof, live OAuth current-gate parsing, and backend construction structural proof.

## Latest Plan-Creation Receipt After `734554d` Review

- Plan revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`
- Revised artifacts:
  - `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
  - `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Commit: `918db95 docs: address quota plan review findings`
- Verdict: `complete`
- Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`
- Accepted direction: adversarially review the revised plans before implementation.

## Latest Plan-Review Receipt After `918db95`

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-918db95.md`
- Reviewed commit: `918db95d06d4578ee604b097d49f9b2837a11368`
- Verdict: `needs_revision`
- Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`
- Accepted direction: revise plans before implementation.
- Highest-risk repairs: Plan 1A exact-one helper proof, Plan 1B structural
  guard freshness after proxy/selection edits, auth/quota acyclic ownership,
  T9 replay/previous-response ownership, mechanically auditable checkpoint
  write surfaces, A1/A2 resolver gate ordering, exact commands for proof rows,
  route/local-auth helper write surfaces, and quota lease owner clarity.

## Latest Plan-Creation Receipt After `918db95` Review

- Plan revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`
- Reviewed input: `918db95d06d4578ee604b097d49f9b2837a11368`
- Revised artifacts:
  - `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
  - `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Commit: `e7b55fc31c7b7a53507eacd8c87ef0201729ba17`
- Verdict: `complete`
- Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`
- Push status: blocked by remote `https://github.com/shravan-agent/codex-router.git/` returning repository not found.
- Accepted direction: adversarially review the revised plans before implementation.
- Repairs folded: Plan 1A exact-one helper proof, Plan 1B structural freshness
  rerun, acyclic quota/auth ownership, state-owned T9 replay and
  previous-response ownership, exact task write surfaces, A1/A2 resolver gate
  ordering, exact commands for proof rows, route/local-auth helper paths, exact
  profile header proof, and state-backed quota refresh lease ownership.

## Latest Plan-Review Receipt After `e7b55fc`

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-e7b55fc.md`
- Reviewed commit: `e7b55fc31c7b7a53507eacd8c87ef0201729ba17`
- Verdict: `needs_revision`
- Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`
- Accepted direction: revise plans before implementation.
- Highest-risk repairs: exact-one helper command false-greens, quota lease
  structural proof forbidding state API names, final structural rows using
  future/missing paths unsafely, missing `auth/src/lib.rs` write scope,
  WebSocket previous-response restart/unknown-owner proof, live selector
  adapter auditability, selection/state replay split, and live-gate
  approval-key parsing.

## Latest Plan-Creation Receipt After `e7b55fc` Review

- Plan revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`
- Reviewed input: `e7b55fc31c7b7a53507eacd8c87ef0201729ba17`
- Revised artifacts:
  - `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
  - `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
  - `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Commit: `05bf7553ac5ad3a164dc6b842afbf8415d560845`
- Verdict: `complete`
- Recommended next workflow: `shravan-dev-workflow:plan-review-swarm`
- Push status: blocked by remote `https://github.com/shravan-agent/codex-router.git/` returning repository not found.
- Accepted direction: adversarially review the revised plans before implementation.
- Repairs folded: fail-closed exact-one helper commands, state-backed quota
  lease structural proof, explicit factory/adapter path existence checks,
  `auth/src/lib.rs` quota-client migration scope, WebSocket previous-response
  restart/unknown-owner row, A2 live selector adapter proof, selection
  turn-state/affinity codec-only boundary, and stricter live-gate approval-key
  rejection.

## Latest Plan-Review Receipt After `05bf755`

- Review artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-05bf755.md`
- Reviewed commit: `05bf7553ac5ad3a164dc6b842afbf8415d560845`
- Verdict: `ready_with_parent_led_exception`
- Recommended next workflow: `shravan-dev-workflow:implementation-execute-plan`
- Accepted direction: start implementation at Plan 1A Gate 0 only; no product
  code was implemented during review.
- Swarm exception: four read-only subagents were previously spawned for this
  review shape, but the host hit `Too many open files` and the user asked about
  agent behavior. The parent shut the agents down and completed a parent-led
  read-only review instead of accepting partial lane output.
- Preserved constraints: fresh-worktree or dirty-tree carry-forward receipt is
  required before code edits; Plan 1A must complete validation and
  implementation-review before Plan 1B starts; Plan 2 OAuth/device-code/keyring
  login remains out of scope until a separate reviewed Plan 2 exists; live
  OAuth/quota proof remains approval-gated; PR-ready proof remains blocked until
  remote push/PR access is fixed.
