# scope-and-proof-fit

Status: answered
Agent: Leibniz (`019eeea5-356e-7822-991a-e167eae5c08c`)
Confidence: medium-high

## Verdict

Split this into two plans and two PRs.

- Plan/PR 1: quota runtime correctness and status proof.
- Plan/PR 2: OAuth/device-code multiple-account login.

A single bundled PR is not proof-fit unless the user explicitly accepts the larger live-auth gate and longer merge path.

## Evidence Inspected

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:98-153`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:203-240`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:364-430`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:492-497`
- `docs/testing/live-oauth-quota.md:23-165`
- `docs/testing/live-oauth-quota.md:206-212`
- `crates/codex-router-cli/src/lib.rs:73-115`
- `crates/codex-router-cli/src/lib.rs:318-392`
- `crates/codex-router-cli/src/lib.rs:1074-1094`
- `crates/codex-router-cli/src/lib.rs:3289-3442`
- `crates/codex-router-cli/src/account.rs:32-60`
- `crates/codex-router-cli/src/account.rs:135-183`
- `crates/codex-router-cli/src/account.rs:226-359`
- `crates/codex-router-cli/src/quota.rs:50-420`
- `crates/codex-router-cli/src/quota.rs:472-490`
- `crates/codex-router-cli/src/quota.rs:693-769`
- `crates/codex-router-cli/src/quota.rs:1017-1228`
- `crates/codex-router-auth/src/oauth.rs:3-84`
- `crates/codex-router-auth/src/refresh_worker.rs:6-83`
- `crates/codex-router-secret-store/src/file_backend.rs:15-79`
- `crates/codex-router-selection/src/eligibility.rs:6-64`
- `crates/codex-router-selection/src/weighted_deficit.rs:60-98`
- `crates/codex-router-state/src/quota_snapshot.rs:73-353`
- `tmp/research-workflows/2026-06-21-quota-burn-down/research-ledger.md:80-101`
- `docs/plans/reviews/2026-06-20-codex-router-plan-review.md:90-95`
- `tmp/plan-workflows/2026-06-21-codex-router-feature-initial-codex-router-quota-output-account-onboarding/implementation-review-report.md:44-56`

## Why Split

1. Full login is approval-gated/high-risk; quota runtime/status fixes are locally provable.
2. The spec requires OS keyring as the normal login backend, while current code still uses `FileSecretStore` in the active paths.
3. Quota correctness has independent missing prerequisites: immediate refresh, nonblocking request path, transient-vs-terminal failures, next-normal switching, and weekly-weighted state.
4. Current auth substrate is not login-ready and not runtime-complete; quota refresh still reads access tokens directly and ignores refresh-token/expiry metadata.
5. Weekly-weighted routing is a quota-state problem, not a login problem.

## Plan Cut

Plan 1 includes:

- serve-owned quota worker semantics
- persisted quota correctness
- status UX and pace/runout display
- next-normal-request switching
- imported-account auth refresh substrate, if access-token expiry must be handled correctly before login
- selector-state/schema work if weekly weighting is real selector behavior

Plan 1 excludes:

- `account login`
- browser/device-code flow
- default keyring rollout
- live multi-account OAuth/quota pooling proof
- logout/delete-secret behavior
- runtime `auth.json` fallback

Plan 2 includes:

- normal login secret backend selection, with OS keyring default
- `account login` device-code/browser flow
- account creation/add flow
- login wired onto Plan 1 refresh/update primitives
- approval-gated live proof/runbook updates

## Replan Triggers

- If login must land in the same PR, promote the entire effort to a larger PR with live-auth proof explicitly marked approval-gated.
- If weekly weighting must be real routing behavior, Plan 1 needs selector-state/schema work.
- If runtime correctness requires refresh-token exchange for imported accounts, keep that in Plan 1 and do not hide it in login.
