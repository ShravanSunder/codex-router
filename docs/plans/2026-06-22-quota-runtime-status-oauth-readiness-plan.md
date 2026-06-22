# Quota Runtime, Status, And OAuth Readiness Umbrella Plan

Date: 2026-06-22
Branch: `feature/initial-codex-router`
Status: umbrella/control plan; not executable; revised after plan-review `needs_revision`
Executable children:

- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`

Non-executable future work:

- Plan 2 OAuth/device-code/keyring account onboarding. This needs its own
  reviewed plan before onboarding-complete or release-ready multi-account auth
  can be claimed.

## Source Coverage

- [x] Source spec loaded: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`, `497` lines.
- [x] Spec chunk coverage: `1-170`, `171-340`, `341-497`.
- [x] Prior revised plan loaded: `647` lines.
- [x] Prior plan/spec review synthesis captured in `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/post-review-synthesis.md`.
- [x] This restructure uses `shravan-dev-workflow:plan-creation-swarm`.
- [x] Post-restructure review loaded:
      `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-post-restructure.md`,
      `227` lines.

## What Failed Before

The earlier plan looked comprehensive, but it was still too easy to misuse as a single implementation stream.

- [x] It bundled credential substrate, SQLite/state contracts, serve runtime behavior, selector policy, status UX, smoke proof, docs, and OAuth future work in one document.
- [x] It made Plan 2 visible but did not make the executable boundary impossible to confuse.
- [x] It had a strong matrix, but that matrix lived at umbrella level, so an executor could still blur Plan 1A substrate proof with Plan 1B runtime proof.
- [x] It did not force a merge/review gate between auth/state substrate and quota runtime behavior.

The success change is structural: the umbrella is no longer executable. Only the child plans are executable.

## Success Rule

No code task may start unless it belongs to exactly one executable child plan.

No code task may start until a dirty-tree isolation receipt exists. Preferred
execution uses a fresh worktree from the same branch tip. If execution stays in
this dirty worktree, the receipt must classify every dirty path, save hunk
fingerprints for same-path overlaps, and require each checkpoint commit to prove
that no out-of-scope path or baseline-only hunk was staged.

Plan 1A and Plan 1B are allowed to make quota/runtime/account-selection behavior
safe, but they are not sufficient to claim onboarding-complete multi-account
auth. A reviewed Plan 2 receipt is required before README/default UX can present
router onboarding as complete.

Plan 1 is not complete until both child receipts exist:

- [ ] Plan 1A merge-gate receipt exists.
- [ ] Plan 1A implementation-review blockers are resolved.
- [ ] Plan 1B final closeout receipt exists.
- [ ] Plan 1B implementation-review blockers are resolved.
- [ ] Plan 2 OAuth/device-code/keyring plan exists and is reviewed before any
      onboarding-complete claim.

Each child plan must have:

- [ ] Its own goal and non-goals.
- [ ] Its own write surfaces.
- [ ] Its own ordered checklist.
- [ ] Its own requirement/proof matrix.
- [ ] Its own validation gates.
- [ ] Its own checkpoint commits.
- [ ] Its own implementation-review gate.
- [ ] A clear statement of what is deferred.
- [ ] Its own proof matrix with `Proof owner`, exact preflight list command,
      exact execution command, expected observation, stale-proof guard, and
      red/green requirement.

## Account UX And Secret Backend Boundary

Current Plan 1-compatible command vocabulary:

- `account import-codex-auth --router-root <path> --label <label> --auth-json <path> --allow-plaintext-file-secrets`
- `account list --router-root <path>`
- `account enable --router-root <path> --account <id-or-label>`
- `account disable --router-root <path> --account <id-or-label>`
- `quota status --router-root <path> [--format table|plain] [--all-limits]`

Reserved for Plan 2:

- `account login`
- `account logout`
- `account remove`
- multi-account add/re-auth flow
- OS keyring/Keychain default backend
- migration or explicit fallback story for existing file-backed imports

Chosen v1 wording:

- There is no separate `account select` command in Plan 1. Runtime account
  choice is selector-owned.
- There is no separate account-specific `status` command in Plan 1. Account
  inventory is `account list`; quota state is `quota status`.
- `account import-codex-auth` is compatibility/dev/recovery input only.
  `auth.json` must not become runtime truth.
- The file backend is plaintext-at-rest under user-private filesystem
  permissions. It is not the normal multi-account OAuth storage story.

## Child Plan Boundaries

### Plan 1A: Credential And State Substrate

File:

- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`

Purpose:

- Build the substrate that makes runtime quota work safe.

Includes:

- [ ] Runtime boundary extraction where needed for auth/quota/state separation.
- [ ] Secret redaction and account-id contract cleanup.
- [ ] Fail-closed credential import/update.
- [ ] Unified credential resolver for quota refresh, HTTP/SSE, and WebSocket egress.
- [ ] Durable per-window selector source decision and repository/schema work.

Completion boundary:

- [ ] Credential updates cannot leave selectable partial accounts.
- [ ] Expired imported tokens are refreshed or fail closed before provider egress across quota refresh, HTTP/SSE, and WebSocket.
- [ ] Per-window selector data is available through an explicit durable source.
- [ ] Plan 1A targeted validation passes.
- [ ] Plan 1A implementation-review-swarm completes with no unresolved blockers.

### Plan 1B: Quota Runtime, Selection, Status, And Smoke

File:

- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`

Purpose:

- Implement user-visible quota runtime behavior on top of Plan 1A.

Includes:

- [ ] Transient-vs-terminal quota failure taxonomy.
- [ ] Nonblocking immediate-plus-scheduled quota refresh.
- [ ] Weekly/long-window-aware selection.
- [ ] Next-normal-path account switching.
- [ ] SQLite-only quota status table with pace/runout.
- [ ] Named installed-smoke cases.
- [ ] Docs/runbook alignment.

Completion boundary:

- [ ] Startup never blocks on quota refresh.
- [ ] Last-known quota remains usable after transient refresh failures.
- [ ] Immediate refresh works on the non-zero interval production path.
- [ ] Weekly quota pressure beats short-reset urgency.
- [ ] Status table and expanded view are SQLite-only and redacted.
- [ ] Named smoke cases prove startup/status/hostile/HTTP/WebSocket behavior.
- [ ] Plan 1B targeted and full relevant validation passes.
- [ ] Plan 1B implementation-review-swarm completes with no unresolved blockers.

### Plan 2: OAuth/Device-Code Multi-Account Login

Not executable here.

Before implementation, create a separate reviewed Plan 2 artifact covering:

- [ ] OS keyring/Keychain default backend.
- [ ] Dependency decision for keyring backend. Current crates.io evidence:
      `keyring 4.1.2` is available, MIT/Apache-2.0, Rust 1.88.0, and supports
      Apple Keychain through the default `v1` feature.
- [ ] Dependency decision for OAuth/device-code flow. Current crates.io
      evidence: `oauth2 5.0.0` is available, MIT/Apache-2.0, Rust 1.65, with
      `reqwest` + `rustls-tls` defaults.
- [ ] Device-code/browser polling flow.
- [ ] Multi-account add flow.
- [ ] Re-auth flow for expired or revoked accounts.
- [ ] `account logout`/secret purge.
- [ ] `account remove` metadata plus secret purge.
- [ ] Keyring/file-backend selection and migration story.
- [ ] Threat model.
- [ ] Mocked redacted UX and failure-mode proof.
- [ ] Approval-gated live proof.

## Execution DAG

```text
gate 0: dirty-tree isolation receipt + source artifact freeze
  |
  v
Plan 1A: credential/state substrate
  |
  +-- T1 no-behavior-change boundary extraction
  +-- T2 identity/redaction/token-egress guards
  +-- T3 fail-closed credential writes
  +-- merge gate A1: fail-closed credential receipt
  +-- T4 unified credential resolver
  +-- T5 durable per-window selector source
  +-- merge gate A2: substrate-complete receipt
  |
Plan 1A validation + implementation-review-swarm
  |
  v
Plan 1B: quota runtime/status/selection
  |
  +-- T6 failure taxonomy
  +-- merge gate B0: failure-taxonomy receipt
  +-- T7 immediate + scheduled refresh
  +-- T8 weekly-aware selection
  +-- T9 next-normal-path switching + same-turn/previous-response affinity
  +-- T10 quota status UX
  +-- T11 docs/runbook
  +-- merge gate B1: runtime/status/docs-ready receipt
  +-- T12 validation/smoke
  |
Plan 1B validation + implementation-review-swarm
  |
  v
Plan 2 creation/review before OAuth login implementation
```

Parallelism:

- [ ] Do not parallelize across Plan 1A and Plan 1B.
- [ ] Do not start Plan 1B before Plan 1A review and validation pass.
- [ ] Within Plan 1A, do not fan out by default. T2 and T3 are serial because
      current account import couples identity, secret writes, credential
      metadata, and enabled-state flip in one path.
- [ ] Within Plan 1A, do not parallelize T3 with T4 or T4 with T5.
- [ ] Within Plan 1B, failure taxonomy must land before immediate background refresh.
- [ ] Within Plan 1B, do not fan out by default. T8 precedes T9, and T10
      precedes T11.
- [ ] Within Plan 1B, do not parallelize T8 with T9, T10 with T11, or T12 with any writing lane.

## Cross-Plan Validation Rule

Every closeout must report:

- [ ] Command.
- [ ] Exit code.
- [ ] Pass/fail count where available.
- [ ] Requirement IDs covered.
- [ ] Proof owner.
- [ ] Exact preflight list command and result for each named test row.
- [ ] Stale-proof guard result.
- [ ] Red/green evidence for behavior changes.
- [ ] Explicit `not-run` reason for gated live proof.
- [ ] No executable proof row may use vague substitute wording, broad prefix
      filters, or wrapper-only smoke proof.
- [ ] Smoke proof must name each exact `installed_codex_*` scenario individually.
- [ ] Structural proof rows, such as resolver bypass checks, must include the
      exact search command and expected match count.

## Deferred Full-Spec Rows

These are not forgotten. They are intentionally deferred out of Plan 1A/1B:

- [ ] `account login` device-code/browser flow, Plan 2.
- [ ] OS keyring/Keychain normal login backend, Plan 2.
- [ ] `account logout`/secret purge implementation, Plan 2.
- [ ] `account remove` metadata plus secret purge, Plan 2.
- [ ] Profile write apply with approval.
- [ ] Gated live OAuth/device-code proof, Plan 2.
- [ ] Realtime/WebRTC support, explicitly out of v1.
- [ ] Real upstream account rotation/pooling live proof, approval-gated.

These are not deferred:

- [ ] Local bearer-token lifecycle proof receipt must appear in Plan 1B.
- [ ] Same-turn and previous-response affinity must appear in Plan 1B.
- [ ] Resolver bypass guard must appear in Plan 1A.
- [ ] Audit JSONL allowlist proof must appear in Plan 1A.
- [ ] Quota refresh one-writer behavior must appear in Plan 1B.

## Route To Execution

- [ ] Run `plan-review-swarm` on the child plans if they change materially.
- [ ] Execute only Plan 1A first.
- [ ] Do not claim Plan 1A complete until its review and validation gates pass.
- [ ] Execute Plan 1B only after a Plan 1A completion receipt exists with
      validation and implementation-review evidence. A single PR stack is
      allowed only if no Plan 1B checkpoint commit appears before that Plan 1A
      receipt commit.
- [ ] Do not implement Plan 2 until a new Plan 2 exists and is reviewed.
- [ ] Do not claim PR-ready/onboarding-complete status until the remote push/PR
      blocker is resolved. Current configured remote:
      `https://github.com/shravan-agent/codex-router.git` returned repository
      not found during push.
