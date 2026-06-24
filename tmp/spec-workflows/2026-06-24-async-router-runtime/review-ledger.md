# Async Router Runtime Spec Review Ledger

Date: 2026-06-24
Reviewed artifact: `async-router-runtime-spec.md`

## Coverage

- Initial review pass covered the original draft before issue-closure and
  permanent-guardrail expansion.
- Second review pass covered the expanded 752-line async runtime spec
  end-to-end after the issue-closure and guardrail additions.
- Review lanes completed:
  - whole-spec-coverage
  - architecture-boundaries + contract-and-scope
  - validation-and-testability + adversarial-crux
  - requirements-testability + validation-and-testability
  - planning-readiness + adversarial-crux + progressive-disclosure
  - spec-difference + guardrail-codification + harness-fit + security

## Accepted Findings

### Hand-rolled WebSocket stack must be permanently forbidden

Severity: blocker

Evidence:

- The live failure came from the production router owning blocking
  accept/handshake/frame-forwarding behavior instead of delegating protocol
  mechanics to the async stack.

Accepted refinement:

- The primary spec now forbids hand-rolled production HTTP/WebSocket protocol
  handling in `serve`.
- The primary spec now requires permanent static/structural and behavioral
  guardrails: no blocking sockets/tungstenite/reqwest/httparse hot path, no
  direct proxy `rusqlite`, permanent compound WebSocket regression tests, and
  CI/repo-local validation before runtime work can be called done.
- Second review found the banlist was still too weak. The primary spec now also
  requires a positive release-serve ownership check: all HTTP serving/upstream
  traffic must enter through Hyper-owned types, all WebSocket traffic must enter
  through Hyper upgrade plus `tokio-tungstenite` stream/sink types, and this
  applies across the full release `serve` dependency graph.
- The primary spec now forbids release-linked legacy blocking runtimes,
  alternate `serve` implementations, compatibility modules, hidden feature/env
  selectors, or helper-crate protocol owners.
- Planning-readiness review found the shipping boundary still looked like an
  open question. The primary spec now makes it a decision: the release
  build/dependency graph and CLI contract expose exactly one production `serve`
  runtime path; legacy blocking runtime code must be removed or test/dev-only
  and not release-linked.

### R9 could miss the exact compound live failure

Severity: blocker

Evidence:

- Spec proof bullets listed concurrency and close-while-pending as separate
  obligations.
- Existing current tests can already prove concurrency without the local-close
  trigger.
- The real observed failure involved concurrent sessions plus one WebSocket
  stuck after local close while upstream I/O was pending.

Accepted refinement:

- R9 now requires a compound live-regression proof with at least two active
  WebSocket sessions, one local close while upstream I/O is pending, cleanup of
  the affected session/upstream/registry, and successful completion of the
  surviving session.
- The primary spec now includes an `Issue Closure Contract` requiring the old
  failure shape to be reproduced against the old/blocking shape or equivalent
  failure harness and then proven gone against the async runtime.
- The primary spec now requires a long-running three-Codex-runtime soak proof,
  including sustained WebSocket activity, repeated turns or multi-step
  exchanges, log checks, and task/socket/session cleanup.
- Second review found isolated fixtures were not enough. The primary spec now
  requires the close-while-pending failure proof to traverse the real
  `codex-router serve` entrypoint: actual listener, Hyper request/upgrade path,
  WebSocket accept path, session registry, cancellation path, and cleanup path.
- Planning-readiness review found proof rows could still be softened into
  generic tasks. The primary spec now requires `plan-creation-swarm` to produce
  a mandatory proof/guardrail matrix with one row per hard gate, source anchor,
  proof layer, harness, command/execution surface, expected observation,
  durable evidence artifact, stale-proof guard, and status checkbox.
- Requirements/testability review found the soak actor model and duration were
  still too soft. The primary spec now requires three independent installed
  Codex CLI processes/runtimes through one shared router PID, at least five
  continuous minutes of overlap, at least three post-handshake interactions or
  multi-frame exchanges per runtime during that same window, and positive
  WebSocket continuity artifacts per runtime.
- Harness-fit review found the installed-Codex proof could still be satisfied
  by an under-specified local harness. The primary spec now requires one real
  `codex-router serve` process, three installed Codex CLI client processes, a
  deterministic mock upstream by default, approval-gated live OAuth/provider
  runs only when separately authorized, and one redacted evidence artifact that
  records process/session correlation, overlap timing, transport, handshake and
  activity counters, close reasons, fallback/retry absence, active-session
  high-water mark, zero-active-after state, and socket cleanup without prompts,
  tool arguments, response bodies, tokens, account labels, or provider payloads.

### R9 missed the opposite pump/backpressure direction

Severity: important

Evidence:

- The spec required bidirectional pump semantics but only named upstream-read
  pending cleanup.
- The current blocking implementation has separate local-to-upstream and
  upstream-to-local blocking phases.

Accepted refinement:

- R9 now requires proof for blocked write/backpressure direction and cleanup
  evidence through task termination and close reason.

### Same-session bidirectional interleave was not a deterministic proof row

Severity: blocker

Evidence:

- The draft said "truly bidirectional" but did not require one deterministic
  mock session where upstream emits a non-terminal event and waits for a second
  client frame before completion.

Accepted refinement:

- R9 now requires same-session bidirectional interleave while sibling WebSocket
  or HTTP/SSE traffic still progresses.

### Auth mode drifted from the greenfield source of truth

Severity: blocker

Evidence:

- The draft implied mandatory local-token auth, but the greenfield spec says
  default `serve` is tokenless loopback and local-token mode is explicit
  hardening.

Accepted refinement:

- R6 now states default tokenless loopback behavior and explicit
  `--require-local-token` hardening mode.
- R7 now scopes token-generation revocation to explicit local-token mode.
- Lifecycle flows now apply auth mode and smuggling rejection before route
  classification.
- R9 now requires separate tokenless default smoke and explicit local-token
  hardening smoke.

### Auth refresh needed a cancellation-safe commit contract

Severity: important

Evidence:

- The draft said auth owns credential resolution but did not state the commit
  invariant across secret write, active credential generation, and quota
  invalidation.

Accepted refinement:

- R5 now states provider credential refresh is an auth-owned logical commit:
  either the old generation remains authoritative, or the new secret,
  generation, and quota invalidation commit together or reconcile
  idempotently. Proxy/runtime code must not orchestrate those substeps.

### Transport pumps could hide persistence waits behind traits

Severity: important

Evidence:

- The draft forbade raw SQLx in proxy loops but did not forbid abstracted
  state/secret/durable audit calls that gate frame/body forwarding.

Accepted refinement:

- R8, the disallowed edges, and lifecycle contracts now say WebSocket frame
  pumps and HTTP/SSE body pumps must not await SQLite, secret-store, or durable
  audit persistence before forwarding data or completing close paths.
- Guardrail review found this still needed a permanent anti-regression hook, not
  only prose. The primary spec now requires a pump-side side-effect guardrail:
  pumps may emit bounded in-memory events, persistence must be deferred outside
  forwarding and close-progress gates, and a structural or behavioral regression
  check must fail if slow or blocked state/audit/secret sinks delay forwarding or
  session close completion.

### Post-upgrade pre-upstream WebSocket failures were underspecified

Severity: important

Evidence:

- The draft distinguished pre-upgrade rejection and post-upstream duplex close,
  but did not map first-frame timeout, selection failure, credential failure, or
  upstream-open failure to deterministic local outcomes.

Accepted refinement:

- R3 now defines a deterministic local WebSocket close contract and redacted
  trace/audit outcome for each post-upgrade, pre-upstream failure class.

### Guardrail scope could still miss helper crates or dev/runtime loopholes

Severity: blocker

Evidence:

- Negative checks scoped only to obvious proxy files can miss a renamed helper
  crate, alternate runtime, hidden feature switch, or release-linked blocking
  compatibility path.

Accepted refinement:

- The primary spec now scopes guardrails to the full non-test production
  `serve` reachability path across all crates/modules in the release binary.
- Test support, mock upstreams, ignored smoke harnesses, route-native fixtures,
  and dev-only helper binaries may use low-level sockets or blocking protocol
  crates only when they are not reachable from, linked as, or selectable by the
  release `codex-router serve` command.
- The release build/dependency graph and CLI contract must expose exactly one
  production `serve` runtime path.

## Pending Review Inputs

- None. Accepted findings from the completed spec-review pass have been routed
  back into `async-router-runtime-spec.md`.

## Current Verdict

Spec review findings addressed. Ready for `shravan-dev-workflow:orchestrator-goal`
to open the goal-backed delivery workflow, with `plan-creation-swarm` as the
next phase. The implementation plan must carry the `Issue Closure Contract`,
the `Permanent Regression Guardrails`, and the mandatory proof/guardrail matrix
as hard final acceptance gates.
