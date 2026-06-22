# Quota Runtime, Status, And OAuth Readiness Umbrella Plan

Date: 2026-06-22
Branch: `feature/initial-codex-router`
Status: umbrella/control plan; not executable
Executable children:

- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`

Non-executable future work:

- Plan 2 OAuth/device-code login. This needs its own reviewed plan before code work.

## Source Coverage

- [x] Source spec loaded: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`, `497` lines.
- [x] Spec chunk coverage: `1-170`, `171-340`, `341-497`.
- [x] Prior revised plan loaded: `647` lines.
- [x] Prior plan/spec review synthesis captured in `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/post-review-synthesis.md`.
- [x] This restructure uses `shravan-dev-workflow:plan-creation-swarm`.

## What Failed Before

The earlier plan looked comprehensive, but it was still too easy to misuse as a single implementation stream.

- [x] It bundled credential substrate, SQLite/state contracts, serve runtime behavior, selector policy, status UX, smoke proof, docs, and OAuth future work in one document.
- [x] It made Plan 2 visible but did not make the executable boundary impossible to confuse.
- [x] It had a strong matrix, but that matrix lived at umbrella level, so an executor could still blur Plan 1A substrate proof with Plan 1B runtime proof.
- [x] It did not force a merge/review gate between auth/state substrate and quota runtime behavior.

The success change is structural: the umbrella is no longer executable. Only the child plans are executable.

## Success Rule

No code task may start unless it belongs to exactly one executable child plan.

Plan 1 is not complete until both child receipts exist:

- [ ] Plan 1A merge-gate receipt exists.
- [ ] Plan 1A implementation-review blockers are resolved.
- [ ] Plan 1B final closeout receipt exists.
- [ ] Plan 1B implementation-review blockers are resolved.

Each child plan must have:

- [ ] Its own goal and non-goals.
- [ ] Its own write surfaces.
- [ ] Its own ordered checklist.
- [ ] Its own requirement/proof matrix.
- [ ] Its own validation gates.
- [ ] Its own checkpoint commits.
- [ ] Its own implementation-review gate.
- [ ] A clear statement of what is deferred.

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
- [ ] Device-code/browser polling flow.
- [ ] Multi-account add flow.
- [ ] `account logout`/secret purge.
- [ ] Keyring/file-backend selection and migration story.
- [ ] Threat model.
- [ ] Mocked proof.
- [ ] Approval-gated live proof.

## Execution DAG

```text
gate 0: freeze repo state and source artifacts
  |
  v
Plan 1A: credential/state substrate
  |
  +-- T1 no-behavior-change boundary extraction
  +-- safe fan-out A: T2 identity/redaction/token-egress guards
  +-- safe fan-out A: T3 fail-closed credential writes
  +-- merge gate A1
  +-- T4 unified credential resolver
  +-- T5 durable per-window selector source
  +-- merge gate A2
  |
Plan 1A validation + implementation-review-swarm
  |
  v
Plan 1B: quota runtime/status/selection
  |
  +-- T6 failure taxonomy
  +-- merge gate B0
  +-- T7 immediate + scheduled refresh
  +-- safe fan-out B: T8 weekly-aware selection -> T9 next-normal-path switching
  +-- safe fan-out B: T10 quota status UX -> T11 docs/runbook
  +-- merge gate B1
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
- [ ] Within Plan 1A, only T2 and T3 may fan out after T1, and only if their write scopes stay disjoint.
- [ ] Within Plan 1A, do not parallelize T3 with T4 or T4 with T5.
- [ ] Within Plan 1B, failure taxonomy must land before immediate background refresh.
- [ ] Within Plan 1B, only selector work T8/T9 and status/docs work T10/T11 may fan out after T7.
- [ ] Within Plan 1B, do not parallelize T8 with T9, T10 with T11, or T12 with any writing lane.

## Cross-Plan Validation Rule

Every closeout must report:

- [ ] Command.
- [ ] Exit code.
- [ ] Pass/fail count where available.
- [ ] Requirement IDs covered.
- [ ] Stale-proof guard result.
- [ ] Red/green evidence for behavior changes.
- [ ] Explicit `not-run` reason for gated live proof.
- [ ] No executable proof row may use placeholder text such as `or equivalent`, `named test`, or wrapper-only smoke proof.
- [ ] Smoke proof must name each exact `installed_codex_*` scenario individually.

## Deferred Full-Spec Rows

These are not forgotten. They are intentionally deferred out of Plan 1A/1B:

- [ ] `account login` device-code/browser flow.
- [ ] OS keyring/Keychain normal login backend.
- [ ] `account logout`/secret purge implementation.
- [ ] Profile write apply with approval.
- [ ] Local bearer token lifecycle rotation, unless already proven by existing scope.
- [ ] Full turn-state envelope implementation, unless already proven by existing scope.
- [ ] Realtime/WebRTC support, explicitly out of v1.
- [ ] Real upstream account rotation/pooling live proof, approval-gated.

## Route To Execution

- [ ] Run `plan-review-swarm` on the child plans if they change materially.
- [ ] Execute only Plan 1A first.
- [ ] Do not claim Plan 1A complete until its review and validation gates pass.
- [ ] Execute Plan 1B only after Plan 1A is complete or intentionally merged into a single PR with an explicit review-approved exception.
- [ ] Do not implement Plan 2 until a new Plan 2 exists and is reviewed.
