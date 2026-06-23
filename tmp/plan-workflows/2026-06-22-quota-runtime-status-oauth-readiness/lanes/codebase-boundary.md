# codebase-boundary

Status: completed
Agent: Mencius (`019eeea4-68cd-77e1-b6aa-e27dfa8e47bd`)
Confidence: medium-high

## Summary

The revised quota runtime contract crosses four concrete ownership boundaries:

1. `crates/codex-router-cli/src/lib.rs` owns serve bootstrap and the background refresh worker lifecycle.
2. `crates/codex-router-cli/src/quota.rs` owns provider quota refresh, failure classification, normalization, and status output.
3. `crates/codex-router-proxy/src/http_sse.rs` and `crates/codex-router-proxy/src/server.rs` own request-time quota hydration and account selection.
4. `crates/codex-router-state/src/quota_snapshot.rs` and `crates/codex-router-state/src/sqlite.rs` own the persisted selector/status contract.

The current repo has the right high-level seams, but three important gaps are confirmed:

- startup refresh waits the first interval
- refresh failures overwrite selector snapshots with failed zero-headroom state instead of preserving last-known snapshots for transient failures
- weekly-protecting selection cannot be fully correct with the current persisted snapshot shape because selector state stores only one `remaining_headroom` and one `reset_unix_seconds` per route band

## Evidence Inspected

- `crates/codex-router-cli/src/lib.rs:73-116`
- `crates/codex-router-cli/src/lib.rs:328-393`
- `crates/codex-router-cli/src/quota.rs:45-229`
- `crates/codex-router-cli/src/quota.rs:965-1038`
- `crates/codex-router-cli/src/quota.rs:1122-1135`
- `crates/codex-router-proxy/src/server.rs:217-245`
- `crates/codex-router-proxy/src/server.rs:282-287`
- `crates/codex-router-proxy/src/server.rs:350-355`
- `crates/codex-router-proxy/src/server.rs:388-393`
- `crates/codex-router-proxy/src/http_sse.rs:552-826`
- `crates/codex-router-proxy/src/http_sse.rs:884-957`
- `crates/codex-router-state/src/repositories.rs:12-75`
- `crates/codex-router-state/src/quota_snapshot.rs:35-307`
- `crates/codex-router-state/src/sqlite.rs:295-358`
- `crates/codex-router-state/src/sqlite.rs:495-515`
- `crates/codex-router-selection/src/eligibility.rs:37-63`
- `crates/codex-router-selection/src/weighted_deficit.rs:66-98`
- `crates/codex-router-cli/src/account.rs:243-320`
- `crates/codex-router-state/src/account.rs:73-121`
- `docs/testing/live-oauth-quota.md:70-76`
- `docs/testing/live-oauth-quota.md:162-164`

## Planning Consequences

- Change worker timing in `crates/codex-router-cli/src/lib.rs`.
- Change failure classification and status persistence in `crates/codex-router-cli/src/quota.rs`.
- Change selector weighting and next-request reselection in `crates/codex-router-proxy/src/http_sse.rs`.
- Expand `crates/codex-router-state/src/quota_snapshot.rs` and `crates/codex-router-state/src/sqlite.rs` if weekly protection must be real selector behavior.

## Open Questions Resolved By Parent Plan

- Treat all-unknown startup as nonblocking but fail-closed for selection until a valid snapshot or successful refresh exists.
- Treat weekly weighting as real selector behavior; include a schema-backed selector-state step.
- Keep upstream-response retry/rotation out of this slice; account switching is next-normal-request only.
- Unify status freshness with runtime freshness where possible, instead of leaving an unrelated hard-coded display window.
