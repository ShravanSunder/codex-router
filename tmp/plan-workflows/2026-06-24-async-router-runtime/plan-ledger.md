# Async Router Runtime Plan Ledger

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Status: accepted after focused plan-review-swarm

## Source Coverage

- Spec:
  `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
  - `wc -l`: 759
  - parent-read coverage: lines 1-759
- Review ledger:
  `tmp/spec-workflows/2026-06-24-async-router-runtime/review-ledger.md`
  - `wc -l`: 240
  - parent-read coverage: lines 1-240
- Goal details:
  `tmp/workflow-state/2026-06-24-async-router-runtime/details.md`
- Transition log:
  `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`

## Lane Packets And Artifacts

- codebase-boundary:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/codebase-boundary.md`
- validation-proof:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/validation-proof.md`
- security-reliability:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/security-reliability.md`
- vertical-slice-decomposition:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/vertical-slice-decomposition.md`
- execution-order:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/execution-order.md`
- scope-and-proof-fit:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/scope-and-proof-fit.md`

## Accepted Lane Evidence

Accepted:

- current release `serve` path is still blocking/manual and rooted at
  `crates/codex-router-cli/src/lib.rs`, `crates/codex-router-proxy/src/server.rs`,
  `crates/codex-router-proxy/src/websocket.rs`, and
  `crates/codex-router-proxy/src/upstream.rs`
- this cannot be planned as a narrow WebSocket-only patch
- SQLx state/auth boundary is first-class scope because accepted R5 forbids
  runtime SQLite through `rusqlite` or proxy-owned raw SQL
- WebSocket transport must be planned with registry/revocation/close proof
  because separating them creates false-done risk
- installed-Codex proof must be split into early real-serve smoke and final
  three-runtime e2e/soak
- guardrails need early inventory plus final enforced release-reachability check
- proof matrix must preserve one row per hard gate

Rejected:

- generic "add integration tests" task
- WebSocket-only fix
- accept-loop-only fix
- hidden release-linked blocking runtime or compatibility `serve`
- mock-only final e2e
- live OAuth/provider traffic as default proof
- session picker/OAuth/keychain/quota redesign in this runtime goal

Deferred to implementation details but kept behind explicit row commands:

- exact SQLx migration layout and compile-time query checking timing
- exact secret-store operations that need bounded `spawn_blocking`

No proof command surface is deferred out of the plan contract. Every proof row
runs through `scripts/proof-matrix.sh <ROW>`; implementation slices create or
fill the row-specific target behind that command before marking a row green.

## Plan-Review Reduction

Initial verdict: `needs revision`, addressed in the plan/spec before
implementation.

Focused re-review verdict: ready for `implementation-execute-plan`.

Accepted blocker/important findings folded into
`implementation-plan.md`:

- matrix rows needed an executable command/status contract
- T1 checkpoint wording created a hard-cutover contradiction
- T7 helper wording allowed in-process proof instead of a real child
  `codex-router serve` process
- T6 final guardrails were ordered before installed-Codex e2e/soak harness rows
  existed
- T7/T8 needed child-process/barrier semantics for actual concurrent installed
  Codex runtimes
- release reachability checker needed an algorithm contract and checker
  self-tests
- pump-side side-effect proof needed HTTP/SSE and WebSocket variants plus sink
  saturation behavior
- old-failure red proof and async green proof needed separate rows
- auth-smuggling, first-frame close classes, exact first-frame forwarding,
  account pinning, selection preservation, credential commit unit semantics,
  startup bind-before-refresh, unbounded buffering, detached readers, and
  redaction allowlist validation needed explicit rows or slice contracts
- Clap handling needed an explicit rule: if touched CLI parsing changes, convert
  the touched command contract to Clap and add parser proof

Accepted spec clarification folded into
`async-router-runtime-spec.md`:

- Hyper owns the local WebSocket upgrade response; after Hyper accepts the
  upgrade, router code wraps the stream with
  `WebSocketStream::from_raw_socket` or `from_partially_read`; local
  `accept_async`/`accept_hdr_async` after Hyper upgrade is forbidden.

Rejected or not adopted:

- no implementation code changes during plan review
- no live provider/OAuth proof requirement added
- no session picker, OAuth/keychain, quota algorithm, or retry/fallback scope
  added

## Parent Synthesis Decisions

1. The plan uses eight implementation tasks including the planning gate:
   T0 through T8.
2. T1 and T2 are serial before transport work.
3. T3 and T4 may parallelize only after shared state/auth contracts settle.
4. T5 owns WebSocket pumps and registry/observability proof together.
5. T6 is split into early inventory, release-runtime structural enforcement,
   and final permanent-suite/CI enforcement after T7/T8.
6. T7 and T8 are serial final proof harness work and must use a spawned child
   `codex-router serve` process, not in-process runtime helpers.
7. Implementation must route to `implementation-review-swarm` after proof and
   then `implementation-pr-wrapup`; merge remains out of scope.

## Plan-Review Focus

The reviewer must attack these risks:

- any missing proof row from R9, Issue Closure Contract, Permanent Regression
  Guardrails, or Acceptance Gate
- any slice that can claim completion while release `serve` still reaches
  blocking runtime code
- any WebSocket slice that omits registry/revocation/close-proof ownership
- any installed-Codex proof that does not traverse real `codex-router serve`
- any structural guardrail that checks only obvious files instead of release
  reachability
- any scope creep into session picker, OAuth/login/keychain, quota algorithm, or
  live-provider proof
- any evidence artifact that could leak prompts, tool arguments, response
  bodies, tokens, refresh tokens, provider payloads, account labels, or raw
  provider account ids

## Completion Receipt

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/plan-ledger.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Plan-review-swarm findings were folded back into
the spec and implementation plan, focused re-review found no remaining blocker,
and implementation may start against the reviewed proof matrix.
