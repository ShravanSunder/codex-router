# Quota Runtime, Status, And OAuth Readiness Umbrella Plan

Date: 2026-06-22
Branch: `feature/initial-codex-router`
Status: umbrella/control plan; not executable; revised after plan-review `needs_revision`
Revision status: folded accepted findings from `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-460b51e.md`
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
execution uses a fresh worktree from the reviewed plan commit or the latest
approved receipt commit. Before any code edit in a fresh worktree, copy or
promote the exact lifecycle packets under
`tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/` and
`tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/`, then record a
carry-forward receipt with source path, target path, source commit/head,
checksum or byte count, and `git status --short` before and after.

If execution stays in this dirty worktree, the receipt must classify every dirty
path, save hunk fingerprints for same-path overlaps, and require each checkpoint
commit to prove that no out-of-scope path or baseline-only hunk was staged. The
same dirty-tree proof is required at A1, A2, B0, B1, and final closeout, not
only at Gate 0.

No code task may start until required source artifacts are frozen. If source
inputs such as `docs/specs/2026-06-20-codex-router-greenfield-spec.md` or
`docs/specs/references/2026-06-20-research-evidence.md` are dirty relative to
the execution base, execution must either commit/promote them before cutting the
fresh worktree or record a source carry-forward receipt for each input with
path, source commit/head, checksum or byte count, working-tree line count,
execution-base line count, and whether the input is normative.

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
gate 0a: source artifact freeze + dirty-tree inventory
  |
gate 0b: fresh-worktree/tmp lifecycle carry-forward receipt
  |
  v
Plan 1A: credential/state substrate
  |
  T1 no-behavior-change boundary extraction
  |
  T2 identity/redaction/token-egress guards
  |
  T3 fail-closed credential writes
  |
  merge gate A1: fail-closed credential receipt
  |
  T4 unified credential resolver
  |
  T5 durable per-window selector source
  |
  merge gate A2: substrate-complete receipt
  |
Plan 1A validation + implementation-review-swarm
  |
  v
Plan 1B: quota runtime/status/selection
  |
  T6 failure taxonomy
  |
  merge gate B0: failure-taxonomy receipt
  |
  T7 immediate + scheduled refresh
  |
  T8 weekly-aware selection
  |
  T9 next-normal-path switching + same-turn/previous-response affinity
  |
  T10 quota status UX
  |
  T11 docs/runbook
  |
  merge gate B1: runtime/status/docs-ready receipt
  |
  T12 validation/smoke
  |
Plan 1B validation + implementation-review-swarm
  |
  v
Plan 2 creation/review before OAuth login implementation
```

Parallelism:

- [ ] Do not parallelize across Plan 1A and Plan 1B.
- [ ] Do not start Plan 1B before Plan 1A review and validation pass.
- [ ] Do not fan out tasks by default inside either child plan. Accepted review
      findings tie auth, credential writes, SQLite visibility, alias-family
      publication, one-writer leases, local auth, and smoke proof tightly enough
      that serial execution is the safe default.
- [ ] Parallel implementation requires an explicit replan proving disjoint write
      surfaces, independent proof rows, and no shared checkpoint receipt.
- [ ] Plan 1A order is T1, T2, T3, A1, T4, T5, A2, then validation and
      implementation review.
- [ ] Plan 1B order is T6, B0, T7, T8, T9, T10, T11, B1, T12, then validation
      and implementation review.

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
- [ ] Each checkpoint receipt must include the commit hash, `git show
      --name-only <checkpoint>`, owned-path-only proof, same-path baseline-hunk
      proof, and explicit evidence that no baseline-only hunk was staged.

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
