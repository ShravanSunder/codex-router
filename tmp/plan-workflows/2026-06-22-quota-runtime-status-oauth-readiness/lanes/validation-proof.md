# validation-proof

Status: completed with gaps identified
Agent: Galileo (`019eeea4-99c8-72c1-8b87-03cb3a153663`)
Confidence: medium-high

## Summary

The revised spec now has concrete proof obligations for startup, refresh cadence, transient/terminal failures, status UX, next-normal account switching, pace/runout math, and weekly-weighted selection. Existing tests cover parts of quota status, refresh persistence, selector reset hints, and serve startup, but they do not yet prove the stronger revised contract.

## Evidence Inspected

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:364-430`
- `docs/specs/references/2026-06-20-research-evidence.md:53-57`
- `tmp/research-workflows/2026-06-21-quota-burn-down/research-ledger.md:42-87`
- `crates/codex-router-cli/src/quota.rs:319-417`
- `crates/codex-router-cli/src/quota.rs:693-769`
- `crates/codex-router-cli/src/quota.rs:1017-1228`
- `crates/codex-router-cli/src/lib.rs:2353-3583`
- `crates/codex-router-cli/src/lib.rs:3183-3660`
- `crates/codex-router-proxy/src/http_sse.rs:775-826`
- `crates/codex-router-proxy/src/lib.rs:468-720`
- `tests/smoke/installed_codex_mock.sh:21-26`
- `crates/codex-router-test-support/src/installed_codex.rs:71-170`
- `crates/codex-router-test-support/src/installed_codex.rs:1358-1373`
- `docs/testing/live-oauth-quota.md:12-21`
- `docs/testing/live-oauth-quota.md:53-76`
- `docs/testing/live-oauth-quota.md:143-164`
- `docs/testing/live-oauth-quota.md:206-211`

## Required Proof Rows

| Requirement | Proof Layer | Must Prove |
| --- | --- | --- |
| Startup does not block on quota | Integration, smoke | Listener binds and request path can route from persisted SQLite before provider quota responds. |
| Immediate + scheduled refresh | Integration | One background refresh starts after bind without waiting the interval, and later scheduled refreshes continue. |
| Transient failure preservation | Integration, protocol | Last-known selector snapshot survives transient provider/network failure while status shows stale/failed diagnostics. |
| Terminal failure scoping | Integration, protocol | Only affected account/route band becomes ineligible; response aliases fan out together. |
| Next normal selectable path | Protocol | Request N+1 chooses another eligible account after account A becomes ineligible; no mid-stream switch. |
| Status table | Integration | `quota status` is SQLite-only and shows account, route, status, headroom, window, reset, pace, runout, notes. |
| Expanded status | Integration | `--all-limits` keeps the effective bottleneck row and per-window rows. |
| Pace/runout | Unit | Pace uses actual-used percent minus expected-used percent; runout uses projected burn rate with reset-aware edge cases. |
| Effective bottleneck inheritance | Unit, integration | Effective row inherits reset, pace, and runout from the limiting window. |
| Weekly-weighted selection | Unit, integration | Longer-window/weekly protection beats short-reset urgency when those signals conflict. |
| Smoke status capture | Smoke | Installed smoke captures a redacted quota table after background refresh. |
| Live quota/OAuth | Gated live proof | Not default CI; approval-gated and redacted per runbook. |

## Gaps

- Current persisted selector state stores only one `remaining_headroom` and one `reset_unix_seconds`; weekly-vs-short-window correctness needs richer inputs or a narrowed claim.
- Current refresh failure behavior tends toward failed-zero persistence, which conflicts with transient preservation.
- Current smoke harness does not appear to assert a quota-status table.
