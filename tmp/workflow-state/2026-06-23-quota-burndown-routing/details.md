# Goal Details: 2026-06-23-quota-burndown-routing

## Objective

Deliver reset-aware quota burn-down routing and quota status for `codex-router`
as a fully proven product path, not only a spec, plan, or partial
implementation.

Completion requires:

- accepted revised spec
- accepted implementation plan that traces every task back to the goal and spec
- adversarial plan review passed or findings folded back into the plan
- implementation completed for the accepted plan
- implementation review findings addressed or explicitly rejected with evidence
- full proof loop captured, including end-to-end Codex-through-router behavior
- PR created or updated and proven ready, but not merged unless separately
  authorized

## Scope

In scope:

- reset-aware quota burn-down algorithm
- account classification and routing decisions across 5h and weekly windows
- shared quota assessment semantics for runtime routing and status display
- human quota/status UX with concise account-centric rows, Unicode bars, and
  explicit selected-next explanation
- non-blocking startup and request behavior using persisted quota state
- background refresh behavior and stale/unknown/ineligible handling
- proof gates across unit, integration, smoke, and end-to-end runtime paths

Out of scope unless explicitly brought back into this goal:

- merging the PR
- unrelated OAuth/keychain work not required by quota routing/status proof
- destructive cleanup of unrelated dirty worktree files
- weakening or deleting proof gates to make the lifecycle pass

## Required Reading

- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/review-ledger.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/algorithm-prior-art-crux.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/contract-architecture-difference.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/planning-adversarial-crux.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/requirements-validation.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/ux-progressive-guardrails.md`

## Accepted Spec Review Findings

The current spec is not accepted. The review found these required fixes:

1. Make the burn-down score to selector weight contract normative.
2. Define shared ownership and dependency edges for assessment DTOs, selection,
   proxy adapters, state DTOs, and CLI display.
3. Freeze threshold and reset-salvage policy as fixed v1 constants or named
   config defaults with rationale and proof bounds.
4. Define mixed window collapse for ineligible, stale, unknown, missing reset,
   no effective row, and empty window set.
5. Make human quota/status output strict: at most two rows per account, Unicode
   bars, no `pp`, no `bottleneck`, no account id in default human table, and
   explicit selected-next explanation when routing is shown.
6. Define black-box non-blocking proof for server boot/listen, first routed
   request, and quota status render.
7. Define redaction and observability proof across status rows, selection
   explanations, refresh errors, traces/logs, and smoke transcripts.

## Requirements/proof matrix

Requirement / claim:
Spec captures the actual algorithm and UX contract.
Proof source:
Revised spec plus rerun `shravan-dev-workflow:spec-review-swarm` with
`phase_result: complete`.
evidence source:
phase skill result and parent inspection of review artifacts.
freshness guard:
Review must cite the revised spec path and current line coverage.

Requirement / claim:
Implementation plan is true to the goal and accepted spec.
Proof source:
`shravan-dev-workflow:plan-creation-swarm` output with explicit traceability
from every task to goal/spec requirements.
evidence source:
phase skill result and parent inspection of requirements/proof matrix.
freshness guard:
Plan must name the accepted spec review artifact and current git commit.

Requirement / claim:
Plan is not allowed to proceed if it misses full fixes or e2e proof.
Proof source:
`shravan-dev-workflow:plan-review-swarm` with zero accepted blocker findings, or
accepted findings folded back into plan creation.
evidence source:
phase skill result plus parent verification of review findings.
freshness guard:
Review must load both the plan and the accepted spec, not the plan alone.

Requirement / claim:
Runtime routing uses reset-aware burn-down assessment, not minimum-headroom-only
selection.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
unit tests, integration tests, implementation review, and parent command output.
freshness guard:
Tests must run against the implementation branch after final fixes.

Requirement / claim:
Quota status is concise and useful for humans.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
snapshot/golden tests and manual CLI output inspection.
freshness guard:
Golden output must include historical bad cases: noisy per-route rows, `pp`,
`bottleneck`, account ids, and missing selected-next explanation.

Requirement / claim:
Startup and normal requests do not block on live provider quota refresh.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
smoke test, runtime logs, and parent command output.
freshness guard:
Proof must include boot/listen, first routed request, and quota status render.

Requirement / claim:
Codex can communicate through the router end to end, including WebSocket.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
e2e command transcript using real Codex profile against local router, plus
server logs showing WebSocket path or explicit fallback behavior if fallback is
part of the accepted spec.
freshness guard:
Must run after implementation fixes in the current repo state.

Requirement / claim:
Sensitive account/token material is not leaked in user output, logs, traces, or
test transcripts.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
implementation review, redaction tests, log/trace inspection, and smoke
transcript inspection.
freshness guard:
Must inspect the final emitted output surfaces, not only data structures.

## Hard Gates

- No plan creation until the spec is revised and spec review passes.
- No implementation until plan review passes or accepted plan findings are
  folded back into plan creation.
- No implementation completion claim without unit, integration, smoke, and e2e
  proof gates accounted for.
- No goal completion while implementation review, PR readiness, or WebSocket
  end-to-end proof remains open.
- No checkpoint commit may include unrelated dirty worktree files.

## Blocked Condition

This goal is blocked only if the same blocker repeats under host blocked-state
rules and meaningful progress cannot continue without user input or external
state change. Failed review, failed tests, dirty worktree state, or missing
proof are not completion; they route back to the owning workflow.

## Checkpoint Rhythm

- After revised spec: commit scoped spec artifacts only.
- After accepted spec review: commit scoped review artifacts only.
- After accepted plan: commit scoped plan artifacts only.
- After plan review: commit review artifacts and route to implementation only
  if accepted.
- During implementation: commit only verified slices after proof.
- Before any done claim: run goal closeout audit with matrix rows and current
  evidence.
