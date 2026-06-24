# Async Router Runtime Implementation Plan

Date: 2026-06-24
Status: revised after plan-review-swarm findings
Goal id: `2026-06-24-async-router-runtime`

## Objective

Replace the production `codex-router serve` runtime with one async pure-proxy
runtime based on Tokio, Hyper, `tokio-tungstenite`, and SQLx-owned state. The
implementation must prove that multiple concurrent installed Codex WebSocket
clients can use one router process without WebSocket fallback, stalls, leaked
sessions, or hidden blocking protocol/runtime paths.

Terminal condition for this goal is PR-ready, not merged.

## Source Coverage

Accepted source artifacts:

- `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
  - line count: 759
  - parent coverage: read lines 1-759 in chunks
- `tmp/spec-workflows/2026-06-24-async-router-runtime/review-ledger.md`
  - line count: 240
  - parent coverage: read lines 1-240
- `tmp/workflow-state/2026-06-24-async-router-runtime/details.md`
- `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`

Planning lanes:

- `lanes/codebase-boundary.md`
- `lanes/validation-proof.md`
- `lanes/security-reliability.md`
- `lanes/vertical-slice-decomposition.md`
- `lanes/execution-order.md`
- `lanes/scope-and-proof-fit.md`

## Non-Goals

- no session picker or resume UX
- no OAuth/login/keychain redesign
- no quota algorithm redesign beyond preserving fast persisted selector inputs
- no disabling WebSockets
- no router-owned retry, fallback, or circuit-breaker policy
- no release-linked legacy blocking runtime, alternate `serve`, or compatibility
  runtime
- no live OAuth/provider proof unless separately approved
- no merge

## Implementation Strategy

The work is not a WebSocket-only patch. The current release path still has
blocking TCP accept, manual HTTP/WebSocket protocol ownership, blocking
WebSocket pumps, blocking HTTP upstream transport, proxy-owned blocking SQLite
state, and a smoke harness that does not traverse real `codex-router serve`.

The plan therefore uses source-owned vertical slices:

1. establish one async production `serve` ownership path
2. move runtime state/auth interactions to SQLx-owned async contracts
3. cut HTTP/SSE to Hyper
4. cut WebSocket accept/routing to Hyper upgrade + `tokio-tungstenite`
5. implement WebSocket duplex pumps with registry/revocation/close proof
6. lock down structural guardrails
7. upgrade installed-Codex proof from smoke to concurrent e2e and soak

## Plan-Review Revision Decisions

Plan-review-swarm returned `needs revision`. The parent accepted these findings
and folded them into this plan before implementation:

- The proof matrix must be executable. Every row is initially unchecked and must
  be runnable through `scripts/proof-matrix.sh <ROW>` before that row can be
  marked green. Slice work must create the underlying test, script, fixture, or
  validator for its rows before claiming the row complete.
- T1 is a runtime substrate and cutover seam, not an incomplete release-selected
  `serve` cutover. The first release-selected `serve` cutover happens only after
  T3, T4, and T5 compose through the shared async runtime.
- Local WebSocket upgrade ownership is explicit: Hyper owns local request
  validation and `101 Switching Protocols`; the upgraded stream is then wrapped
  with `tokio_tungstenite::WebSocketStream::from_raw_socket` or
  `from_partially_read`. `accept_async` and `accept_hdr_async` are not allowed
  on the local Hyper-upgraded stream.
- Installed-Codex acceptance proof must spawn the built `codex-router serve`
  binary as a child process and record router PID, argv, listener, readiness,
  child cleanup, and one shared PID across clients. In-process
  `LoopbackRouterRuntime` helpers do not count for S/E acceptance rows.
- The three-runtime soak must use an explicit supervisor/barrier: one router
  child process, three installed Codex child processes released together, and a
  deterministic mock upstream that holds all three WebSocket sessions active
  during the same five-minute overlap window.
- T6 is split into T6a inventory, T6b release-runtime structural guard after T5,
  and T6c final permanent-suite/CI guardrail after T7/T8.
- Redaction proof is allowlist-based and aggregate: every evidence-producing row
  must pass row-local redaction validation, and PR readiness must run a final
  aggregate scan over the evidence directory.
- Pump-side side-effect proof covers both HTTP/SSE body pumps and WebSocket
  frame/close pumps, including slow or blocked affinity recorder, audit sink,
  state sink, and secret-store boundary behavior.
- T2 explicitly owns async request-time traits for account selection,
  credential resolution, affinity recording, and selector state, not only SQLx
  storage internals.

## Vertical Slice Cards

### T0. Planning Acceptance And Source Freeze

Source anchors:

- spec R9 proof matrix contract: lines 364-379
- Issue Closure Contract: lines 381-465
- Permanent Regression Guardrails: lines 467-532
- Acceptance Gate: section anchor `Acceptance Gate For This Spec`

Behavior:

- freeze this plan and matrix as the source for implementation
- route to `plan-review-swarm` before code changes
- accepted plan-review findings must return to plan creation before execution

Allowed writes:

- `tmp/plan-workflows/2026-06-24-async-router-runtime/*`
- `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`

Checkpoint:

- plan-review-swarm returns `complete` or findings are folded back

Proof:

- plan artifact contains one row per hard proof/guardrail gate
- plan ledger records accepted lane evidence and unresolved decisions

Split trigger:

- if plan-review finds missing matrix rows, do not start implementation

### T1. Single Async Production Serve Path

Source anchors:

- required stack and no alternate protocol owner: spec lines 62-124
- R1 async runtime ownership: spec lines 126-139
- guardrails: spec lines 475-510

Behavior:

- introduce the async runtime substrate and typed release cutover seam for
  `codex-router serve`
- define the shared Hyper switchpoint: one module owns local Hyper request
  dispatch and upgrade branching; T3 owns HTTP/SSE bodies/upstream; T4 owns
  WebSocket accepted-stream/session creation
- define the Tokio-owned listener, supervised connection tasks, cancellation,
  graceful shutdown, and close reasons
- do not make the incomplete async substrate the release-selected `serve` path
  until T3, T4, and T5 compose through it
- preserve tokenless default and explicit local-token hardening config
- if CLI parsing is touched, convert the touched command contract to Clap in
  this slice and add parser proof rows before proceeding

Likely writes:

- `Cargo.toml`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-proxy/src/server.rs`
- possible new proxy runtime modules under `crates/codex-router-proxy/src/`

Dependencies:

- starts after T0

Checkpoint:

- async runtime substrate and cutover seam compile
- route/upgrade dispatch ownership is explicit and tested once
- legacy release `serve` may remain selected only until T3/T4/T5 are integrated;
  no checkpoint may call the release path single/async complete until final
  reachability guardrails pass

Proof:

- unit/compile proof for typed runtime config
- integration proof that runtime binds loopback and shuts down cleanly
- structural inventory of release `serve` reachability path
- Clap parser proof if CLI parsing is touched

Split trigger:

- split if this task starts implementing HTTP/SSE or WebSocket transport logic
  beyond runtime shell and shared interfaces

### T2. SQLx Async State And Auth Boundary

Source anchors:

- R5 SQLx state boundary: spec lines 210-233
- dependency map and disallowed edges: spec lines 581-602
- credential refresh accepted finding: review ledger lines 155-170

Behavior:

- move runtime-facing state to `codex-router-state` SQLx-owned async contracts
- preserve schema/migration ownership in state
- ensure proxy runtime does not own raw SQLx queries or direct `rusqlite`
- preserve selector quota/affinity/account semantics
- preserve auth-owned credential refresh logical commit
- introduce async request-time contracts for `AccountDecisionSelector`,
  `ProviderCredentialResolver`, affinity owner/secret recording, and selector
  state reads so Hyper services do not block Tokio workers or hold sync locks
  across awaits
- define failpoints for credential refresh before refresh response handling,
  after secret write, before state commit, after state commit, and during
  concurrent resolver reads

Likely writes:

- `Cargo.toml`
- `crates/codex-router-state/Cargo.toml`
- `crates/codex-router-state/src/*`
- `crates/codex-router-auth/Cargo.toml`
- `crates/codex-router-auth/src/resolver.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/credential_runtime.rs`
- `crates/codex-router-proxy/src/local_auth.rs` if auth trait boundaries move
- proxy runtime call sites in `server.rs`, `http_sse.rs`, and `websocket.rs`

Dependencies:

- starts after T1 runtime interface exists

Checkpoint:

- runtime callers use async state/auth handles for request-time selection,
  affinity lookup/recording, quota reads, and credential resolution
- auth resolver owns refresh commit semantics; proxy does not sequence secret
  write, generation advance, and quota invalidation

Proof:

- unit proof for repository semantics
- unit proof for selection preservation
- unit proof for credential commit state transitions
- integration proof against real SQLite for async account/quota/affinity state
- credential refresh cancellation proof
- structural proof: no direct `rusqlite` in production proxy runtime path

Split trigger:

- split into T2a request-time async repositories and T2b auth refresh commit if
  SQLx migration and credential commit proof cannot land safely in one slice

### T3. Hyper HTTP/SSE Proxy Path

Source anchors:

- R2 Hyper HTTP/SSE proxy: spec lines 140-148
- R4 no hidden buffering/protocol rewriting: spec lines 200-208
- HTTP lifecycle: spec lines 606-621

Behavior:

- local HTTP/SSE serving enters router through Hyper request/service types
- upstream HTTP/SSE uses Hyper client/response body types
- preserve route classification, local auth, selection, credential injection,
  sanitized headers, streaming order, and affinity semantics
- defer durable affinity/audit persistence outside response-body forwarding
  progress

Likely writes:

- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/upstream.rs`
- `crates/codex-router-proxy/src/headers.rs`
- `crates/codex-router-proxy/src/routes.rs`
- tests in `crates/codex-router-proxy/src/lib.rs`

Dependencies:

- starts after T1 and request-time parts of T2

Checkpoint:

- HTTP/SSE path works through real async runtime and Hyper upstream transport
- mixed progress proof exists for HTTP/SSE while WebSocket is stalled

Proof:

- unit proof: route classification, local auth, header sanitation, affinity
  extraction
- integration proof: unsupported routes reject before selection/upstream,
  auth-smuggling reject before upstream, local auth never leaks, previous-response
  affinity preserved
- smoke proof: real `codex-router serve` HTTP/SSE path

Split trigger:

- split local Hyper serving from upstream Hyper transport if body abstraction
  conversion becomes too broad for one proven slice

### T4. WebSocket Accept, First Frame, And Pre-Upstream Failure Contract

Source anchors:

- R3 async WebSocket pure proxy: spec lines 150-199
- R6 auth/header invariants: spec lines 235-253
- post-upgrade/pre-upstream accepted finding: review ledger lines 193-206

Behavior:

- local WebSocket upgrade enters through Hyper upgrade plumbing
- local side uses Hyper request validation, Hyper `101 Switching Protocols`,
  and `hyper::upgrade::on`; the upgraded stream is wrapped with
  `tokio_tungstenite::WebSocketStream::from_raw_socket` or
  `from_partially_read` in server role
- upstream side uses the `tokio-tungstenite` client handshake
- local `accept_async` or `accept_hdr_async` after Hyper upgrade is forbidden
- bounded first-frame wait validates only routing metadata
- first-frame policy defines timeout, max first-message bytes, allowed frame
  type, metadata-only parse fields, and close classes for timeout, too-large,
  non-text, malformed, unexpected, selection, credential, and upstream-open
- account selection and upstream credential resolution happen after first frame
- post-upgrade/pre-upstream failures close locally with deterministic redacted
  close reasons and no router retry/fallback/account switch

Likely writes:

- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- possible new `crates/codex-router-proxy/src/websocket_session.rs`
- tests in `crates/codex-router-proxy/src/lib.rs`

Dependencies:

- starts after T1 and request-time parts of T2

Checkpoint:

- real serve-path upgrade accepts locally before upstream open
- first-frame policy and pre-upstream failure matrix are proven

Proof:

- unit proof: first-frame validation, bounded metadata parsing,
  auth-smuggling rejection
- integration proof: first-frame timeout, invalid frame, selection failure,
  credential failure, and upstream-open failure each produce deterministic local
  close and redacted audit/trace outcome
- integration proof: first frame forwarded to upstream unchanged and selector
  invoked once per WebSocket session
- smoke proof: fragmented upgrade still works through real serve path

Split trigger:

- split if post-upstream pump implementation starts leaking into this slice

### T5. WebSocket Duplex Pumps, Session Registry, Revocation, And Close Semantics

Source anchors:

- R1 task supervision: spec lines 130-138
- R3 bidirectional forwarding: spec lines 191-195
- R4 backpressure: spec lines 207-208
- R7 revocation: spec lines 254-265
- R8 observability: spec lines 267-287
- R9 integration proof rows: spec lines 296-337
- Issue Closure Contract: spec lines 381-465

Behavior:

- replace response-turn-gated blocking tunnel with supervised async
  bidirectional pumps
- local-to-upstream and upstream-to-local pumps make independent progress
- session registry stores session ids, token generation, cancellation handles,
  active state, and close reason, not cloned sockets
- revocation closes only stale sessions in explicit token-required mode
- durable state/secret/audit work cannot gate frame forwarding or close progress
- affinity/audit side effects are emitted as bounded in-memory events and
  persisted after forwarding/close progress
- pump event queues have an explicit saturation policy: bounded capacity,
  no await on enqueue from frame/body forwarding or close-progress paths, and
  drop/coalesce/fail-open behavior documented per event type
- no unbounded pump channels or detached production reader tasks are allowed

Likely writes:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- possible new proxy session/registry/observability modules
- tests in `crates/codex-router-proxy/src/lib.rs`

Dependencies:

- starts after T4
- depends on T2 for async state/auth handles

Checkpoint:

- one pinned session model is inspectable
- pumps are full-duplex and cancellation-safe
- registry high-water/zero-active-after can be observed
- close reasons are emitted redacted

Proof:

- same-session bidirectional interleave before `response.completed`
- compound close-while-pending with sibling WebSocket progress
- stalled WebSocket while HTTP/SSE completes
- upstream close while local idle
- blocked write/backpressure cleanup
- token generation revocation
- registry cleanup after local close/upstream close/revocation/shutdown
- pump-side slow sink regression
- structural proof against unbounded pump channels and detached production
  reader tasks

Split trigger:

- split into T5a duplex pumps and T5b registry/observability only if the
  combined proof cannot pass inside one slice; do not treat registry as aftercare

### T6. Structural Guardrails

Source anchors:

- Permanent Regression Guardrails: spec lines 467-532
- guardrail scope accepted finding: review ledger lines 208-227

Behavior:

- add permanent repo-local structural checks that fail if release `serve`
  reintroduces a hand-rolled production HTTP/WebSocket stack
- add positive ownership checks for Hyper and `tokio-tungstenite`
- distinguish release-reachable production code from test/dev-only fixtures
- implement a release reachability checker based on the release binary target,
  `cargo metadata`/dependency graph input, and a source allow/deny scan over
  non-test reachable modules
- add checker self-tests: one reachable bad-code fixture must fail, and one
  unreachable test/dev fixture may pass
- integrate cheap deterministic guardrail rows into CI; final manual soak rows
  remain PR-ready gates when too expensive for every CI run

Likely writes:

- `Cargo.toml`
- `crates/codex-router-proxy/Cargo.toml`
- repo-local validation script under `scripts/` or `tests/`
- `.github/workflows/ci.yml` if the guardrail is cheap and deterministic
- tests or fixtures proving checker behavior

Dependencies:

- T6a inventory starts after T2, after the async runtime and state/auth
  boundaries are named
- T6b release-runtime structural guard completes after T3/T4/T5 merge
- T6c final permanent-suite/CI guard completes after T7/T8 create or update the
  installed-Codex e2e and soak harnesses

Checkpoint:

- guardrail fails on known bad patterns in release reachability
- guardrail permits test-support/mock/dev-only low-level sockets when not linked
  as release `serve`

Proof:

- structural rows G-01 through G-21 in the matrix
- CI/repo-local validation row
- final G-01 through G-23 run after T8 and after all harness, manifest, and CI
  edits

Split trigger:

- split if release reachability cannot be distinguished from test/dev helpers
  with a simple command

### T7. Real Serve Installed-Codex Smoke Harness

Source anchors:

- R9 smoke/e2e rows: spec lines 338-347
- Issue Closure installed-Codex rows: spec lines 421-433
- accepted harness finding: review ledger lines 94-102

Behavior:

- T7a adds child-process supervision for the built `codex-router serve` binary:
  router PID, argv, stdout readiness, listener address, shutdown, and cleanup
- no in-process `LoopbackRouterRuntime::start`, `run_with_io`, or helper that
  bypasses the child `codex-router serve` process counts for S/E acceptance rows
- T7b updates installed-Codex mock smoke to use that child `serve` process
- keep deterministic mock upstream by default
- prove tokenless default profile and explicit token hardening smoke paths
- produce redacted evidence artifacts with schema allowlist validation and
  negative canaries for tokens, account labels, prompts, tool args, response
  bodies, provider payloads, and raw account ids

Likely writes:

- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`
- possible test-support mock upstream modules

Dependencies:

- final smoke starts after T5 and after T6b release-runtime guardrails pass
- artifact schema and mock-upstream scaffolding may pre-stage after T3/T4, but
  cannot mark S rows green until the child-process `serve` path is used

Checkpoint:

- installed Codex can run HTTP/SSE and WebSocket through real `codex-router
  serve` against deterministic mock upstream
- S-03/S-04 artifacts include router binary path, PID, argv, listener, registry
  session ids, and child cleanup result

Proof:

- smoke rows S-01 through S-04
- redaction artifact validation

Split trigger:

- split if one harness tries to do smoke, concurrent e2e, and five-minute soak
  in one step

### T8. Three-Runtime E2E And Soak Harness

Source anchors:

- R9 e2e/soak rows: spec lines 339-359
- Issue Closure soak acceptance signals: spec lines 435-461

Behavior:

- run three independent installed Codex CLI processes/runtimes through one
  shared `codex-router serve` PID
- one supervisor starts the router child process, then starts three installed
  Codex child processes before releasing a barrier
- the deterministic mock upstream holds all three WebSocket sessions active for
  the same five-minute overlap window and emits per-runtime frame/activity
  counters during that window
- maintain at least five minutes of shared WebSocket overlap
- require at least three post-handshake model interactions or multi-frame
  exchanges per runtime during the overlap
- include one deterministic multi-step mock transcript: upstream emits a
  non-terminal event, waits for a second local client frame, then completes only
  after receiving that frame
- emit one redacted continuity/cleanup artifact

Likely writes:

- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`
- evidence schema helpers under test-support

Dependencies:

- starts after T7 real-serve smoke is green and T5 registry observability exists

Checkpoint:

- final e2e and soak evidence exists and passes redaction checks
- E-01/E-02 fail unless active-session high-water is 3 during the measured
  window, each child has frames inside that same window, and all clients share
  the recorded router PID

Proof:

- rows E-01 through E-09

Split trigger:

- if five-minute soak is too expensive for ordinary CI, keep deterministic
  smoke/e2e in CI and require `scripts/proof-matrix.sh E-02` to produce a fresh
  ignored/manual soak artifact for PR readiness

## Execution DAG

```text
T0: accepted plan + review gate
  |
T1: async runtime substrate + shared Hyper switchpoint
  |
T2: SQLx async state/auth boundary
  |
  +-- T3: Hyper HTTP/SSE path
  |
  +-- T4: WebSocket accept + first-frame/pre-upstream contract
  |
  +-- T6a: early structural guardrail inventory
  |
T5: WebSocket duplex pumps + registry + revocation + close semantics
  |
T6b: release-runtime structural guardrails against merged serve path
  |
T7: real-serve installed-Codex smoke harness
  |
T8: three-runtime e2e + five-minute soak + final proof pack
  |
T6c: final permanent-suite + CI guardrail pass
  |
implementation-review-swarm
  |
implementation-pr-wrapup
```

Parallelism rule:

- T3 and T4 can proceed in parallel only after T2 settles the shared state/auth
  contracts.
- T5 must include registry/revocation/observability and close proof; do not split
  those into post-implementation cleanup unless proof forces a planned split.
- T6 has early inventory, release-runtime enforcement, and final permanent-suite
  enforcement after T7/T8.
- T7 and T8 are serial because they depend on the real release `serve` path.

## Checkpoint Commit Rhythm

- CP1 after T1: async runtime substrate, shared Hyper switchpoint, and cutover
  seam; release `serve` is not yet claimed complete
- CP2 after T2: SQLx async state/auth boundary and resolver contract cutover
- CP3 after T6a: early structural/dependency guardrail inventory
- CP4 after T3/T4/T5 merge: functional async transport integration through
  CLI/runtime entry; release ownership still awaits T6b
- CP5 after issue-closure integration proof rows are green
- CP6 after final structural guardrails pass on release reachability
- CP7 after installed-Codex real-serve smoke/e2e harness is green
- CP8 after soak artifact, T6c final guardrails, and final proof pack are
  complete

Each checkpoint commit must stage only scoped files for that checkpoint and must
include proof evidence in the commit message or associated evidence artifact.

## Requirements / Proof Matrix

Legend:

- Layer: U = unit, I = integration, S = smoke, E = e2e, G = structural
  guardrail, P = PR/release readiness
- Red/green: `yes` means the implementation phase should first establish a
  failing or currently-missing proof for the expected reason before making it
  pass. Structural and PR gates are fresh checks rather than behavior red/green.
- Command/status: every row's concrete command is
  `scripts/proof-matrix.sh <ROW>`. The script may dispatch to Rust tests,
  smoke scripts, structural checkers, or artifact validators, but the row is not
  green until that command exists, runs from repo root, writes the row evidence
  artifact, passes row-local redaction validation, and its status is changed
  from `[ ] pending` to `[x] passed` in the execution receipt.
- Freshness: each row command must record `git rev-parse HEAD` and fail stale if
  touched files match the row's freshness guard after the artifact timestamp.

| Row | Source | Owner | Layer | Harness / Command Surface | Expected Observation | Evidence | Freshness Guard | Red/Green |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| U-01 | R9 Proof expectations | T3/T4 | U | proxy route tests | unsupported HTTP/WS routes reject before selection/upstream | unit output | route/server changes | yes |
| U-02 | R9 Proof expectations; R6 Auth invariants | T3/T4 | U | local_auth/proxy tests | tokenless and token-required auth matrix, including smuggling carriers | unit output | local_auth/server changes | yes |
| U-03 | R9 Proof expectations; R3 Async WebSocket pure proxy | T4 | U | websocket first-frame tests | malformed, oversized, wrong type, unexpected frame fail deterministically | unit output | websocket changes | yes |
| U-04 | R9 Proof expectations | T3/T5 | U | affinity extraction tests | only allowed response id metadata is extracted | unit output | http_sse/websocket changes | yes |
| U-05 | R6 Auth invariants, R9 Proof expectations | T3/T4 | U | header sanitation tests | local auth stripped, provider auth injected once | unit output | headers/http/ws changes | yes |
| U-06 | goal Required Proof Matrix Seeds, R5 | T2 | U | selection preservation tests | selector eligibility, quota snapshot, and affinity decisions match pre-cutover semantics | unit output | selection/state changes | yes |
| U-07 | goal Required Proof Matrix Seeds, R5 | T2 | U | credential commit state-machine tests | old generation or fully committed new generation is authoritative; no half state | unit output | auth/state/secret changes | yes |
| I-01 | R9 Proof expectations | T5 | I | mock WS integration | multiple concurrent WS sessions progress independently | integration log | websocket/server changes | yes |
| I-02 | R9 Proof expectations | T5 | I | mock WS integration | stalled upstream WS does not block sibling WS completion | integration log | websocket/server changes | yes |
| I-03 | R9 Proof expectations | T3/T5 | I | mixed HTTP/WS integration | stalled WS does not block independent HTTP/SSE completion | integration log | http_sse/upstream/ws changes | yes |
| I-04 | R9 Proof expectations; Issue Closure Contract | T5 | I | interleave mock upstream | upstream waits for second local frame before completion; router forwards it; selector invoked once; first routed frame payload hash is unchanged | transcript artifact | websocket pump changes | yes |
| I-05a | R9 Proof expectations; Issue Closure Contract | T5 | I | old-failure reproducer | old/blocking or equivalent failure harness wedges, leaks, or fails for the expected reason | regression artifact | websocket/server changes | yes |
| I-05b | R9 Proof expectations; Issue Closure Contract | T5 | I | async comparison fixture | async runtime closes affected session, closes upstream, removes registry entry, and survivor completes | regression artifact | websocket/server changes | yes |
| I-06 | R9 Proof expectations | T5 | I | upstream-close fixture | upstream close while local idle closes local and terminates task | close artifact | websocket pump changes | yes |
| I-07 | R9 Proof expectations; Issue Closure Contract | T5 | I | blocked-write/backpressure fixture | peer stopped reading/closed does not leak task or stall opposite close | close artifact | websocket pump changes | yes |
| I-08 | R9 Proof expectations; R3 Async WebSocket pure proxy Async WebSocket pure proxy| T4 | I | pre-upstream failure fixture | each failure class maps to deterministic close and redacted event | audit/close artifact | websocket/server changes | yes |
| I-09 | R9 Proof expectations | T4 | I | fragmented upgrade test | fragmented upgrade succeeds through async serve path | integration output | server/ws changes | yes |
| I-10 | R9 Proof expectations | T3/T4 | I | unsupported route probe | unsupported route rejects before selection/upstream | upstream transcript | routes/server changes | yes |
| I-11 | R9 Proof expectations | T3/T4 | I | hostile auth probes | HTTP, WS pre-upgrade query/cookie/subprotocol, mismatched local-auth carriers, and WS post-upgrade first-frame carriers reject with no route classification, no account selection, and upstream count zero | upstream count + audit | auth/server changes | yes |
| I-12 | R9 Proof expectations | T3/T4 | I | upstream header canary | no local auth reaches upstream | upstream transcript | header/ws/http changes | yes |
| I-13 | R9 Proof expectations | T3/T5 | I | affinity owner tests | previous-response owner recorded for HTTP/SSE and WS | state artifact | state/affinity changes | yes |
| I-14 | R9 Proof expectations; R7 Session revocation | T5 | I | token rotation fixture | stale token-mode sessions close; fresh sessions unaffected | registry artifact | registry/auth changes | yes |
| I-15 | R9 Proof expectations | T5 | I | registry cleanup fixture | local close/upstream close/revocation/shutdown remove active session | registry high-water/zero | registry changes | yes |
| I-16 | R9 Proof expectations; R5 SQLx state boundary | T2/T6 | I | refresh cancellation fixture | failpoints before refresh response, after secret write, before state commit, after state commit, and during concurrent reads expose no half commit | state/secret artifact | auth/state changes | yes |
| I-17a | R9 Proof expectations; Permanent Regression Guardrails | T3/T6 | I | HTTP/SSE slow sink fixture | slow affinity recorder, audit sink, state sink, or secret-store boundary cannot delay HTTP/SSE body forwarding | timing artifact | http_sse/state/audit changes | yes |
| I-17b | R9 Proof expectations; Permanent Regression Guardrails | T5/T6 | I | WS slow sink fixture | slow affinity recorder, audit sink, state sink, or secret-store boundary cannot delay WS frame forwarding or close | timing artifact | websocket/state/audit changes | yes |
| I-18 | Issue Closure Contract | T5 | I/S | real `codex-router serve` fixture | close-while-pending traverses actual listener, Hyper upgrade, session registry, cancellation, cleanup | serve artifact | server/ws changes | yes |
| I-19 | Issue Closure Contract | T5 | I | pump cleanup family | local/upstream close and blocked write cleanup both directions | close artifact | websocket pump changes | yes |
| I-20 | R3 Async WebSocket pure proxy; R4 No hidden buffering | T4/T5 | I | first-frame exact-forwarding fixture | first client frame is forwarded unchanged to one upstream account/session and no payload-policy inspection occurs | upstream transcript hash | websocket/session changes | yes |
| I-21 | R5 SQLx state boundary | T1/T2 | I/S | slow quota refresh startup fixture | listener binds and first request is accepted while broad quota refresh is slow or stalled | startup timing artifact | cli/state/quota changes | yes |
| S-01 | R9 Proof expectations | T7 | S | installed-Codex real-serve smoke | tokenless default profile succeeds without `CODEX_ROUTER_TOKEN` | smoke transcript | cli/profile/harness changes | yes |
| S-02 | R9 Proof expectations | T7 | S | installed-Codex hardening smoke | missing/wrong/old/smuggled tokens reject before selection/upstream; rotation closes stale WS | smoke transcript | auth/harness changes | yes |
| S-03 | R9 Proof expectations | T7 | S | child `codex-router serve` + installed Codex | installed Codex uses spawned built router binary; artifact records binary path, PID, argv, listener, readiness, and cleanup | smoke transcript | cli/server changes | yes |
| S-04 | Issue Closure Contract | T7 | S/E | installed-Codex mock upstream | real client behavior, deterministic multi-step interaction, no fallback/reconnect/retry, schema allowlist redaction passes | redacted transcript | harness/ws changes | yes |
| E-01 | R9 Proof expectations | T8 | E | supervisor/barrier three-Codex e2e | three installed Codex children are released together, share one router PID over WS, and complete multi-step path without fallback/retry/downgrade | e2e artifact | harness/runtime changes | yes |
| E-02 | R9 Proof expectations | T8 | E | five-minute soak harness | supervisor holds three active WS sessions for at least five continuous minutes with per-runtime frame activity inside the same window | soak artifact | harness/runtime changes | yes |
| E-03 | Issue Closure Contract | T8 | E | soak artifact validator | overlap window timestamps prove concurrent activity | artifact validation | harness changes | yes |
| E-04 | Issue Closure Contract | T8 | E | soak artifact validator | each runtime has three post-handshake interactions or frame exchanges during overlap | artifact validation | harness changes | yes |
| E-05 | Issue Closure Contract | T8 | E | soak artifact validator | one runtime completes tool-call-style/multi-step interleave during overlap | artifact validation | harness changes | yes |
| E-06 | Issue Closure Contract | T8 | E | registry evidence | router records active high-water 3 and zero active after completion | registry artifact | registry/harness changes | yes |
| E-07 | Issue Closure Contract | T8 | E | continuity artifact | per-runtime client/router/upstream correlation and positive WS continuity | continuity artifact | harness/observability changes | yes |
| E-08 | Issue Closure Contract | T8 | E | socket cleanup checker | all upstream WS closed, no leaked local established/CLOSE_WAIT sockets, normal close reasons | cleanup artifact | runtime/harness changes | yes |
| E-09 | Issue Closure Contract | T8 | E | allowlist redaction validator | artifact schema allowlist passes; negative canaries catch tokens, labels, prompts, tool args, response bodies, provider payloads, raw account ids | redaction report | harness changes | yes |
| G-01 | Permanent Regression Guardrails | T6 | G | release reachability checker | no production `std::net::TcpListener` or `TcpStream` in release serve path | structural report | server/manifests changes | no |
| G-02 | Permanent Regression Guardrails | T6 | G | release reachability checker | no production `reqwest::blocking` in release serve path | structural report | manifests/upstream/auth changes | no |
| G-03 | Permanent Regression Guardrails | T6 | G | release reachability checker | no blocking tungstenite connect/accept in release serve path | structural report | manifests/ws changes | no |
| G-04 | Permanent Regression Guardrails | T6 | G | release reachability checker | no production `httparse` serving/response parsing | structural report | server/upstream changes | no |
| G-05 | Permanent Regression Guardrails | T6 | G | release reachability checker | no blocking `Box<dyn Read + Send>` response stream in async runtime | structural report | http_sse changes | no |
| G-06 | Permanent Regression Guardrails | T6 | G | release reachability checker | no direct proxy runtime `rusqlite` | structural report | state/proxy changes | no |
| G-07 | Permanent Regression Guardrails | T6 | G | positive ownership checker | local HTTP, upgrade, upstream HTTP/SSE, and WS streams enter through Hyper/tokio-tungstenite types | structural report | server/transport changes | no |
| G-08 | Permanent Regression Guardrails | T6 | G | release reachability checker | no helper/private alternate parser/handshake/frame runtime reachable from release serve | structural report | runtime changes | no |
| G-09 | Permanent Regression Guardrails | T6 | G | release graph checker | guardrail scope covers non-test production serve path and excludes only unreachable test/dev fixtures | structural report | checker changes | no |
| G-10 | Permanent Regression Guardrails | T6 | G | dependency shape check | `tokio-tungstenite` only production WS protocol dependency; blocking tungstenite test/dev-only | cargo/dependency report | manifests changes | no |
| G-11 | Permanent Regression Guardrails | T6 | G | CLI/release graph check | legacy blocking runtime removed/test-only; exactly one production serve runtime path | structural report | runtime changes | no |
| G-12 | Permanent Regression Guardrails | T5/T6 | G/I | structural + slow-sink check | pumps only emit bounded in-memory events and persistence cannot delay forwarding/close | structural/timing report | pump changes | mixed |
| G-13 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | compound close-while-pending remains in permanent suite | inventory report | tests changes | no |
| G-14 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | same-session interleave remains in permanent suite | inventory report | tests changes | no |
| G-15 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | blocked-write/backpressure cleanup remains in permanent suite | inventory report | tests changes | no |
| G-16 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | mixed WS + HTTP/SSE progress remains in permanent suite | inventory report | tests changes | no |
| G-17 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | installed-Codex concurrent smoke/e2e remains in suite | inventory report | harness changes | no |
| G-18 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | long-running three-runtime soak remains available | inventory report | harness changes | no |
| G-19 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | real serve close-while-pending regression remains in suite | inventory report | tests changes | no |
| G-20 | Permanent Regression Guardrails | T6 | G | permanent suite inventory | pump-side side-effect non-blocking regression remains in suite | inventory report | tests changes | no |
| G-21 | Permanent Regression Guardrails | T6 | G/P | repo-local/CI validation | guardrails run before done claim; cheap deterministic rows in CI | CI/local report | CI/checker changes | no |
| G-22 | R4 No hidden buffering | T5/T6 | G/I | pump buffering guard | no unbounded production pump channels and no detached production reader tasks in release serve path | structural/behavioral report | pump/runtime changes | mixed |
| G-23 | R3 local Hyper upgrade handoff | T4/T6 | G | local WS handoff guard | local Hyper-upgraded streams use `from_raw_socket`/`from_partially_read`; no local `accept_async`/`accept_hdr_async` double-handshake | structural report | server/ws changes | no |
| P-01 | Acceptance Gate | T0/T6 | P | plan-review-swarm | full-duplex WS coverage mapped | review report | plan changes | no |
| P-02 | Acceptance Gate | T0/T6 | P | plan-review-swarm | mixed HTTP/SSE + WS coverage mapped | review report | plan changes | no |
| P-03 | Acceptance Gate | T0/T6 | P | plan-review-swarm | local auth, selection, affinity semantics preserved | review report | plan changes | no |
| P-04 | Acceptance Gate | T0/T6 | P | plan-review-swarm | state/SQLx ownership clarity preserved | review report | plan changes | no |
| P-05 | Acceptance Gate | T0/T6 | P | plan-review-swarm | no retry/wrapper/session-picker scope creep | review report | plan changes | no |
| P-06 | Acceptance Gate | T0/T6 | P | plan-review-swarm | matrix catches real compound stuck-session failure | review report | plan changes | no |
| P-07 | Acceptance Gate | final | P | implementation review | full Issue Closure Contract rows pass before completion | review report | proof changes | no |
| P-08 | Acceptance Gate | final | P | implementation review | Permanent Regression Guardrail rows pass before completion | review report | proof changes | no |
| P-09 | Acceptance Gate | T0 | P | plan-review-swarm | matrix preserves one row per hard gate | review report | plan changes | no |
| P-10 | goal terminal | final | P | implementation-pr-wrapup | PR created/updated, checks and review-thread state fresh, not merged | PR report | final diff/CI changes | no |

### Matrix Status Ledger

Initial status for every row above is `[ ] pending`.

Implementation may change a row to `[x] passed` only in a durable execution
receipt after:

1. `scripts/proof-matrix.sh <ROW>` exists and runs from repo root.
2. The command writes the row artifact under the evidence directory.
3. The artifact records row id, command, git HEAD, timestamp, touched target,
   pass/fail, expected observation, redaction result, and freshness guard.
4. Row-local redaction validation passes.
5. The parent final gate re-runs aggregate redaction and stale-artifact checks.

Rows with suffixes such as `I-05a`, `I-05b`, `I-17a`, and `I-17b` are separate
hard gates. Passing the sibling row does not imply this row is passed.

The five-minute soak is allowed to be an ignored/manual PR-ready gate, but it is
not optional. The authoritative command is `scripts/proof-matrix.sh E-02`; its
artifact must be fresh against the final PR HEAD.

## Validation Gates

Gate 0: plan review

- `shravan-dev-workflow:plan-review-swarm`
- required before implementation

Gate 1: local fast checks after each implementation slice

- `cargo fmt --all -- --check`
- `scripts/proof-matrix.sh <ROW>` for every row owned by the slice
- targeted `cargo test` / `cargo nextest run` may be called by the row script,
  but cannot replace the row command receipt
- targeted structural guard command when introduced through the row script

Gate 2: relevant workspace checks after integration checkpoints

- `cargo check --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace`
- `cargo deny check`
- `cargo audit`

Gate 3: runtime smoke/e2e

- real `codex-router serve` boot smoke
- installed-Codex real-serve HTTP/SSE and WebSocket smoke
- three-runtime e2e
- these gates must spawn the built `codex-router serve` child process for S/E
  acceptance rows; in-process runtime helpers are allowed only for lower-layer
  integration rows that are explicitly not S/E acceptance rows

Gate 4: final soak and PR readiness

- five-minute three-runtime soak artifact
- structural guardrail report
- row-local redaction scans and final aggregate evidence redaction scan
- stale-artifact scan against final PR HEAD
- fresh CI/PR state

## Evidence Directory

Implementation should write durable proof receipts under:

```text
tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/
  unit/
  integration/
  smoke/
  e2e/
  structural/
  pr-gate/
```

Every evidence artifact must include:

- row id
- git HEAD
- command or harness name
- UTC timestamp
- touched binary/test target
- pass/fail
- expected observation summary
- redaction check result
- row status before and after the command
- stale-artifact check result

Forbidden evidence material:

- raw prompts
- tool arguments
- response bodies
- local tokens
- refresh tokens
- provider payloads
- account labels
- raw provider account ids

Redaction is allowlist-based. Evidence writers must emit only documented safe
fields; validators also include negative canaries for local tokens, refresh
tokens, prompts, tool arguments, response bodies, provider payloads, account
labels, and raw provider account ids. A single final E-09 pass is not enough if
earlier row artifacts leak data; every artifact-producing row runs row-local
redaction validation.

## Risks And Recovery

- Biggest merge pressure is `crates/codex-router-proxy/src/server.rs`; keep
  runtime ownership changes serial.
- WebSocket transport and session registry should be planned together for proof,
  even if implementation splits internally.
- Guardrails must run against release `serve` reachability, not broad repo greps
  or obvious files only.
- If installed-Codex soak is too slow for every CI run, keep deterministic smoke
  and structural checks in CI and require a fresh ignored/manual soak artifact
  before PR-ready status.
- If any slice reveals the spec is wrong, stop implementation and return to
  spec creation/review with evidence.

## Recommended Next Workflow

`shravan-dev-workflow:plan-review-swarm`

phase_result: complete
evidence:
- `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/plan-ledger.md`
- `tmp/plan-workflows/2026-06-24-async-router-runtime/lanes/`
recommended_next_workflow: `shravan-dev-workflow:plan-review-swarm`
recommended_transition_reason: The accepted spec has been converted into a
source-traced implementation plan with a mandatory proof/guardrail matrix; the
next lifecycle gate is adversarial plan review before code changes.
