# security-reliability

Status: completed
Agent: Faraday (`019eeea4-fa82-7dc0-b56a-46b8d65ce21b`)
Confidence: medium-high

## Summary

The final plan must explicitly protect secrets, token egress, SQLite coherence, background-worker lifecycle, and account-switching semantics. The main current risk is persisting transient failures as terminal failed-zero quota state.

## Evidence Inspected

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:98-153`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:203-259`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:364-454`
- `docs/testing/live-oauth-quota.md:23-76`
- `docs/testing/live-oauth-quota.md:90-160`
- `docs/testing/live-oauth-quota.md:206-212`
- `crates/codex-router-auth/src/live_quota.rs:14-21`
- `crates/codex-router-auth/src/live_quota.rs:35-53`
- `crates/codex-router-auth/src/live_quota.rs:184-214`
- `crates/codex-router-auth/src/live_quota.rs:373-447`
- `crates/codex-router-auth/src/router_credentials.rs:6-12`
- `crates/codex-router-auth/src/router_credentials.rs:54-111`
- `crates/codex-router-secret-store/src/file_backend.rs:31-198`
- `crates/codex-router-secret-store/src/refresh_lease.rs:41-128`
- `crates/codex-router-cli/src/account.rs:243-398`
- `crates/codex-router-cli/src/quota.rs:289-897`
- `crates/codex-router-cli/src/quota.rs:965-1135`
- `crates/codex-router-cli/src/lib.rs:71-392`
- `crates/codex-router-cli/src/lib.rs:2567-2610`
- `crates/codex-router-cli/src/lib.rs:2614-2767`
- `crates/codex-router-cli/src/lib.rs:3053-3180`
- `crates/codex-router-cli/src/lib.rs:3288-3410`
- `crates/codex-router-cli/src/lib.rs:3617-3660`
- `crates/codex-router-cli/src/lib.rs:3800-3978`
- `crates/codex-router-proxy/src/http_sse.rs:552-959`

## Constraints

- Runtime credentials must come from router-owned secret storage, not direct `auth.json` reads.
- `auth.json` stays compatibility-only for explicit import or gated live proof.
- Quota endpoint allowlisting must happen before bearer-token egress.
- Status/error/log/audit output must not leak access tokens, refresh tokens, local bearer tokens, auth headers, raw emails, raw provider JSON, request bodies, or response bodies.
- Transient refresh failures preserve last-known selector snapshots and update only redacted diagnostics/status.
- Terminal failures mark only affected account/route bands ineligible.
- Response aliases (`responses`, `models`, `memories_trace_summarize`, `responses_compact`) must update consistently.
- Partial import/repair must fail closed; no selectable account without coherent secret + metadata state.
- Serve binds and reports readiness before broad quota I/O.
- Worker shutdown is bounded and diagnostics are redacted.
- `quota status` remains SQLite-only and read-only.
- Account switching is next-normal-request only, not mid-stream.

## Must-Have Rows

| ID | Requirement | Proof |
| --- | --- | --- |
| SR1 | Define transient vs terminal failure taxonomy. | Transient preserves snapshot; terminal zeroes only affected bands/aliases. |
| SR2 | Partial import/repair containment. | Injected failure leaves account disabled/ineligible and healthy accounts unaffected. |
| SR3 | Token egress guard. | Disallowed base URL is rejected before token read/send in refresh and serve. |
| SR4 | Immediate post-bind background refresh. | Listener readiness precedes quota response; immediate cycle happens without waiting interval. |
| SR5 | Bounded worker lifecycle. | Stop/join completes within timeout with redacted stderr. |
| SR6 | SQLite snapshot/status coherence. | Failure injection cannot leave mismatched selector/status rows; aliases update together. |
| SR7 | Status remains non-live. | Zero provider I/O during status; default and expanded views render from SQLite. |
| SR8 | Account-switching correctness. | Next request chooses another eligible account after terminal ineligibility. |
| SR9 | Secret redaction regression. | Canary search across stdout/stderr/errors/status text. |

## Open Questions Resolved By Parent Plan

- Unknown/no-snapshot state is nonblocking but not selectable unless the selector has no better option and an explicit fallback rule is added later; this plan chooses fail-closed for selection.
- Request-path upstream response classification is not in this slice; terminal ineligibility comes from refresh/manual status writes.
- Access-token unusable plus refresh-token present is in scope for imported-account auth refresh substrate before login UX.
