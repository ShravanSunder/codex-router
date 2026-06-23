# Spec Review Report: Quota Output And Account Onboarding

Date: 2026-06-21
Target: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/quota-output-account-onboarding-spec.md
Coverage: 419-line revised spec; original 321-line draft read in full before review, then accepted findings were patched and verified by targeted `rg`.

## Verdict

Ready for `shravan-dev-workflow:plan-creation-swarm` after the accepted review findings were incorporated.

## Lanes Run

- contract-and-scope: decision-needed, findings accepted
- architecture-boundaries: needs revision, findings accepted
- security-threat-model: needs revision, findings accepted
- validation-and-testability: needs revision, findings accepted

## What Held

- Router-owned account/quota state is the right product boundary.
- `auth.json` must remain explicit import/proof input, not runtime credential source of truth.
- `quota status` must read local persisted state and perform no provider I/O.
- Request-time selection must not block on broad quota refresh.
- Compact quota output and detailed all-window output are separate views over normalized quota data.
- Login and logout should not be faked. Import is the usable first onboarding path; logout waits for delete-capable secret storage.

## Accepted Findings And Edits

1. Logout scope was internally unresolved.

Resolution: reserved `account logout` until `SecretStore::delete_secret` exists; removed logout from in-scope command list and boundary map.

2. Background refresh lacked enough runtime/proof contract.

Resolution: added R4A runtime details, a quota refresh runtime owner section, and proof matrix rows requiring serve-level scheduling, persisted success/failure, and zero request-path provider calls.

3. Compact table route identity was ambiguous.

Resolution: compact rows are now keyed by `(account, route band)` and include a `Route` column.

4. Persisted quota state was not rich enough for local-only status.

Resolution: added a persisted quota status schema requiring normalized compact and detailed rows, bottleneck markers, pace/runout inputs, freshness, and redacted refresh failures.

5. Proof matrix inputs were too thin.

Resolution: added a plan-creation proof matrix input section requiring one row per R1-R10 plus R4A and mandatory rows for import, lifecycle, status local-only, serve background refresh, table output, storage disclosure, and live gate.

6. SQLite path writes and identity handling were under-specified.

Resolution: added a security acceptance contract covering state DB path containment or approval, symlink/`.codex` rejection, opaque non-PII account ids, redacted labels, quota response normalization, and network approval boundaries.

## Contested Or Planning Decisions

- Keychain-first versus explicit file-backend development fallback remains a plan decision. The spec requires disclosure and proof whichever path is chosen.
- Access-token-only import may be acceptable only if status makes expiry/refresh limitations visible.
- `quota status --format json` is out of scope unless plan creation adds a positive-schema proof row before implementation.

## Open Questions

1. Does the supported Codex/Prodex `auth.json` shape include refresh token and expiry fields?
2. Should refresh failure state be stored in quota status rows, a separate refresh-status table, or both?

These are planning inputs, not blockers to plan creation.

## Evidence

- Revised spec line count: 419 lines.
- Review packet line count: 56 lines.
- Accepted-finding anchors verified with `rg`:
  - `Persisted quota status schema`
  - `Quota refresh runtime owner`
  - `Security Acceptance Contract`
  - `Plan-Creation Proof Matrix Inputs`
  - `Account | Route | Status | Headroom | Window | Reset | Pace | Runout | Notes`
  - `provider I/O`
  - `opaque`
  - `account logout`

Subagent proof reported:

- architecture lane ran `cargo test -p codex-router-state -p codex-router-quota -p codex-router-secret-store --lib`: 19 passed, 0 failed.
- security lane ran `cargo test -p codex-router-secret-store -p codex-router-state -p codex-router-auth -p codex-router-cli -p codex-router-proxy`: 93 passed, 0 failed.

## Next Step

Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`

phase_result: complete
evidence: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/spec-review-report.md; tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/quota-output-account-onboarding-spec.md
recommended_next_workflow: shravan-dev-workflow:plan-creation-swarm
recommended_transition_reason: Accepted spec-review findings were incorporated and the revised spec is ready to turn into an implementation plan.
