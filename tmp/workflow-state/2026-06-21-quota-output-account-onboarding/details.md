# Goal Workflow Details: Quota Output And Account Onboarding

goal_id: 2026-06-21-quota-output-account-onboarding
required workflow skill: shravan-dev-workflow:orchestrator-goal
current workflow: shravan-dev-workflow:spec-creation-swarm
next workflow: shravan-dev-workflow:spec-review-swarm
terminal condition: PR created or updated, implementation proof captured, implementation review findings addressed or explicitly rejected, PR checks and readiness freshly reported, not merged.

## Objective

Make codex-router usable for real local Codex routing by adding router-owned account onboarding and fixing quota status output.

This includes:

- router-owned OAuth account import/login/list/enable/disable/logout UX
- quota refresh/status UX over router-owned accounts
- compact human-readable quota output, with detailed all-window output available
- preservation of Codex ownership of sessions, history, transport behavior, and home configuration
- redacted, approval-gated live proof only

## Scope

In scope:

- `crates/codex-router-cli`
- `crates/codex-router-auth`
- `crates/codex-router-secret-store`
- `crates/codex-router-state`
- `crates/codex-router-quota`
- docs/runbook updates for the implemented commands and live proof boundary
- tests and implementation review for the changed surfaces

Out of scope unless explicitly reauthorized:

- merging the PR
- broad changes to Codex or Prodex checkouts
- automatic writes to `~/.codex`
- printing OAuth tokens, raw auth JSON, raw account emails, prompts, request bodies, or response bodies
- live OAuth/quota/model-traffic execution without explicit approval in the transcript
- 1Password adapter implementation unless selected later

## Current Evidence

- `README.md` states codex-router owns local router auth, upstream OAuth accounts, quota snapshots, account selection, and byte-preserving forwarding, while Codex owns CLI/session behavior.
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:184` says router owns OAuth credentials.
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:188` says Codex/Prodex `auth.json` is only compatibility input for explicit import, migration, or gated live proof.
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:198` requires OS keyring/macOS Keychain as the default real backend.
- `docs/testing/live-oauth-quota.md:23` shows the current executable surface is only `live quota` over `auth.json` or a profiles root.
- `crates/codex-router-cli/src/lib.rs:313` has no top-level account or quota command today.
- `crates/codex-router-cli/src/live.rs:303` renders the current live quota table.
- `crates/codex-router-cli/src/live.rs:521` still renders `ahead`/`behind` pace wording.
- `crates/codex-router-state/src/account.rs:35` has non-secret account metadata with enabled/disabled status.
- `crates/codex-router-state/src/quota_snapshot.rs:35` has persisted quota snapshots with route band, headroom, reset hint, and stale penalty.
- `crates/codex-router-secret-store/src/file_backend.rs:15` exposes the current secret-store trait and hardened file backend.
- `crates/codex-router-secret-store/src/account_tokens.rs:8` currently keys only upstream access tokens.

## Workflow State Contract

Required reading:

- `tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/quota-output-account-onboarding-spec.md`
- `tmp/workflow-state/2026-06-21-quota-output-account-onboarding/details.md`
- `tmp/workflow-state/2026-06-21-quota-output-account-onboarding/events.jsonl`

Current workflow remains `shravan-dev-workflow:spec-creation-swarm` until the parent verifies the spec artifact and records transition to spec review. Phase skills may recommend transitions, but the latest orchestrator-written event in `events.jsonl` owns official workflow transition state.

## Proof Expectations

The later implementation plan must define exact commands. At minimum, proof must cover:

- unit tests for auth parsing/import, secret-key conventions, account enable/disable/logout, quota math, table rendering, and redaction
- integration tests for SQLite account metadata plus secret-store credential writes
- CLI tests for account import/list/enable/disable/logout and quota status/refresh modes
- smoke proof for configured router selection using router-owned account state
- lint/type/format/security checks already authoritative for this repo
- live proof only when explicitly approved; otherwise report `not-run: approval required`

## Stop Rules

Stop before implementation if spec review accepts a finding that changes the account/auth boundary, secret backend default, persisted quota schema, or live proof safety boundary.

Stop during implementation if validation failures are outside the agreed code path and would require infrastructure/tooling edits.
