# T2b Execution Receipt: Credential Commit Failpoints

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T2b credential refresh commit failpoints
Starting HEAD: `b7d2712`

## Scope Completed

- Added test-only `RefreshCommitFailpoint` hooks to
  `RouterCredentialResolver`.
- Proved the current resolver's cancellation/half-commit behavior:
  - after secret write before state commit, old generation remains
    authoritative and generation 2 secret is orphaned/unreachable
  - after state commit before return, retry observes committed generation 2
    without a second refresh
- Kept failpoints behind `#[cfg(test)]`; production resolver API is unchanged.

## Red Evidence

- `cargo test -p codex-router-auth credential_resolver_failpoint -- --nocapture`
  - exit code: 101 before implementation
  - expected failure: missing `RefreshCommitFailpoint` and
    `with_refresh_commit_failpoint`

## Green Evidence

- `cargo test -p codex-router-auth credential_resolver_failpoint -- --nocapture`
  - exit code: 0
  - result: 2 passed, 0 failed
- `cargo test -p codex-router-auth -- --nocapture`
  - exit code: 0
  - result: 15 passed, 0 failed
- `cargo check --workspace`
  - exit code: 0
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 0

## Matrix Status

- This supports T2 row `U-07` semantics, but does not mark `U-07` passed yet
  because `scripts/proof-matrix.sh U-07` remains a pending scaffold row.
- Remaining T2 work:
  - async SQLx credential commit/write boundary
  - proxy runtime cutover to async state/auth handles
  - `I-16` cancellation/failpoint integration row

## Not Claimed

- Credential refresh runtime is not yet async SQLx.
- Proxy runtime still uses sync credential resolver.
- No release `serve` async-completion claim.
