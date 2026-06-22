# Restructure Lane: Validation Proof And Security

Lane: `validation-proof-and-security`
Agent: Turing (`019eeee0-8e62-7e72-91ca-8b5cbe71b752`)
Status: answered
Evidence label: `proof-contract-child-plan-v1`
Security context: applicable
Confidence: high

## Candidate Evidence

Child plans need four separate proof surfaces:

- Task-local checkbox gates.
- Executable requirement/proof matrix.
- Deferred and gated-live proof tables.
- Closeout receipt schema.

Hard rule: by execution start, no row may say `or equivalent`, `named test`, or point only at a wrapper script. Every row must name the exact command or exact scenario/test name.

## Accepted Parent Changes

- Added child proof contracts to Plan 1A and Plan 1B.
- Removed placeholder proof language in Plan 1A and Plan 1B matrix rows.
- Added exact installed smoke scenario names to Plan 1B.
- Added cross-plan rule that smoke proof must name exact `installed_codex_*` scenarios individually.
- Kept live proof approval-gated with exact `not-run: approval required` receipt.

## Anchors

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:98-153`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:203-258`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:364-430`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:432-455`
- `docs/testing/live-oauth-quota.md:120-212`
- `tests/smoke/installed_codex_mock.sh:1-26`

## Completion Receipt

Status: answered.
Parent wrote this lane artifact.
Remaining uncertainty: exact future test filters may change during implementation, but the plan now requires exact filters before execution closeout.
