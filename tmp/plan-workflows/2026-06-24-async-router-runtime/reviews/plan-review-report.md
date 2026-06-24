# Async Router Runtime Plan Review Report

Date: 2026-06-24
Workflow: `shravan-dev-workflow:plan-review-swarm`
Review target: `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
Accepted source: `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`

## Coverage

- Plan reviewed: 709 lines before revision
- Spec reviewed: 752 lines before revision; current revised spec is 759 lines
- Spec review ledger reviewed: 240 lines
- Plan-review packet written:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/reviews/plan-review-packet.md`
- Lanes run:
  - whole-plan-cohesion: needs revision
  - spec-compliance: needs revision
  - testability-validation: needs revision
  - security-reliability: needs revision
  - architecture-execution-scope: needs revision
  - adversarial-design: needs revision

## Verdict

Initial verdict: `needs revision`.

Revised verdict after parent reduction, plan/spec edits, and focused re-review:
`ready for implementation-execute-plan`.

Do not skip proof gates during implementation. The next workflow is
`shravan-dev-workflow:implementation-execute-plan`.

## Accepted Findings

Blockers accepted and addressed:

- proof matrix lacked an executable command/status contract
- T1 implied an impossible hard cutover before HTTP/WS transports existed
- T7 allowed helper-shaped in-process proof instead of a real child
  `codex-router serve` process
- T6 final guardrails were ordered before T7/T8 harness rows existed
- real `serve` and three installed-Codex runtime proof were not operationally
  pinned as child processes behind a shared router PID/barrier

Important findings accepted and addressed:

- release reachability checker needed an algorithm contract and self-tests
- WebSocket auth-smuggling/order proof was too coarse
- first-frame bounded parsing and close taxonomy were too vague
- credential refresh cancellation needed forced half-commit failpoints
- evidence redaction needed row-local and aggregate allowlist validation
- pump-side non-blocking proof needed sink saturation behavior and HTTP/SSE plus
  WebSocket variants
- old-failure red evidence and async green evidence needed separate rows
- selection preservation and credential commit semantics needed unit rows
- first-frame exact forwarding, account pinning, startup bind-before-refresh,
  unbounded buffering, and detached reader guards needed explicit coverage
- Clap handling needed an explicit rule if CLI parsing is touched
- Hyper-local WebSocket upgrade handoff needed a spec/plan clarification

## Plan Edits Applied

- Added `Plan-Review Revision Decisions` to the plan.
- Clarified T1 as async runtime substrate and cutover seam, not release-selected
  incomplete `serve`.
- Expanded T2 to own async request-time selector, credential, affinity, and
  state contracts plus refresh failpoints.
- Clarified T4 local WebSocket handoff:
  Hyper accepts local upgrade, then `tokio-tungstenite` wraps the upgraded
  stream without a second local handshake.
- Expanded T5 pump saturation, bounded event, no-unbounded-channel, and
  no-detached-reader requirements.
- Split T6 into T6a/T6b/T6c and made final guardrails run after T8.
- Split T7 into child-process real-serve supervision plus installed-Codex smoke.
- Added T8 supervisor/barrier semantics for three installed Codex child
  processes through one router PID.
- Updated DAG and checkpoint rhythm.
- Added proof matrix command/status contract:
  `scripts/proof-matrix.sh <ROW>`.
- Added missing/split proof rows:
  U-06, U-07, I-05a, I-05b, I-17a, I-17b, I-20, I-21, G-22, G-23.
- Added matrix status ledger, stale-artifact check, row-local redaction, and
  aggregate redaction requirements.
- Updated plan ledger with accepted/rejected review findings.

## Spec Edit Applied

- Clarified local WebSocket ownership:
  Hyper owns local upgrade response; after Hyper accepts the upgrade, router
  wraps the stream with `WebSocketStream::from_raw_socket` or
  `from_partially_read`; local `accept_async`/`accept_hdr_async` after Hyper
  upgrade is forbidden.

## Remaining Review Focus

Focused re-review checked:

- every accepted finding above is really represented in the revised plan/spec
- no new contradiction was introduced by T6c after T8
- `scripts/proof-matrix.sh <ROW>` is a sufficient executable command contract
  for planning, with row-specific implementation required before green status
- matrix row count and status contract still cover all hard gates
- no implementation has started

Focused re-review results:

- focused-testability-corrections: ready
- focused-architecture-corrections: ready
- focused-whole-plan-corrections: needs revision with two non-blocking
  important findings

Focused whole-plan findings addressed by parent:

- T6a ordering made consistent: T6a starts after T2, and the DAG/commit rhythm
  now match that ordering.
- Revised spec line counts updated to 759, and matrix source labels were moved
  from fragile line-number anchors to stable section anchors.

## Completion Receipt

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/plan-ledger.md`
- `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/reviews/plan-review-report.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Initial plan-review blockers were addressed and
focused re-review found no remaining blocker; implementation may start with the
reviewed plan and proof matrix as the execution contract.
