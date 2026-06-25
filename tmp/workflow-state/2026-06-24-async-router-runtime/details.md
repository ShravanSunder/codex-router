# Goal Details: 2026-06-24-async-router-runtime

## Objective

Deliver the async pure-proxy runtime repair for `codex-router` through the full
goal-backed lifecycle, ending at a PR-ready artifact that proves multiple
concurrent installed Codex WebSocket clients work through one router process.

Completion requires:

- accepted async runtime spec and review ledger
- implementation plan that traces every hard proof gate to the spec
- adversarial plan review passed or findings folded back into the plan
- implementation completed for the accepted plan
- implementation review findings addressed or explicitly rejected with evidence
- full proof loop captured, including three concurrent installed Codex runtimes
  through one `codex-router serve` PID
- PR created or updated and proven ready, but not merged unless separately
  authorized

## Current Workflow

- Current workflow: `shravan-dev-workflow:orchestrator-goal`
- Latest completed workflow: `shravan-dev-workflow:implementation-execute-plan`
- Phase result: `complete`
- Next workflow: `shravan-dev-workflow:implementation-pr-wrapup`
- Current focus: close the PR-ready non-merge boundary. Implementation,
  post-review fixes, current-head proof receipts, and final push are complete at
  `ff54355f3fc11898f972b0c0eb39bc55298228ad`, but no open PR currently exists
  for this already-pushed `main` branch state.

## Required Reading

Resolve these paths in the current checkout:

- `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
- `tmp/spec-workflows/2026-06-24-async-router-runtime/review-ledger.md`
- `tmp/spec-workflows/2026-06-24-async-router-runtime/swarm-ledger.md`
- `tmp/workflow-state/2026-06-24-async-router-runtime/details.md`
- `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`

Historical lane files under
`tmp/spec-workflows/2026-06-24-async-router-runtime/lanes/` are supporting
evidence only. The primary spec and review ledger are authoritative for planning.

Implementation artifacts:

- `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/plan-ledger.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/reviews/plan-review-report.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/`
- `tmp/research-workflows/2026-06-24-codex-websocket-invariants/research-ledger.md`

## Scope

In scope:

- replace the production `serve` runtime with one Tokio-owned async runtime
- move HTTP/SSE serving and upstream proxying to Hyper/hyper-util
- move WebSocket upgrade, handshake, and frame streams to `tokio-tungstenite`
- preserve local auth, route classification, account selection, credential
  injection, affinity, quota/state lookup semantics, and redacted observability
- move runtime SQLite access to state-owned SQLx contracts
- update CLI command contracts with Clap where touched by runtime work
- leave permanent structural and behavioral guardrails against hand-rolled
  production HTTP/WebSocket regression
- prove the observed stuck multi-client WebSocket issue is gone through the real
  `codex-router serve` path and installed Codex CLI runtime path

Out of scope unless explicitly brought back into this goal:

- session picker or resume UX
- OAuth/login/keychain redesign
- quota algorithm redesign beyond preserving fast persisted selection inputs
- disabling WebSockets
- retry, circuit-breaker, or router-owned fallback policy
- release-linked legacy blocking runtime, alternate `serve` implementation, or
  compatibility runtime
- merging the PR
- destructive cleanup of unrelated dirty worktree files

## Required Proof Matrix Seeds

Plan creation must expand these seeds into one row per hard gate with source
spec anchor, proof layer, harness, command/execution surface, expected
observation, durable evidence artifact, stale-proof guard, and status checkbox:

- unit proof for route classification, local auth, first request data frame
  parsing, affinity, header sanitation, selection preservation, and
  cancellation-safe credential commit semantics
- integration proof for concurrent WebSockets, stalled upstream plus sibling
  progress, mixed WebSocket plus HTTP/SSE progress, same-session bidirectional
  interleave, blocked write/backpressure cleanup, post-upgrade/pre-upstream
  failure outcomes, unsupported routes, local-auth smuggling rejection, affinity
  recording, token-mode revocation, registry cleanup, credential refresh
  cancellation, and pump non-blocking side effects
- real `codex-router serve` close-while-pending proof through the production
  listener, Hyper upgrade path, WebSocket accept path, session registry,
  cancellation path, and cleanup path
- installed Codex smoke/e2e with default tokenless profile
- three independent installed Codex CLI processes/runtimes through one shared
  router PID, with WebSocket traffic and multi-step/tool-call-style interaction
- long-running soak: all three installed Codex runtimes overlap for at least five
  continuous minutes, each produces repeated post-handshake activity, no fallback
  or reconnect loop occurs, and task/socket/session cleanup is proven
- structural guardrail: no hand-rolled production HTTP/WebSocket stack reachable
  from release `serve`
- positive ownership guardrail: release `serve` traffic enters through Hyper and
  `tokio-tungstenite` types across the full release dependency graph
- pump-side side-effect guardrail: slow or blocked SQLite/secret/audit sinks
  cannot delay frame/body forwarding or close completion

## Stop Condition

Goal completes only when implementation is complete, required proof gates pass or
are explicitly not-applicable, implementation review findings are resolved or
explicitly rejected with evidence, PR is created/updated and proven ready, and
merge is not performed unless explicitly authorized.

## Blocked Condition

Blocked only if the same material blocker recurs under host blocked-state rules,
such as missing/contradictory goal pointers, inability to run the required
installed Codex e2e proof, inaccessible remote preventing PR-ready proof, or user
approval required for a live-provider run that becomes necessary for the next
transition.

## Checkpoint Rhythm

- Record official orchestrator transitions in
  `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`.
- Commit scoped durable artifacts at verified lifecycle checkpoints.
- Keep unrelated untracked artifacts out of this goal unless explicitly adopted.
- Phase skills must return `phase_result`, `evidence`,
  `recommended_next_workflow`, and `recommended_transition_reason`.
