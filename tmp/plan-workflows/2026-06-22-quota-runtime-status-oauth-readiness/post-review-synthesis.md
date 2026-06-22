# Post-Review Synthesis

Date: 2026-06-22
Target spec: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
Target plan: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`

## Coverage

- Spec loaded by controller: `497` lines, chunks `1-170`, `171-340`, `341-497`.
- Plan loaded by controller before review: `498` lines, chunks `1-170`, `171-340`, `341-498`.
- Final review lanes reported end-to-end review of both artifacts.

## Final Review Lanes

| Lane | Agent | Verdict |
| --- | --- | --- |
| spec-compliance-and-requirements | Carson (`019eeec5-75f4-7582-8f6a-3c4859890a8e`) | needs revision |
| testability-validation | Jason (`019eeec5-ad6e-7fc2-ab78-2c85269b8c0d`) | needs revision |
| security-reliability | Gibbs (`019eeec5-ec07-7f81-b991-71ab9e93b83f`) | needs revision |
| execution-scope-and-adversarial-design | James (`019eeec6-226c-70e2-bf17-9c3e0ce13abf`) | needs revision |

## Accepted Findings

- Original plan proof matrix was too thin and did not satisfy the spec-required proof contract.
- Original plan lacked checkbox-style execution/proof tracking.
- Plan 2 was presented as a sketch but not clearly non-executable.
- Imported-token refresh needed to cover normal HTTP/SSE and WebSocket egress, not only quota refresh.
- Immediate refresh proof needed to target the non-zero interval production path.
- Failure taxonomy needed to land before immediate refresh, so transient failures do not destroy last-known snapshots.
- Validation needed `cargo nextest`, `cargo deny`, `cargo audit`, and explicit closeout receipt expectations.
- Unknown/no-snapshot accounts needed a not-free-capacity proof row.
- Weekly selector state needed an explicit durable source decision and migration/repository proof.
- Smoke proof needed named scenarios, not just one wrapper command.
- Current import examples needed `--auth-json`, not `--path`.
- Future OAuth/device-code Plan 2 needed logout/purge included before it can be complete.

## Plan Edits Applied

- Replaced the plan with a revised Plan 1A/Plan 1B structure.
- Added checkboxes to scope, tasks, proof gates, validation gates, risks, and replan triggers.
- Marked Plan 2 as a non-executable sketch pending its own reviewed plan.
- Added a unified credential resolver task covering quota refresh, HTTP/SSE, and WebSocket egress.
- Reordered failure taxonomy before immediate startup refresh.
- Added non-zero interval immediate-refresh and scheduled-refresh proof rows.
- Added `cargo nextest`, `cargo deny`, `cargo audit`, `cargo fmt --all --check`, and closeout receipt requirements.
- Expanded the proof matrix to include requirement id, source, owner, proof layer, fixture/mock, command, expected observation, evidence source, proof owner, stale-proof guard, and red/green expectation.
- Added deferred full-spec rows for out-of-scope requirements.
- Added named smoke case requirements.
- Updated future import command example to use `--auth-json`.
- Added future `account logout`/secret purge to Plan 2 requirements.

## Mechanical Validation

- `git diff --check`: passed after plan rewrite.

## Remaining State

- Product code was not changed in this review/revision pass.
- The revised plan is awaiting targeted post-edit verification from agent Maxwell (`019eeed0-a449-7991-a1af-fb1ef771bf90`).
