# Plan Revision Lane: OAuth Account UX Scope

Status: answered
Security context: applicable

Accepted into plan:

- Plan 1A/1B do not implement interactive OAuth/device-code login.
- Plan 1A defines backend-neutral credential substrate that Plan 2 can extend.
- Plan 1B documents current command truth and prevents file-backed import from being presented as normal onboarding.
- Plan 2 is required before onboarding-complete or release-ready multi-account auth claims.
- Current Plan 1 command vocabulary is `account import-codex-auth`, `account list`, `account enable`, `account disable`, and `quota status`.
- Plan 2 command vocabulary is `account login`, `account logout`, `account remove`, multi-account add/re-auth, OS keyring/Keychain backend, migration/fallback story, mocked UX proof, and approval-gated live proof.

Key evidence:

- Spec secret backend and `auth.json` boundary: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- Current CLI account verbs: `crates/codex-router-cli/src/account.rs`
- Current help text: `crates/codex-router-cli/src/lib.rs`
- Current secret-store implementation: `crates/codex-router-secret-store/src/file_backend.rs`

Confidence: high
