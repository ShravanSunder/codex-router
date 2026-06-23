# Plan Ledger: Quota Runtime, Status, And OAuth Readiness

Date: 2026-06-22
Branch: `feature/initial-codex-router`
Workflow: `shravan-dev-workflow:plan-creation-swarm`

## Source Coverage

- Spec loaded after edit: `docs/specs/2026-06-20-codex-router-greenfield-spec.md` (`497` lines)
- Research evidence loaded after edit: `docs/specs/references/2026-06-20-research-evidence.md` (`105` lines)
- Quota burn-down research loaded: `tmp/research-workflows/2026-06-21-quota-burn-down/research-ledger.md` (`108` lines)

## Lane Outputs

| Lane | Agent | Status | Artifact |
| --- | --- | --- | --- |
| codebase-boundary | Mencius (`019eeea4-68cd-77e1-b6aa-e27dfa8e47bd`) | completed | `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/codebase-boundary.md` |
| validation-proof | Galileo (`019eeea4-99c8-72c1-8b87-03cb3a153663`) | completed with gaps | `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/validation-proof.md` |
| execution-order | Ramanujan (`019eeea4-c831-73c3-a8f8-6917edd1febd`) | candidate-ready | `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/execution-order.md` |
| security-reliability | Faraday (`019eeea4-fa82-7dc0-b56a-46b8d65ce21b`) | completed | `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/security-reliability.md` |
| scope-and-proof-fit | Leibniz (`019eeea5-356e-7822-991a-e167eae5c08c`) | answered | `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/lanes/scope-and-proof-fit.md` |

## Parent Decisions

1. Split the work into two proof-fit plans/PRs:
   - Plan 1: quota runtime correctness and status proof.
   - Plan 2: OAuth/device-code multi-account login.
2. Treat weekly weighting as real selector behavior, not just status visibility.
3. Therefore Plan 1 includes selector-state/schema work for per-window quota inputs.
4. Treat startup as nonblocking but selection as fail-closed for accounts with no valid persisted or freshly refreshed quota state.
5. Keep account switching next-normal-request only; no mid-stream retries or token/account rewrites.
6. Include imported-account auth-refresh substrate in Plan 1 because runtime correctness should not expire as soon as imported access tokens expire.
7. Keep `account login` and OS-keyring defaulting in Plan 2.

## Main Plan

- `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
