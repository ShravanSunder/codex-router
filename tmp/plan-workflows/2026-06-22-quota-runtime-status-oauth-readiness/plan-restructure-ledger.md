# Plan Restructure Ledger

Date: 2026-06-22
Workflow: `shravan-dev-workflow:plan-creation-swarm`

## Source Coverage

- Spec: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`, `497` lines.
- Umbrella plan before restructure: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`, `647` lines.
- Review synthesis: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/post-review-synthesis.md`, `59` lines.

## Failure Being Fixed

The prior artifact was reviewed and technically ready, but its structure still allowed execution confusion:

- Plan 1A substrate work and Plan 1B runtime/status work lived in one executable document.
- Plan 2 OAuth login was visible in the same file, making scope drift easier.
- A single umbrella matrix could be mistaken for child-plan proof.
- There was no separate child-plan completion boundary or child-plan review gate.

## Restructure Decision

- The original file is now an umbrella/control plan only.
- Plan 1A is extracted to `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`.
- Plan 1B is extracted to `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`.
- Plan 2 remains non-executable until a separate OAuth/device-code plan exists.

## Subagent Lanes

The following plan-creation lanes were dispatched for the restructure:

- `scope-and-proof-fit`: Bohr (`019eeee0-33c0-7ed0-9317-92e8f50d35e8`)
- `execution-order`: Cicero (`019eeee0-61b5-77e1-a331-b3933d38746b`)
- `validation-proof-and-security`: Turing (`019eeee0-8e62-7e72-91ca-8b5cbe71b752`)

Lane outputs are candidate evidence. Parent synthesis owns final plan text.

## Parent Synthesis

Accepted:

- Plan 1A and Plan 1B are the correct split: Plan 1A is substrate, Plan 1B is runtime/status/smoke.
- The umbrella must remain non-executable and may only roll child receipts up.
- Plan 1A is a stacked prerequisite/merge-gate slice, not final Plan 1 completion.
- T1 must be behavior-preserving and falsifiable; broad "where practical" language was removed.
- T6 must land before T7 because the current failed-zero path can destroy last-known quota state.
- Only two fan-out points are allowed: T2/T3 after T1, and T8/T9 versus T10/T11 after T7.
- Child plans need a proof contract that forbids placeholder proof, wrapper-only smoke, and prose-only live deferral.
- WebSocket remains in scope for Plan 1B proof.

Deferred:

- Plan 2 OAuth/device-code login, keyring default, logout/purge, profile apply, local-token lifecycle, turn-state envelope, and live proof remain outside Plan 1A/1B.

Residual uncertainty:

- T5 may or may not need a schema bump. The child plan keeps that as an implementation-time decision with migration proof if needed.

## Artifacts

- Umbrella plan: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
- Plan 1A: `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
- Plan 1B: `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- Lane artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/restructure-scope-and-proof-fit.md`
- Lane artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/restructure-execution-order.md`
- Lane artifact: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/restructure-validation-proof-and-security.md`
