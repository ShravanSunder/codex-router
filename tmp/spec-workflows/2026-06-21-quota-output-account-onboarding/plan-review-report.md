# Plan Review Report: Quota Output And Account Onboarding

Date: 2026-06-21
Target: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/implementation-plan.md
Coverage: 356-line draft plan read in chunks 1-140, 141-280, 281-356; revised plan verified at 429 lines.

## Verdict

Ready for `shravan-dev-workflow:implementation-execute-plan` after accepted plan-review findings were incorporated.

## Lanes Run

- spec-compliance: needs revision, findings accepted
- architecture-assumptions: needs revision, findings accepted
- testability-validation: needs revision, findings accepted
- security-reliability: needs revision, findings accepted
- execution-scope: needs revision, findings accepted

## Accepted Findings And Plan Edits

1. Storage backend decision was unresolved.

Resolution: plan now locks explicit plaintext file-backend fallback for this slice, requiring `--allow-plaintext-file-secrets`, docs/help disclosure, and Keychain as a follow-up storage-backend slice.

2. Router root, state DB, and secret-root contract was ambiguous.

Resolution: plan now hard-cuts new account/quota/serve flows to one `--router-root`; state DB and file secret root are derived from that root.

3. SQLite migration strategy was unsafe.

Resolution: plan now requires explicit v2 migration from current v1, not editing v1 in place.

4. Serve-owned quota worker boundary was under-specified.

Resolution: plan now says CLI `serve` owns the auth-backed worker; proxy remains auth/provider-agnostic. Worker has interval, timeout, paused scheduler seam, cooperative stop, join, and failure isolation.

5. State schema and renderer could drift.

Resolution: plan now defines `QuotaStatusRow` as the sole CLI renderer input for persisted status and compatibility live quota.

6. Credential import and quota normalization responsibilities were mixed.

Resolution: T3 now only parses auth/import credentials and token key conventions; expiry/health metadata lives in SQLite; quota normalization belongs to T5.

7. Atomicity and recovery for selector snapshots plus status rows were missing.

Resolution: T2/T5 require a transaction API and fault tests proving both selector and detailed status surfaces commit together or preserve previous good state.

8. Import partial failure was undefined.

Resolution: T4 now creates/reuses a disabled account row, writes/verifies secrets, then enables. Retry must repair partial imports without duplicate drift.

9. Label/PII handling was incomplete.

Resolution: plan now rejects email-like labels and requires canary tests proving raw email does not appear in output.

10. Validation gates did not match CI.

Resolution: T10 now includes targeted package tests and CI-aligned `cargo fmt`, `cargo clippy`, `cargo nextest`, `cargo deny`, `cargo audit`, and `actionlint`.

## Verification

- Revised plan line count: 429.
- Review packet line count: 74.
- Placeholder scan found no remaining matches for `maybe`, `if needed`, `possibly`, `must decide`, `unless necessary`, or `Open Questions`.
- Key revised anchors verified with `rg`:
  - `Plan Review Decisions Locked`
  - `--allow-plaintext-file-secrets`
  - `v2 migration`
  - `QuotaStatusRow`
  - `one SQLite transaction`
  - `--quota-refresh-interval-seconds`
  - `--quota-refresh-timeout-seconds`
  - `cargo fmt --all -- --check`
  - `cargo nextest run --workspace`

## Next Step

Recommended next workflow: `shravan-dev-workflow:implementation-execute-plan`

phase_result: complete
evidence: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/plan-review-report.md; tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/implementation-plan.md
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Accepted plan-review findings were incorporated and the reviewed plan is ready for code execution.
