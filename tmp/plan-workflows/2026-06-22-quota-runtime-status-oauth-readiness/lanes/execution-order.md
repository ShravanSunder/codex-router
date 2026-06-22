# execution-order

Status: candidate-ready
Agent: Ramanujan (`019eeea4-c831-73c3-a8f8-6917edd1febd`)
Confidence: medium-high

## Summary

The safe execution order is not “add login, then hope quota works.” The codebase has large shared chokepoints and an already dirty tree. The runtime/auth/secret/state contracts need to be stabilized before login is exposed as a normal command.

## Evidence Inspected

- `Cargo.toml:1-13`
- `crates/codex-router-cli/src/lib.rs:71-115`
- `crates/codex-router-cli/src/lib.rs:328-380`
- `crates/codex-router-cli/src/lib.rs:485-617`
- `crates/codex-router-cli/src/account.rs:32-63`
- `crates/codex-router-cli/src/account.rs:243-320`
- `crates/codex-router-cli/src/quota.rs:304-419`
- `crates/codex-router-cli/src/quota.rs:953-1098`
- `crates/codex-router-state/src/sqlite.rs:495-533`
- `crates/codex-router-state/src/account.rs:73-122`
- `crates/codex-router-state/src/sqlite.rs:242-293`
- `crates/codex-router-state/src/sqlite.rs:664-669`
- `crates/codex-router-auth/src/router_credentials.rs:6-112`
- `crates/codex-router-auth/src/live_quota.rs:34-53`
- `crates/codex-router-auth/src/oauth.rs:1-84`
- `crates/codex-router-auth/src/refresh_worker.rs:1-84`
- `crates/codex-router-auth/src/quota_client.rs:6-33`
- `crates/codex-router-proxy/src/http_sse.rs:552-756`
- `tmp/plan-workflows/2026-06-21-codex-router-feature-initial-codex-router-quota-output-account-onboarding/implementation-execute-plan-brief.md:20-23`
- `tmp/plan-workflows/2026-06-21-codex-router-feature-initial-codex-router-quota-output-account-onboarding/implementation-review-report.md:44-57`

## Recommended Sequence

1. Freeze repo state and identify pre-existing dirty paths.
2. Extract runtime boundaries from oversized CLI files before stacking more behavior into them.
3. Fix identity and redaction contracts: account IDs for runtime, labels for display, no secret-bearing `Debug`.
4. Add coherent credential commit/update primitives so import and future refresh cannot leave selectable partial accounts.
5. Implement refresh-token-backed auth refresh for imported accounts if runtime correctness includes expiring tokens.
6. Move quota runtime to immediate post-bind refresh plus scheduled refresh.
7. Expand selector-state inputs and update selector/runtime policy for weekly weighting and next-normal account switching.
8. Wire CLI/status/help/docs after service contracts settle.
9. Run final proof and review-closeout.

## Safe Parallelism

Safe after the quota runtime contract is stable:

- proxy/selection policy
- CLI/table/help/docs
- proof-only regression additions

Unsafe to parallelize:

- concurrent edits to `crates/codex-router-cli/src/lib.rs`
- concurrent edits to `crates/codex-router-cli/src/quota.rs`
- concurrent edits to `crates/codex-router-state/src/sqlite.rs`
- auth refresh before credential commit primitives exist

## Checkpoint Commits

1. Boundary extraction only.
2. Identity + redaction contract cleanup.
3. Atomic credential import/update primitives.
4. Refresh-token-backed auth refresh runtime.
5. Immediate + scheduled quota runtime and max-age fix.
6. Selector policy update.
7. CLI/account/quota UX and docs.
8. Proof and review closeout.
