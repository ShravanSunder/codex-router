# Codex Router Goal Details

Goal id: 2026-06-20-codex-router

## Objective

Build `codex-router` as a greenfield Rust repo for a narrow local proxy in front
of the real OpenAI Codex CLI. The router must keep Codex behavior owned by
Codex, route requests across multiple OpenAI OAuth accounts, maintain quota and
account state in a local store, and avoid Prodex's multi-provider gateway scope.

## Key Artifacts

- Spec: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- Research evidence: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/specs/references/2026-06-20-research-evidence.md`
- Spec review: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/specs/reviews/2026-06-20-codex-router-spec-review.md`
- README: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/README.md`
- Live Codex source: `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex` at `d66708232299`
- Live Prodex source: `/Users/shravansunder/Documents/dev/open-source/ai-dev/prodex` at `682e442a11b0`
- Archived old fork: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router-prodex-fork-archive-2026-06-20`

## Current Decision

The reviewed spec is the source of truth. The official next workflow is
`shravan-dev-workflow:plan-create`.

The plan must start by reading the full spec and review report. It must then
produce a detailed implementation plan before any code implementation begins.

## Scope

- Local proxy for Codex custom-provider traffic.
- Multiple OpenAI OAuth accounts managed by the router.
- Account selection, rotation, quota accounting, background refresh, and local
  persistence.
- HTTP, SSE, compact, models, memory-trace, and WebSocket forwarding surfaces
  that Codex itself uses for custom providers.
- Codex profile generation or dry-run application, guarded by explicit approval
  for any `~/.codex` write.
- Rust repo setup with strict linting, formatting, dependency policy, test
  pyramid, and maintainable module boundaries.

## Non-Goals

- Bundling, forking, wrapping, or modifying the Codex CLI.
- Acting as a general multi-provider gateway.
- Prodex gateway/admin/virtual-key/billing/metrics/guardrail/SCIM/SSO/tenant
  surfaces.
- Realtime or WebRTC routing in v1.
- Provider gating, model gating, request timeout policy, context compression
  policy, or retry decisions that belong to Codex.
- Touching global auth or home config without explicit approval.

## Plan-Create Requirements

The implementation plan must be specific enough to execute without redesign.
It must include:

- Rust workspace/repo setup:
  - `rust-toolchain.toml`
  - `Cargo.toml` workspace layout
  - `rustfmt.toml`
  - Clippy policy with strict warnings
  - dependency audit/license policy
  - CI commands or local equivalents
  - test pyramid commands by layer
- Module boundaries:
  - protocol forwarding
  - local auth
  - OAuth account store
  - quota state and background refresh
  - routing state machine
  - Codex profile helper
  - audit logging
  - CLI/server entrypoints
- Source-mining tasks:
  - verify current Codex provider config and proxy protocol from live source
  - verify latest OpenAI Codex docs/manual
  - extract only narrow Prodex behavior worth reusing
  - reject Prodex provider-core/gateway surfaces explicitly
- Proof gates:
  - unit
  - integration
  - protocol
  - smoke with installed Codex and isolated config
  - gated live OAuth/quota checks only with explicit approval
- A requirements/proof matrix with rows for every spec requirement and stop
  condition.

## Requirements/Proof Matrix Seed

Requirement / claim:
Codex owns protocol behavior; router forwards only supported Codex custom-provider
surfaces and fails closed on unsupported surfaces.
Proof source:
plan-create must map supported routes to Codex source/docs evidence and define
protocol tests for HTTP, SSE, compact, models, memory-trace, and WebSocket.
proof owner: plan-create, then implementation-execute-plan
stale-proof guard: Codex source commit and manual retrieval timestamp recorded

Requirement / claim:
Router performs account selection only after local auth and before upstream
connection, including WebSocket first `response.create` frame routing.
Proof source:
plan-create must define routing state-machine tests and WebSocket mock transcript
tests.
proof owner: plan-create, then implementation-execute-plan
stale-proof guard: mock transcript fixture tied to current Codex protocol evidence

Requirement / claim:
Quota state is precomputed/refreshed in the background and used for fast routing.
Proof source:
plan-create must define unit tests for account scoring plus integration tests for
store refresh and degraded/stale quota behavior.
proof owner: plan-create, then implementation-execute-plan
stale-proof guard: store schema version and deterministic fixture clock

Requirement / claim:
Local bearer auth and audit logging avoid secret leakage and fail closed before
account selection.
Proof source:
plan-create must define tests for missing env header, token rotation, WebSocket
revocation, and positive allowlist audit serialization.
proof owner: plan-create, then implementation-execute-plan
stale-proof guard: audit schema snapshot and token-redaction assertions

Requirement / claim:
The router never mutates `~/.codex` silently.
Proof source:
plan-create must define profile dry-run tests and explicit approval tests for any
home config write path.
proof owner: plan-create, then implementation-execute-plan
stale-proof guard: temp `CODEX_HOME` or isolated fixture proof

Requirement / claim:
The Rust repo follows best-practice maintainability standards from the start.
Proof source:
plan-create must define exact setup files, lint/format/type/test commands, CI or
local equivalent, and the first crate/module structure before implementation.
proof owner: plan-create
stale-proof guard: current Rust stable/toolchain version and command output

Requirement / claim:
Live OAuth/quota behavior is validated only with explicit approval.
Proof source:
plan-create must separate mock/integration proof from gated live proof and name
the approval boundary.
proof owner: plan-create, then parent orchestrator
stale-proof guard: live approval transcript and redacted evidence only

## Stop And Block Rules

Stop condition:
The full default implementation lifecycle is complete: accepted plan, reviewed
plan, implementation proof, implementation review disposition, PR opened or
updated and proven ready, fresh PR checks/review-thread/mergeability state
reported, and no merge performed unless explicitly authorized.

Blocked condition:
Stop and report the exact blocker if the Codex provider contract cannot be
verified from current source/docs, if required local auth/home-write safety cannot
be made testable, if live OAuth/quota proof is needed but not approved, or if a
workflow state pointer is missing or contradictory.

Checkpoint rhythm:
Record orchestrator transitions in `events.jsonl`. Commit verified checkpoints
when scoped files changed and repo policy permits. Do not treat a commit as
proof.

## Phase Recommendation Footer From Spec Review

phase_result: complete
evidence: spec, research evidence, review report, `git diff --check`, `wc -l`,
and targeted `rg` validation from 2026-06-20
recommended_next_workflow: shravan-dev-workflow:plan-create
recommended_transition_reason: The spec is reviewed and revised; the first
unproven lifecycle gate is a detailed implementation plan with proof matrix.

## Plan-Create Result

Plan artifact:
`/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/plans/2026-06-20-codex-router-implementation-plan.md`

Current workflow:
`shravan-dev-workflow:plan-create` complete

Official next workflow:
`shravan-dev-workflow:plan-review-swarm`

Plan-create evidence:

- full source coverage recorded in the plan
- current Codex and Prodex source commits recorded in the plan
- local Rust toolchain state recorded in the plan
- initial 24-row requirements/proof matrix written by plan-create, later
  expanded to 29 rows by plan-review
- task sequence T1-T12 written
- validation gates by unit, integration, protocol, smoke, quality, gated live,
  and PR readiness layer written

Phase recommendation footer from plan-create:

phase_result: complete
evidence: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/plans/2026-06-20-codex-router-implementation-plan.md`, `git diff --check`, `jq -c . tmp/workflow-state/2026-06-20-codex-router/events.jsonl`, and plan artifact line coverage from 2026-06-20
recommended_next_workflow: shravan-dev-workflow:plan-review-swarm
recommended_transition_reason: The implementation plan now exists and must be
adversarially reviewed before code scaffolding or implementation begins.

## Plan-Review Result

Plan review artifact:
`/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/plans/reviews/2026-06-20-codex-router-plan-review.md`

Current workflow:
`shravan-dev-workflow:plan-review-swarm` complete

Official next workflow:
`shravan-dev-workflow:implementation-execute-plan`

Plan-review evidence:

- plan line coverage after revisions: 861 lines
- spec line coverage after revisions: 450 lines
- requirements/proof matrix has 29 rows and no duplicate IDs
- invalid installed-Codex smoke command shape replaced with
  `codex --profile codex-router exec ...`
- direct fake `codex-router live-proof ...` command removed from the executable
  plan path; live OAuth/quota remains runbook-only unless a CLI surface is
  explicitly added
- host toolchain/bootstrap mutation is an explicit approval checkpoint
- SQLite metadata store, secret-store-before-auth ordering, WebSocket proof
  gates, corruption handling, loopback binding, and PR remote preconditions were
  added or tightened
- exact profile content, shell-safe token export, helper-produced smoke
  activation, `actionlint` bootstrap, and read-only external provenance checks
  were added after the spec-compliance lane returned

Phase recommendation footer from plan-review:

phase_result: complete
evidence: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/plans/reviews/2026-06-20-codex-router-plan-review.md`, revised implementation plan, revised spec smoke wording, stale-command search, proof-matrix ID check, `git diff --check`, and `jq -c . tmp/workflow-state/2026-06-20-codex-router/events.jsonl`
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Plan-review findings have been applied; the next
unproven lifecycle gate is implementation execution starting with T0 source
provenance and host-bootstrap approval detection.

## Implementation-Execute T0 Checkpoint

Execution brief:
`/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/plan-workflows/2026-06-20-codex-router-main-implementation-execute/implementation-execute-plan-brief.md`

Provenance note:
`/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/wip/implementation-provenance.md`

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T0 evidence:

- full reviewed plan loaded: 861 lines, chunks 1-220, 221-440, 441-660,
  661-861
- Codex source verified read-only at
  `d66708232299bdbf373ec55b0d6b938c246cfa60`
- Prodex source verified read-only at
  `682e442a11b0c3e7c2d0264694d77ff259c15312`
- Codex manual helper reports local manual cache current
- installed Codex is `codex-cli 0.141.0`
- pinned Rust `1.95.0` compiler and cargo are available through `rustup run`
- `cargo-nextest`, `cargo-deny`, `cargo-audit`, and `actionlint` are missing

T0 decision:

Do not start T1 or scaffold product code until explicit host-bootstrap approval
is granted for installing the missing tools, or the plan is updated to a
different proof strategy.

## Implementation-Execute T1 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T1 result:

- host-bootstrap completed after user approval
- Rust workspace scaffolded with 9 crates
- CI workflow, rust toolchain, rustfmt, cargo-deny, cargo-audit, and gitignore
  baseline created
- stable `rustfmt` reality check removed unsupported `imports_granularity`
  from the plan and `rustfmt.toml`
- smoke test directory placeholder added so guard commands cover the intended
  path set

T1 proof:

- `cargo fmt --all -- --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo nextest run --workspace`: 9 tests passed, 0 skipped
- `cargo deny check`: pass
- `cargo audit`: pass
- `actionlint .github/workflows/ci.yml`: pass
- forbidden-scope and dependency guard checks: pass

Next implementation slice:
T2 core config, ids, redaction, audit schema, and error primitives.

## Implementation-Execute T2 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T2 result:

- core config model added with deny-unknown TOML parsing
- loopback-only listener validation added
- router-local auth env and audit sink config primitives added
- core id newtypes added
- `SecretString` redacts display/debug/serialization
- redacted allowlist audit event schema added
- typed core errors added

T2 proof:

- RED: `cargo nextest run -p codex-router-core` failed because T2 modules were
  missing
- GREEN: `cargo nextest run -p codex-router-core`: 6 tests passed
- full sweep after T2:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 14 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Plan/proof adjustment:

- `toml` was updated to `1.1.2` to remove a duplicate `winnow` warning and keep
  the baseline warning-clean.

Next implementation slice:
T3 hardened file secret store and refresh lease.

## Implementation-Execute T3 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T3 result:

- hardened file secret store implemented behind a `SecretStore` trait
- secret keys validate file-safe names and reject traversal
- file backend rejects `.codex` roots and symlink paths
- file backend creates private root directories and private secret files
- file backend writes through temp file, sync, and atomic rename
- deterministic refresh lease manager implemented with owner/follower behavior,
  stale recovery, and owner-matched release

T3 proof:

- RED: `cargo nextest run -p codex-router-secret-store` failed because T3
  modules were missing
- GREEN: `cargo nextest run -p codex-router-secret-store`: 7 tests passed
- full sweep after T3:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 20 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Next implementation slice:
T4 local router auth and shell-safe token export.

## Implementation-Execute T4 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T4 result:

- local router token model implemented in core
- current token validation rejects missing, empty, wrong, and old tokens
- token debug output redacts token material and exposes generation metadata
- proxy auth gate validates before selection/upstream routing
- CLI token service rotates through the real T3 `SecretStore` trait
- token generation metadata persists with router-owned token state
- POSIX export helper emits exactly one `CODEX_ROUTER_TOKEN='...'` assignment
  with no prose

T4 proof:

- RED: `cargo nextest run -p codex-router-core -p codex-router-proxy -p codex-router-cli`
  failed because local auth modules and `TokenGeneration` constructors were
  missing
- GREEN: `cargo nextest run -p codex-router-core -p codex-router-proxy -p codex-router-cli`:
  13 tests passed, 0 skipped
- full sweep after T4:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 25 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- old-token WebSocket forced close is deferred until the proxy owns concrete
  WebSocket connection state; T4 proves the auth/generation primitive and the
  pre-selection gate.

Next implementation slice:
T5 OAuth account store, SQLite metadata, and quota snapshot model.

## Implementation-Execute T5 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T5 result:

- auth now owns OpenAI OAuth-specific token expiry, refresh-needed, and refresh
  response classification
- quota now owns deterministic snapshot freshness and route-band headroom logic
- state now owns SQLite v1 migrations, schema version checks, account metadata,
  quota snapshot persistence, and corrupt account isolation
- `rusqlite` is state-only, with defaults disabled and only bundled SQLite
  enabled to avoid the default wasm/VFS dependency branch

T5 proof:

- RED: `cargo nextest run -p codex-router-state -p codex-router-auth -p codex-router-quota`
  failed because the T5 modules and `rusqlite` dependency were missing
- GREEN: `cargo nextest run -p codex-router-state -p codex-router-auth -p codex-router-quota`:
  10 tests passed, 0 skipped
- full sweep after T5:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 32 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- T5 does not yet implement live OAuth login, mock HTTP OAuth/quota services, or
  background refresh workers. Those remain later slices.

Next implementation slice:
T6 selection and routing state machine.

## Implementation-Execute T6 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T6 result:

- selection now classifies eligibility with stale/unknown quota penalties only
  when known-fresh accounts exist
- weighted selection balances eligible accounts by smooth weighted deficit
- reservations reduce immediate account headroom until released
- affinity pins override balance only while pinned account is eligible
- signed turn-state envelopes carry account pin and optional upstream token,
  redact debug output, and reject tampering
- precommit rotation remains narrow: auth rejection and quota exhaustion rotate;
  timeout and malformed-response failures return to Codex

T6 proof:

- RED: `cargo nextest run -p codex-router-selection` failed because T6 modules,
  quota dependency, and id helpers were missing
- GREEN: `cargo nextest run -p codex-router-selection`: 7 tests passed, 0 skipped
- full sweep after T6:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 38 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- T6 does not yet wire selection into proxy transports or persistent
  reservation/affinity repositories.

Next implementation slice:
T7 background refresh runtime.

## Implementation-Execute T7 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T7 result:

- quota startup runtime loads existing snapshots immediately and schedules
  refresh work without inline provider fetch
- auth refresh worker plans background refreshes from deterministic token expiry
  classification without reading secret material
- CLI doctor report renders stale/missing quota state and keeps secret canaries
  out of output

T7 proof:

- RED: `cargo nextest run -p codex-router-quota -p codex-router-auth -p codex-router-cli`
  failed because T7 modules were missing
- GREEN: `cargo nextest run -p codex-router-quota -p codex-router-auth -p codex-router-cli`:
  12 tests passed, 0 skipped
- full sweep after T7:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 41 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- T7 does not yet implement a real threaded/async worker loop or live
  OAuth/quota endpoint calls.

Next implementation slice:
T7.5 contract freeze before proxy integration.

## Implementation-Execute T7.5 Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T7.5 result:

- auth exposes the `AuthenticatedQuotaClient` quota-refresh facade
- state exposes account, quota snapshot, and affinity repository contracts
- SQLite implements those repository contracts and migrates `affinity_pins`
- selection exposes `SelectionDecision`, `ReservationHandle`, and named
  `PrecommitFailureClassifier`

T7.5 proof:

- RED: `cargo nextest run -p codex-router-auth -p codex-router-state -p codex-router-selection`
  failed because T7.5 contract modules and DTOs were missing
- GREEN: `cargo nextest run -p codex-router-auth -p codex-router-state -p codex-router-selection`:
  19 tests passed, 0 skipped
- full sweep after T7.5:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 45 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- T7.5 does not yet implement proxy HTTP/SSE/WebSocket transport.

Next implementation slice:
T8 HTTP/SSE and WebSocket proxy.

## Implementation-Execute T8 Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 partial result:

- route classifier supports the required Codex HTTP routes and WebSocket upgrade
  classification on `/v1/responses`
- unsupported paths, including Realtime/WebRTC style paths, fail closed before
  selection
- upstream request builder preserves body bytes and unknown Codex fields
- upstream header sanitizer strips local router token, hop-by-hop headers,
  client-supplied upstream authorization, and cookie auth
- selected upstream auth is injected exactly once
- test-support now has mock upstream transcript primitives

T8 partial proof:

- RED: `cargo nextest run -p codex-router-proxy -p codex-router-test-support`
  failed because T8 protocol modules were missing
- GREEN: `cargo nextest run -p codex-router-proxy -p codex-router-test-support`:
  6 tests passed, 0 skipped
- full sweep after T8 partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 48 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This T8 partial slice does not yet implement the loopback server, SSE,
  WebSocket first-frame routing, WebSocket handshake header tests, or hostile
  first-frame close behavior.

Next implementation slice:
Continue T8 HTTP/SSE and WebSocket proxy runtime.

## Implementation-Execute T8 HTTP/SSE Handler Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 HTTP/SSE handler partial result:

- added in-process HTTP/SSE proxy handler DTOs and service boundary
- unsupported paths fail closed before upstream work
- supported routes forward through an injected upstream transport
- selected upstream auth and header sanitization from the previous T8 protocol
  slice are used by the handler
- Responses/SSE request body bytes are preserved without interpreting unknown
  Codex fields
- upstream response status, headers such as `ETag`, and body are preserved

T8 HTTP/SSE handler partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because `http_sse`
  module was missing
- GREEN: `cargo nextest run -p codex-router-proxy`: 7 tests passed, 0 skipped
- full sweep after T8 HTTP/SSE handler partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 51 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice does not yet bind a loopback network server or implement WebSocket
  first-frame routing/handshake hostile-case behavior.

Next implementation slice:
Continue T8 WebSocket first-frame and handshake protocol.

## Implementation-Execute T8 WebSocket Protocol Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 WebSocket protocol partial result:

- added WebSocket first-frame protocol DTOs and router
- first local frame must be text, under configured byte limit, valid JSON, and
  `type = "response.create"`
- malformed, non-text, oversized, and non-`response.create` first frames produce
  explicit local close reasons before upstream is opened
- upstream WebSocket handshake headers reuse the sanitizer that strips local
  router token, hop-by-hop headers, client upstream auth, and cookies
- selected upstream auth is injected exactly once
- accepted first frame is forwarded unchanged

T8 WebSocket protocol partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because `websocket`
  module was missing
- GREEN: `cargo nextest run -p codex-router-proxy`: 9 tests passed, 0 skipped
- full sweep after T8 WebSocket protocol partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 53 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice does not yet bind a loopback network server or open real upstream
  WebSocket connections.

Next implementation slice:
Continue T8 loopback server runtime.

## Implementation-Execute T8 Loopback Server Bind Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 loopback server bind partial result:

- added loopback-only server bind primitives in `codex-router-proxy`
- validated `127.0.0.1`, `localhost`, and `::1` as accepted loopback inputs
- rejected `0.0.0.0`, `::`, and LAN address inputs before TCP bind
- bound an ephemeral loopback TCP listener and exposed the actual local address
- kept this slice below protocol policy: no retry, timeout, account, or
  upstream routing behavior was added

T8 loopback server bind partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because `server` module
  was missing
- RED: `cargo nextest run -p codex-router-proxy` failed because `localhost` was
  not accepted as a loopback alias
- GREEN: `cargo nextest run -p codex-router-proxy`: 12 tests passed, 0 skipped
- full sweep after T8 loopback server bind partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 56 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice does not yet implement a full async HTTP network adapter or real
  upstream WebSocket connections.

Next implementation slice:
Continue T8 real HTTP/WebSocket network adapter.

## Implementation-Execute T8 Loopback HTTP Adapter Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 loopback HTTP adapter partial result:

- fixed HTTP/SSE service route classification to use the path component while
  preserving the original path and query string upstream
- added `httparse` as a small HTTP/1.x parser dependency with compatible
  `MIT OR Apache-2.0` license
- added one-connection loopback HTTP adapter over `TcpStream`
- converted parsed HTTP method, path, headers, and body into the existing
  `HttpProxyRequest` DTO
- serialized `HttpProxyResponse` back to a real TCP client
- proved a real loopback TCP `POST /v1/responses?stream=true` request reaches
  the injected upstream transport with query, body, selected upstream auth, and
  sanitized headers preserved

T8 loopback HTTP adapter partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because a supported
  `/v1/responses?...` route was rejected as `unsupported_path`
- GREEN: `cargo nextest run -p codex-router-proxy`: 13 tests passed, 0 skipped
- RED: `cargo nextest run -p codex-router-proxy` failed because
  `LoopbackHttpAdapter` was missing
- GREEN: `cargo nextest run -p codex-router-proxy`: 14 tests passed, 0 skipped
- full sweep after T8 loopback HTTP adapter partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 58 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice does not yet implement the long-running server accept loop,
  network-bound local auth composition, account selection wiring, or real
  upstream WebSocket connections.

Next implementation slice:
Continue T8 network-bound local auth and selection composition.

## Implementation-Execute T8 Network-Bound Auth/Selection Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 network-bound auth/selection partial result:

- added a proxy request-handler boundary so server parsing is separate from
  local auth, account selection, and forwarding
- added authenticated HTTP proxy composition:
  local auth gate -> account selector -> existing HTTP/SSE proxy service
- missing local router token now returns `HttpProxyError::LocalAuth` before any
  selector or upstream work
- successful local auth passes token generation to the selector and forwards
  with the selected upstream token
- loopback TCP adapter now uses the authenticated request handler rather than a
  preselected upstream token shortcut

T8 network-bound auth/selection partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because
  `AuthenticatedHttpProxyService`, `HttpRequestHandler`,
  `SelectedUpstreamAccount`, and `UpstreamAccountSelector` were missing
- GREEN: `cargo nextest run -p codex-router-proxy`: 16 tests passed, 0 skipped
- full sweep after T8 network-bound auth/selection partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 60 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice still uses a mock selector and does not yet implement a
  long-running accept loop or real upstream WebSocket connections.

Next implementation slice:
Continue T8 selector-decision integration or long-running accept loop.

## Implementation-Execute T8 Quota-Aware Selector Adapter Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 quota-aware selector adapter partial result:

- added a concrete proxy selector adapter backed by existing
  `codex-router-selection` and `codex-router-quota` primitives
- selected account material now includes account id, upstream token, and
  selection reason
- selector adapter converts quota freshness/headroom into weighted selector
  candidates
- selector adapter fails closed when no account has usable headroom
- proxy crate now depends on `codex-router-selection` and `codex-router-quota`
  instead of using only mock selector traits

T8 quota-aware selector adapter partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because
  `QuotaAwareAccountSelector`, `QuotaAwareAccountState`,
  `QuotaAwareAccountSelectorError`, `codex-router-quota` dependency, and
  `HttpProxyError::Selection` were missing
- GREEN: `cargo nextest run -p codex-router-proxy`: 18 tests passed, 0 skipped
- full sweep after T8 quota-aware selector adapter partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 62 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice still uses in-memory selector input instead of repository-loaded
  account/quota state and does not yet implement a long-running accept loop or
  real upstream WebSocket connections.

Next implementation slice:
Continue T8 repository-backed selector input or long-running accept loop.

## Implementation-Execute T8 Bounded Loopback HTTP Accept Loop Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 bounded loopback HTTP accept loop partial result:

- added a concrete bounded loopback HTTP server accept loop
- reused the existing one-connection HTTP adapter for each accepted TCP stream
- returned the handled connection count after the caller-provided bound
- kept timeout, retry, health, and provider gating behavior out of the proxy
- added a real TCP test that sends two local authenticated requests and proves
  both reach the upstream recording boundary with query strings preserved

T8 bounded loopback HTTP accept loop partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because
  `LoopbackHttpServer` was missing from `crate::server`
- GREEN: `cargo nextest run -p codex-router-proxy`: 19 tests passed, 0 skipped
- full sweep after T8 bounded loopback HTTP accept loop partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 63 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice proves bounded local serving only. Repository-loaded selector input
  and real upstream WebSocket connections remain open.

Next implementation slice:
Continue T8 repository-backed selector input or real upstream connection wiring.

## Implementation-Execute T8 Repository Hydration Foundation Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 repository hydration foundation partial result:

- added deterministic account listing to the state repository contract
- implemented SQLite account listing with `ORDER BY account_id`
- kept disabled accounts visible to callers so eligibility stays explicit in
  routing/selection code instead of being silently hidden by storage
- added a secret-store key convention for upstream OpenAI access tokens:
  `openai_access_token.<account_id>`
- kept upstream token material out of SQLite; state owns account metadata,
  secret-store owns token material

T8 repository hydration foundation partial proof:

- RED: `cargo nextest run -p codex-router-state` failed because
  `AccountStateRepository::list_accounts` did not exist
- GREEN: `cargo nextest run -p codex-router-state`: 6 tests passed, 0 skipped
- RED: `cargo nextest run -p codex-router-secret-store` failed because
  `crate::account_tokens::upstream_access_token_key` did not exist
- GREEN: `cargo nextest run -p codex-router-secret-store`: 8 tests passed,
  0 skipped
- full sweep after T8 repository hydration foundation partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 65 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This slice is foundation only. The repository-backed proxy selector that
  combines account metadata, quota snapshots, and secret-store tokens remains
  open.

Next implementation slice:
Build repository-backed selector hydration, then continue real upstream
connection wiring.

## Implementation-Execute T8 Repository-Backed Selector Hydration Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 repository-backed selector hydration partial result:

- added `RepositoryBackedAccountSelector` to the proxy crate
- proxy now depends on state and secret-store crates for selector input
  hydration
- selector loads deterministic account metadata from SQLite, skips disabled
  accounts, loads per-account quota snapshots, and reads upstream access tokens
  from the file secret store
- missing token files make that account unauthenticated/ineligible instead of
  treating the account as usable capacity
- state/secret-store failures map to redacted fail-closed selector errors
- in-memory and repository-backed selectors share the same weighted-deficit
  selection implementation

T8 repository-backed selector hydration partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because proxy lacked
  `codex-router-state`, `codex-router-secret-store`, and
  `RepositoryBackedAccountSelector`
- GREEN: `cargo nextest run -p codex-router-proxy`: 20 tests passed, 0 skipped
- full sweep after T8 repository-backed selector hydration partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 66 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- Repository-backed HTTP selection input is now wired. Real upstream WebSocket
  connections, full CLI runtime assembly, and installed-Codex smoke remain open.

Next implementation slice:
Continue real upstream connection wiring or assemble the CLI runtime path.

## Implementation-Execute T8 Upstream Endpoint URL Assembly Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 upstream endpoint URL assembly partial result:

- added `UpstreamEndpoint` and `UpstreamEndpointError`
- endpoint validation rejects empty and non-HTTP(S) provider base URLs
- endpoint URL assembly preserves query strings
- endpoint URL assembly avoids duplicating `/v1` when Codex request paths
  already include `/v1`
- no HTTP client, retry, timeout, health, or provider-gating behavior was added

T8 upstream endpoint URL assembly partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because
  `UpstreamEndpoint` was missing from `crate::upstream`
- GREEN: `cargo nextest run -p codex-router-proxy`: 21 tests passed, 0 skipped
- full sweep after T8 upstream endpoint URL assembly partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 67 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- Real upstream HTTP/WebSocket connections remain open. This slice only defines
  the URL assembly primitive that those transports will use.

Next implementation slice:
Continue real upstream HTTP transport or WebSocket connection wiring.

## Implementation-Execute T8 Local HTTP Upstream Transport Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 local HTTP upstream transport partial result:

- added `HttpUpstreamTransport`
- implemented blocking HTTP/1.1 upstream request/response over `TcpStream`
- reused `UpstreamEndpoint` for upstream URL/path assembly
- preserved query strings, selected upstream auth, upstream response status,
  headers, and body in a real local socket test
- kept local router token and client-supplied hostile upstream auth stripped
- added no new dependency, retry, timeout, health, or provider-gating behavior

T8 local HTTP upstream transport partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because
  `HttpUpstreamTransport` was missing from `crate::upstream`
- GREEN: `cargo nextest run -p codex-router-proxy`: 22 tests passed, 0 skipped
- full sweep after T8 local HTTP upstream transport partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 68 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This proves local/mock HTTP upstream transport only. HTTPS live OpenAI
  transport, real OAuth/quota proof, WebSocket upstream connections, CLI runtime
  assembly, and installed-Codex smoke remain open.

Next implementation slice:
Continue WebSocket upstream connection wiring or assemble CLI runtime for mock
HTTP smoke.

## Implementation-Execute T8 Authenticated WebSocket Selection Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 authenticated WebSocket selection partial result:

- added `AuthenticatedWebSocketRouter`
- WebSocket handshake now exposes normalized header lookup for local auth
- missing local token rejects before account selection
- valid local token uses the selector with WebSocket `/v1/responses` route
  context
- selected upstream token is passed into the existing first-frame router, which
  preserves first-frame bytes and sanitizes handshake headers
- real upstream WebSocket networking/tunneling is still not implemented

T8 authenticated WebSocket selection partial proof:

- RED: `cargo nextest run -p codex-router-proxy` failed because
  `AuthenticatedWebSocketRouter` and `WebSocketCloseReason::LocalAuth` were
  missing
- GREEN: `cargo nextest run -p codex-router-proxy`: 24 tests passed, 0 skipped
- full sweep after T8 authenticated WebSocket selection partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 70 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This proves local WebSocket auth and selected upstream-open data only. Real
  upstream WebSocket connections, CLI runtime assembly, and installed-Codex
  smoke remain open.

Next implementation slice:
Continue WebSocket upstream tunnel proof or assemble CLI runtime for mock HTTP
smoke.

## Implementation-Execute T9 Codex Profile Helper Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T9 Codex profile helper partial result:

- added `CodexRouterProfile` renderer
- rendered profile declares the required custom provider fields:
  `model_provider`, `base_url`, `wire_api = "responses"`,
  `requires_openai_auth = false`, `supports_websockets = true`, and
  `env_http_headers`
- added `CodexRouterProfileWriter`
- dry-run returns target path/content without touching the filesystem
- write without explicit approval returns `ProfileWriteError::ApprovalRequired`
- approved write is scoped to a caller-provided temp Codex home and writes only
  `config.toml`

T9 Codex profile helper partial proof:

- RED: `cargo nextest run -p codex-router-cli` failed because
  `profile`/`CodexRouterProfile` were missing
- GREEN: `cargo nextest run -p codex-router-cli`: 5 tests passed, 0 skipped
- RED: `cargo nextest run -p codex-router-cli` failed because
  `CodexRouterProfileWriter` and `ProfileWriteError` were missing
- GREEN: `cargo nextest run -p codex-router-cli`: 6 tests passed, 0 skipped
- full sweep after T9 profile helper partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 72 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This is the profile helper library layer. Full CLI argument parsing,
  installed-Codex smoke, real upstream WebSocket connections, and PR readiness
  remain open.

Next implementation slice:
Continue CLI command wiring or installed-Codex mock smoke scaffolding after
runtime assembly.

## Implementation-Execute T9 Codex Profile Command Wiring Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T9 Codex profile command wiring partial result:

- added a process-independent CLI execution surface with injectable args, env,
  stdout, and stderr
- added `CliContext` for env-derived checks without printing token values
- wired `profile print` to render the Codex custom-provider profile
- wired `profile doctor` to report `CODEX_ROUTER_TOKEN` present/missing without
  revealing the token value
- wired `profile write --dry-run` to print the exact temp `config.toml` target
  and rendered content without writing
- wired `profile write --approve-codex-home-write` to write only the caller
  supplied temp Codex home
- preserved real `~/.codex` write avoidance in tests

T9 Codex profile command wiring partial proof:

- RED: `cargo nextest run -p codex-router-cli profile_` failed because
  `CliContext` and `run_with_io` were missing
- GREEN: `cargo nextest run -p codex-router-cli profile_`: 7 profile tests
  passed, 4 skipped by filter
- full sweep after T9 profile command wiring partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 77 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This completes the current profile command layer only. Installed-Codex smoke,
  server runtime assembly, real upstream WebSocket tunneling, live OAuth/quota
  proof, implementation review, and PR readiness remain open.

Next implementation slice:
Continue server/runtime assembly for installed-Codex mock smoke, or complete
real upstream WebSocket tunnel proof before T10.

## Implementation-Execute Audit Sink And WebSocket-Core Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

Audit sink slice result:

- `AuditEvent` now uses an allowlisted `proxy_decision` schema:
  - route kind
  - transport kind
  - local auth result
  - decision outcome/reason
  - response commit state
  - optional redacted account hash
  - optional static error class
- `AuditFileSink` writes JSONL events to a private router-root file:
  - parent directory mode `0700` on Unix
  - audit file mode `0600` on Unix
  - no request/response body, local router token, upstream OAuth token, raw
    account id, or account label is serialized
- loopback runtime config now accepts `with_audit_file(...)`
- HTTP/SSE proxy emits:
  - rejected local-auth decisions before selection/upstream
  - allowed forwarded decisions after upstream response commit
- WebSocket is treated as core transport, not an HTTP afterthought:
  - WebSocket preflight local-auth rejection emits a WebSocket audit event
  - WebSocket first-frame router emits allowed/rejected WebSocket audit events
    after local auth and account selection
  - WebSocket audit does not log first-frame payload bytes

Audit sink proof:

- RED: `cargo nextest run -p codex-router-proxy assembled_loopback_router_runtime_writes_redacted_private_audit_events loopback_router_runtime_dispatches_websocket_upgrade_to_tunnel`
  failed because `LoopbackRouterRuntimeConfig::with_audit_file` did not exist
- GREEN: same focused proxy command:
  - 2 tests run
  - 2 passed
  - 34 skipped by filter
- `cargo nextest run -p codex-router-core`:
  - 8 tests run
  - 8 passed
  - 0 skipped
- full sweep after audit sink slice:
  - `cargo fmt --all --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 95 tests run, 95 passed, 2 skipped
  - `cargo deny check`: pass; duplicate transitive warnings only for
    `getrandom` and `windows-sys`
  - `cargo audit`: pass, scanned 181 crate dependencies
  - `actionlint .github/workflows/ci.yml`: pass
  - `tests/smoke/installed_codex_mock.sh`: pass, 2 smoke tests passed
  - `git diff --check`: pass
  - forbidden-scope guard: pass
  - quota dependency guard: pass
  - test-support production dependency guard: pass

Scope note:

- This closes the accepted audit-sink implementation-review finding for
  HTTP/SSE and WebSocket routing decisions.
- Remaining accepted implementation-review work:
  - quota runtime state
  - installed-Codex HTTP/SSE smoke coverage
  - profile preview-first workflow

Next implementation slice:
Continue implementation execution with quota runtime state or installed-Codex
HTTP/SSE smoke coverage.

## Implementation-Execute Quota Runtime State Slice

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

Quota runtime state slice result:

- SQLite quota snapshots are now keyed by `(account_id, route_band)` instead of
  account id alone.
- `QuotaSnapshotRepository` exposes `load_snapshot_for_route_band(...)`.
- `RepositoryBackedAccountSelector` loads quota for the request route band
  directly, so `/v1/models`, `/v1/responses`, compact, memory trace, and
  WebSocket response routing do not overwrite or accidentally read each
  other's quota state.
- `LoopbackRouterRuntime` now owns process-lifetime weighted selector state and
  passes a shared selector handle into per-connection HTTP/SSE and WebSocket
  dispatch paths.
- `codex-router serve` defaults `now_unix_seconds` to system time when
  `--now-unix-seconds` is omitted; the explicit flag remains available for
  deterministic tests.

Quota runtime state proof:

- RED: `cargo nextest run -p codex-router-state quota_snapshots_are_partitioned_by_route_band_for_one_account`
  failed because `load_snapshot_for_route_band` did not exist.
- GREEN: same state command:
  - 1 test run
  - 1 passed
  - 6 skipped by filter
- RED: `cargo nextest run -p codex-router-proxy loopback_router_runtime_balances_across_connections_with_process_selector_state`
  failed because mixed protocol dispatch selected `alpha-token` twice.
- GREEN: same proxy command:
  - 1 test run
  - 1 passed
  - 36 skipped by filter
- RED: `cargo nextest run -p codex-router-cli serve_command_defaults_quota_clock_to_system_time`
  failed because serve defaulted the quota clock to zero.
- GREEN: same CLI command:
  - 1 test run
  - 1 passed
  - 17 skipped by filter
- added selector-level proof:
  `repository_backed_selector_uses_route_specific_quota_snapshots`.
- affected-crate proof:
  - `cargo nextest run -p codex-router-state -p codex-router-proxy -p codex-router-cli`:
    63 tests run, 63 passed, 0 skipped
- full sweep after quota runtime state slice:
  - `cargo fmt --all --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 99 tests run, 99 passed, 2 skipped
  - `cargo deny check`: pass; duplicate transitive warnings only for
    `getrandom` and `windows-sys`
  - `cargo audit`: pass, scanned 181 crate dependencies
  - `actionlint .github/workflows/ci.yml`: pass
  - `tests/smoke/installed_codex_mock.sh`: pass, 2 smoke tests passed
  - `git diff --check`: pass
  - forbidden-scope guard: pass
  - quota dependency guard: pass
  - test-support production dependency guard: pass

Scope note:

- This closes the accepted quota runtime state implementation-review finding:
  route-specific quota persistence, nonzero default runtime clock, and
  process-lifetime selector balancing are all covered by current tests.
- Remaining accepted implementation-review work:
  - installed-Codex HTTP/SSE smoke coverage
  - profile preview-first workflow

Next implementation slice:
Continue implementation execution with installed-Codex HTTP/SSE smoke coverage
or profile preview-first workflow.

## Implementation-Execute Profile Preview-First Slice

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

Profile preview-first slice result:

- `CodexRouterProfileWriter::dry_run` now captures the target path, proposed
  profile content, current target content when present, and a deterministic
  preview token tied to that exact target/current/proposed triple.
- `CodexRouterProfileWriter::write` still requires explicit
  `--approve-codex-home-write`, and now also requires the preview token from a
  prior dry-run. Approval alone is no longer enough to mutate Codex home.
- `profile write --dry-run` now prints:
  - `target: ...`
  - `preview-token: ...`
  - `current: <missing>` or prefixed current lines
  - prefixed proposed lines
- The installed-Codex smoke helper uses the same preview-token handshake before
  writing its isolated temp `CODEX_HOME` profile, so smoke proof cannot bypass
  the real profile safety contract.
- The rendered profile contract still includes `supports_websockets = true`;
  WebSocket remains core and is covered by the existing profile assertion and
  installed-Codex WebSocket smoke.

Profile preview-first proof:

- RED: `cargo nextest run -p codex-router-cli profile_write` initially failed
  before implementation because the new CLI/write signature had no matching
  profile writer support.
- GREEN after implementation and formatting:
  - `cargo nextest run -p codex-router-cli profile_write`: 6 tests run,
    6 passed, 14 skipped by filter
- full sweep after profile preview-first slice:
  - `cargo fmt --all --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 101 tests run, 101 passed, 2 skipped
  - `cargo deny check`: pass; duplicate transitive warnings only for
    `getrandom` and `windows-sys`
  - `cargo audit`: pass, scanned 181 crate dependencies
  - `actionlint .github/workflows/ci.yml`: pass
  - `tests/smoke/installed_codex_mock.sh`: pass, 2 smoke tests passed
  - `git diff --check`: pass
  - forbidden-scope guard: pass
  - quota dependency guard: pass
  - test-support production dependency guard: pass

Scope note:

- This closes the accepted profile preview-first implementation-review
  finding. Home mutation is now preview-first, token-confirmed, explicit, and
  still scoped to the named `codex-router.config.toml` file in the supplied
  Codex home.
- Remaining accepted implementation-review work:
  - installed-Codex HTTP/SSE smoke coverage

Next implementation slice:
Continue implementation execution with installed-Codex HTTP/SSE smoke coverage.

## Implementation-Execute Installed-Codex HTTP/SSE Smoke Slice

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

Installed-Codex HTTP/SSE smoke slice result:

- The installed-Codex mock smoke now runs two real `codex exec` legs against
  the same helper-generated temp `CODEX_HOME` profile and local router token:
  - an HTTP/SSE leg that keeps the default generated profile file intact but
    uses a smoke-only CLI override
    `model_providers.codex-router.supports_websockets=false` so installed
    Codex exercises `/v1/responses` over HTTP/SSE
  - the normal WebSocket leg through the default generated profile with
    `supports_websockets = true`
- The mock upstream now captures and redacts both:
  - HTTP/SSE request line, headers, and body for `POST /v1/responses`
  - WebSocket handshake headers and first `response.create` frame
- HTTP/SSE mock response now emits the Responses SSE sequence required by
  Codex compatibility:
  `response.created`, `response.output_item.added`,
  `response.content_part.added`, `response.output_text.delta`,
  `response.output_text.done`, `response.content_part.done`,
  `response.output_item.done`, and `response.completed`.
- The redacted transcript proves both transports in the same smoke artifact:
  - `http_sse_request_line = "POST /v1/responses HTTP/1.1"`
  - `http_sse_stream_requested = true`
  - `http_sse_local_router_header_present = false`
  - `handshake_count = 1`
  - `first_frame_type = "response.create"`
  - `local_router_header_present = false`
- WebSocket remains core: the generated profile still declares
  `supports_websockets = true`, and the smoke-only HTTP/SSE leg does not change
  the default profile contract.

Installed-Codex HTTP/SSE smoke proof:

- RED: implementation-review finding showed latest transcript had
  `http_probe_count: 0`, one WebSocket handshake, and no HTTP/SSE proof.
- GREEN:
  - `tests/smoke/installed_codex_mock.sh`: 2 tests passed
  - latest transcript:
    `tmp/smoke/installed-codex-mock-65274-1781994825826.json`
  - transcript excerpt:
    - `http_sse_request_line`: `POST /v1/responses HTTP/1.1`
    - `http_sse_stream_requested`: `true`
    - `http_sse_local_router_header_present`: `false`
    - `handshake_count`: `1`
    - `first_frame_type`: `response.create`
    - `local_router_header_present`: `false`
- full sweep after installed-Codex HTTP/SSE smoke slice:
  - `cargo fmt --all --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 101 tests run, 101 passed, 2 skipped
  - `cargo deny check`: pass; duplicate transitive warnings only for
    `getrandom` and `windows-sys`
  - `cargo audit`: pass, scanned 181 crate dependencies
  - `actionlint .github/workflows/ci.yml`: pass
  - `tests/smoke/installed_codex_mock.sh`: pass, 2 smoke tests passed
  - `git diff --check`: pass
  - forbidden-scope guard: pass
  - quota dependency guard: pass
  - test-support production dependency guard: pass

Scope note:

- This closes the accepted installed-Codex HTTP/SSE smoke implementation-review
  finding. The smoke now proves both HTTP/SSE and WebSocket exchanges through
  installed Codex with isolated config, helper-rendered profile/token setup,
  upstream auth injection, and local token stripping.
- Remaining accepted implementation-review work: none recorded in the latest
  workflow state.

Next implementation slice:
Run the next implementation-review or PR readiness workflow over the current
repo state.

## Implementation-Review Fix Batch For Profile And Smoke Closure

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress after
implementation-review reception

Review findings accepted and fixed:

- Preview-first profile writes no longer treat unreadable, invalid UTF-8, or
  otherwise unpreviewable existing files as missing. `dry_run` is now fallible
  and write reuses the same fallible preview path.
- Preview tokens now distinguish missing current state from present empty
  current state, preventing stale previews from overwriting a file that appears
  after preview.
- Profile dry-run output redacts secret-looking current values before stdout
  while still binding the preview token to the unredacted current content.
- Profile no-write tests now assert the actual target
  `codex-router.config.toml`, not unrelated `config.toml`.
- Installed-Codex smoke script no longer hardcodes
  `/Users/shravansunder/...`; it resolves a working Cargo path through standard
  PATH/rustup/user-toolchain fallbacks.
- Installed-Codex child processes now run with a cleared environment plus temp
  `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`, `CODEX_HOME`,
  and `CODEX_ROUTER_TOKEN`.
- Installed-Codex timeout failures suppress captured stdout/stderr byte content
  to avoid leaking secrets in failure logs.
- Installed-Codex smoke now asserts both HTTP/SSE and WebSocket legs surface the
  expected model text through stdout and their `--output-last-message` files.
- Installed-Codex smoke contract now fails if the local router token appears in
  the upstream HTTP/SSE body or WebSocket first frame.
- Mock upstream accept deadline is reset per accepted connection, so a slow
  HTTP/SSE leg does not consume the WebSocket phase budget.

Review-fix proof:

- focused profile proof:
  - `cargo nextest run -p codex-router-cli profile_write`: 10 tests run,
    10 passed, 14 skipped by filter
- focused smoke contract proof:
  - `cargo nextest run -p codex-router-test-support smoke_contract smoke_visible timed_out`:
    4 tests run, 4 passed, 4 skipped by filter
- full sweep after review-fix batch:
  - `cargo fmt --all --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 109 tests run, 109 passed, 2 skipped
  - `cargo deny check`: pass; duplicate transitive warnings only for
    `getrandom` and `windows-sys`
  - `cargo audit`: pass, scanned 181 crate dependencies
  - `actionlint .github/workflows/ci.yml`: pass
  - `tests/smoke/installed_codex_mock.sh`: pass, 2 smoke tests passed
  - latest transcript:
    `tmp/smoke/installed-codex-mock-95126-1781995693334.json`
  - transcript excerpt:
    - `http_sse_request_line`: `POST /v1/responses HTTP/1.1`
    - `http_sse_stream_requested`: `true`
    - `http_sse_local_router_header_present`: `false`
    - `http_sse_local_router_token_in_body`: `false`
    - `handshake_count`: `1`
    - `first_frame_type`: `response.create`
    - `local_router_header_present`: `false`
    - `websocket_local_router_token_in_first_frame`: `false`
  - `rg -n "unwrap\\(|expect\\(" crates`: no matches
  - forbidden-scope guard: pass
  - quota dependency guard: pass
  - test-support production dependency guard: pass
  - `git diff --check`: pass

Scope note:

- The review-fix batch addresses all accepted candidate findings from the
  compact implementation-review lanes for profile preview-first and
  installed-Codex smoke closure.
- WebSocket remains core: the generated profile still declares
  `supports_websockets = true`; HTTP/SSE is exercised only by a smoke-only
  installed-Codex override.

Next implementation slice:
Run a final implementation-review/PR-readiness pass over the repo state.

## Final Implementation Review Pass

Current workflow:
`shravan-dev-workflow:implementation-review-swarm` complete

Final review result:

- A final read-only reviewer pass over the review-fix batch returned:
  `No findings.`
- Reviewed scope:
  - `crates/codex-router-cli/src/profile.rs`
  - `crates/codex-router-cli/src/lib.rs`
  - `crates/codex-router-test-support/src/installed_codex.rs`
  - `tests/smoke/installed_codex_mock.sh`
  - workflow-state details
- The final review specifically checked that:
  - profile dry-run/write is preview-first, stale-token-safe, and redacted
  - unreadable/unpreviewable profile state fails closed
  - generated profile keeps `supports_websockets = true`
  - installed-Codex smoke is isolated, portable, output-asserting, and
    local-token-leak checking
  - WebSocket remains core while HTTP/SSE is exercised only through a smoke-only
    installed-Codex override

Transition:

- Current workflow: `shravan-dev-workflow:implementation-review-swarm`
- Next workflow: `shravan-dev-workflow:implementation-pr-wrapup`
- Reason: implementation proof is complete, accepted implementation-review
  findings were fixed and rechecked, and final implementation-review pass has
  no remaining accepted findings.

Next implementation slice:
Commit/push/open PR, then prove PR readiness with fresh local/remote/checks,
comments, review-thread, mergeability, and head-SHA evidence.

## Implementation-Execute Local Token Lifecycle Runtime Slice

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

Local token lifecycle runtime slice result:

- added `token init --router-root <path>` as a production CLI command that
  creates the initial local router token when missing and prints only generation
  metadata
- added `token rotate --router-root <path>` as a production CLI command that
  advances the local token generation and prints only generation metadata
- preserved `token export --router-root <path>` as the only CLI command that
  reveals local token material for `CODEX_ROUTER_TOKEN`
- changed `LocalRouterTokenService` rotation to retain one previous token
  generation so old-token handshakes can be classified as old rather than
  unknown
- changed the proxy local auth gate from a startup-only value to a shared
  reloadable auth snapshot
- added `LocalAuthReloader`, a small thread-safe handle for swapping local auth
  without sharing the full SQLite-backed runtime across threads
- added WebSocket revocation tracking keyed by local token generation
- runtime reload now closes active WebSocket streams authenticated with old
  token generations while preserving WebSocket as a core supported transport

Local token lifecycle runtime slice proof:

- RED: `cargo nextest run -p codex-router-cli token_init_and_rotate_commands_do_not_print_secret_and_update_export`
  failed because `token init` was an unknown command
- RED: `cargo nextest run -p codex-router-proxy loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket`
  failed because `LoopbackRouterRuntime` had no local auth reload/revocation
  API
- GREEN: `cargo nextest run -p codex-router-cli token_init_and_rotate_commands_do_not_print_secret_and_update_export`:
  1 test passed, 15 skipped by filter
- GREEN: `cargo nextest run -p codex-router-proxy loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket`:
  1 test passed, 34 skipped by filter
- full sweep after local token lifecycle runtime slice:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 93 tests passed, 2 skipped
  - `cargo deny check`: pass; duplicate transitive warnings for `getrandom`
    and `windows-sys` only
  - `cargo audit`: pass; scanned 181 crate dependencies
  - `actionlint .github/workflows/ci.yml`: pass
  - `tests/smoke/installed_codex_mock.sh`: 2 smoke tests passed
  - forbidden-scope and dependency guard checks: pass
  - `git diff --check`: pass

Proof environment note:

- Bare `cargo` was not on `PATH` in this shell. Proof commands used the pinned
  toolchain path:
  `PATH=/Users/shravansunder/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH`.

Scope note:

- This slice proves the token init/rotate/export CLI surfaces and the
  runtime-local reload/revocation mechanism.
- It does not yet prove a separate `codex-router token rotate` process can
  notify an already-running `codex-router serve` process without an explicit
  reloader call, file watcher, admin socket, or other live control channel.
  That remaining cross-process activation behavior should be handled in a
  follow-up token lifecycle slice or explicitly scoped in implementation review.

Next implementation slice:
Resolve the remaining live serve reload semantics for token rotation, then
continue accepted implementation-review findings for audit sink, quota runtime
state, installed-Codex HTTP/SSE smoke coverage, and preview-first profile write.

## Implementation-Execute Serve Token Rotation Watcher Slice

Timestamp: 2026-06-20T21:58:29Z

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

Serve token rotation watcher slice result:

- added a CLI-owned `LocalTokenReloadWatcher` around `codex-router serve`
- the watcher observes the router-owned secret store for local token generation
  changes while `serve` is running
- when generation changes, the watcher loads the current+previous local auth
  snapshot and calls the proxy runtime `LocalAuthReloader`
- active WebSocket connections authenticated with old token generations are
  closed by the runtime revocation registry
- old-token HTTP requests now map to a local `401 Unauthorized` response instead
  of escaping as a server error
- new-token HTTP traffic succeeds through the same still-running `serve`
  process after `codex-router token rotate`
- no Codex route, admin API, home mutation, provider timeout/gating layer, or
  Prodex-style gateway surface was added

Serve token rotation watcher slice proof:

- RED: `cargo nextest run -p codex-router-cli serve_command_reloads_token_rotation_without_restart`
  failed because the old-token WebSocket stayed open after `token rotate`
- GREEN: `cargo nextest run -p codex-router-cli serve_command_reloads_token_rotation_without_restart`:
  1 test passed, 16 skipped by filter
- full sweep after serve token rotation watcher slice:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 94 tests passed, 2 skipped
  - `cargo deny check`: pass; duplicate transitive warnings for `getrandom`
    and `windows-sys` only
  - `cargo audit`: pass; scanned 181 crate dependencies
  - `actionlint .github/workflows/ci.yml`: pass
  - `tests/smoke/installed_codex_mock.sh`: 2 smoke tests passed
  - forbidden-scope and dependency guard checks: pass
  - `git diff --check`: pass
  - workflow `events.jsonl` validation with `jq -c .`: pass

Proof environment note:

- Bare `cargo` was not on `PATH` in this shell. Proof commands used the pinned
  toolchain path:
  `PATH=/Users/shravansunder/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH`.

Scope note:

- This closes the cross-process serve reload semantics that were left open by
  the prior token lifecycle runtime slice.
- The broader implementation remains in `implementation-execute-plan` because
  accepted implementation-review findings remain open for audit sink, quota
  runtime state, installed-Codex HTTP/SSE smoke coverage, and preview-first
  profile write.

Next implementation slice:
Continue accepted implementation-review findings, with audit sink or quota
runtime state as the next highest-risk non-token blocker.

## Implementation-Execute Revision: Transport Preflight Slice

Timestamp: 2026-06-20T21:25:12Z

Current workflow:
`shravan-dev-workflow:implementation-execute-plan`

Input review:
`docs/plans/reviews/2026-06-20-codex-router-implementation-review.md`

Coverage loaded before edits:

- Implementation plan: 861 lines, read `1-220`, `221-520`, `521-720`,
  `721-861`.
- Implementation review: 237 lines, read `1-237`.

Accepted finding deltas:

- Partially addressed blocker 1:
  HTTP local request parsing no longer waits for client EOF. A normal client can
  send a complete `Content-Length` request and receive a response without
  calling `Shutdown::Write`.
- Partially addressed blocker 2:
  runtime WebSocket handling now validates local router token and actual
  handshake path before `accept_hdr`. Missing-token upgrades and unsupported
  paths such as `/v1/realtime` fail before local WebSocket accept. Accepted,
  authenticated `/v1/responses` upgrades now have a bounded first-frame wait.

Still open from the implementation review:

- blocker 1 still needs HTTPS-capable HTTP upstream forwarding and true SSE
  streaming proof.
- blocker 2 still needs follow-up review for completeness, but the accepted
  auth/path/pre-accept and first-frame deadline subfindings have local protocol
  proof.
- blocker 3 local token lifecycle remains open.
- important findings for audit sink, quota/runtime balancing, installed-Codex
  HTTP/SSE smoke, and profile preview confirmation remain open.

Red/green proof captured:

- RED:
  `cargo nextest run -p codex-router-proxy loopback_http_adapter_responds_without_client_write_shutdown`
  failed with client timeout `Resource temporarily unavailable (os error 35)`.
- GREEN:
  `cargo nextest run -p codex-router-proxy loopback_http_adapter_responds_without_client_write_shutdown`
  passed, 1 run / 1 passed.
- RED:
  `cargo nextest run -p codex-router-proxy loopback_router_runtime_rejects_websocket_upgrade_without_token loopback_router_runtime_rejects_unsupported_websocket_path_before_accept`
  failed because both handshakes were accepted.
- GREEN:
  same WebSocket command passed, 2 run / 2 passed.
- RED:
  `cargo nextest run -p codex-router-proxy loopback_router_runtime_bounds_websocket_wait_for_first_frame`
  failed with `timed out waiting on channel`.
- GREEN:
  same no-first-frame command passed, 1 run / 1 passed.

Fresh validation after slice:

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo nextest run --workspace`: 89 run, 89 passed, 2 skipped.

Tooling note:

- Bare `cargo` was not on PATH in this resumed shell. The pinned toolchain was
  repaired with `rustup toolchain install 1.95.0 --component clippy,rustfmt,rust-src`,
  then proof commands used `RUSTUP_TOOLCHAIN=1.95.0` with the pinned toolchain
  bin and `~/.cargo/bin` on PATH.

phase_result: in_progress
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Continue implementation fixes for HTTPS/SSE streaming, token lifecycle, and remaining important findings before another implementation review.

## Implementation-Execute Revision: HTTPS And SSE Streaming Slice

Timestamp: 2026-06-20T21:38:54Z

Current workflow:
`shravan-dev-workflow:implementation-execute-plan`

Input review:
`docs/plans/reviews/2026-06-20-codex-router-implementation-review.md`

Coverage loaded before edits:

- Implementation plan: 861 lines, read `1-220`, `221-520`, `521-720`,
  `721-861`.
- Implementation review: 237 lines, read `1-237`.
- Current transport/runtime code: `crates/codex-router-proxy/src/http_sse.rs`,
  `crates/codex-router-proxy/src/upstream.rs`, `crates/codex-router-proxy/src/server.rs`.
- DeepWiki source check: `seanmonstar/reqwest`, blocking client and streaming
  response body APIs.

Accepted finding deltas:

- Further addressed blocker 1:
  runtime HTTP/SSE forwarding now has a streaming response path; local response
  headers are written before the upstream body reaches EOF, and body bytes are
  copied incrementally to the local client.
- Further addressed blocker 1:
  `HttpUpstreamTransport` no longer rejects `https://` endpoints at send time.
  HTTPS requests use `reqwest::blocking` with `rustls-tls`, default features
  disabled, and redirect following disabled.
- Existing buffered HTTP proxy APIs remain for unit-level protocol tests, but
  `LoopbackRouterRuntime` now uses the streaming request handler path.

Red/green proof captured:

- RED:
  `cargo nextest run -p codex-router-proxy http_upstream_transport_accepts_https_endpoints_at_send_time assembled_loopback_router_runtime_streams_sse_before_upstream_eof`
  failed because HTTPS still returned `http upstream transport requires http endpoint`
  and the SSE client timed out before upstream EOF.
- GREEN:
  same command passed, 2 run / 2 passed.

Fresh validation after slice:

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo nextest run --workspace`: 91 run, 91 passed, 2 skipped.
- `cargo deny check`: pass; warnings only for duplicate transitive crates
  allowed by current `deny.toml`.
- `cargo audit`: pass, scanned 181 crate dependencies.
- `actionlint .github/workflows/ci.yml`: pass.
- `tests/smoke/installed_codex_mock.sh`: pass, 2 smoke tests passed.
- `git diff --check`: pass.
- forbidden-scope search: pass, no matches.
- quota dependency guard: pass, `codex-router-quota` does not depend on
  `codex-router-secret-store`.
- production dependency guard: pass, only `codex-router-test-support` depends
  on test-support.

Dependency-policy delta:

- Added `reqwest` with `default-features = false`, `blocking`, and `rustls-tls`
  for HTTPS-capable upstream forwarding.
- Added explicit license allowlist entries for TLS/root-store transitive
  dependencies: `ISC` and `CDLA-Permissive-2.0`.

New user auth-mode input:

- User wants beginning support for three credential modes:
  1. OAuth account mode.
  2. OAuth plus the router-local token/header activation mode.
  3. OpenAI API key mode for easier testing.
- This is not implemented in this transport slice. It should be reconciled into
  the spec/plan before the next auth/token-lifecycle implementation pass.
- Do not accept raw 1Password secret material in chat. Live proof should use an
  approved 1Password-managed local secret/env reference or MCP-backed retrieval.

User transport invariant:

- WebSocket is core for codex-router v1 and must remain in scope. It is not an
  optional transport, fallback experiment, or removable simplification. HTTP/SSE
  fixes must not regress or de-scope WebSocket support.

Still open from the implementation review:

- blocker 3 local token lifecycle remains open, now with the added auth-mode
  design input above.
- important findings for audit sink, quota/runtime balancing, installed-Codex
  HTTP/SSE smoke coverage, and profile preview confirmation remain open.
- HTTPS proof currently proves the transport no longer rejects HTTPS at
  send-time; it does not yet stand up a local TLS upstream with a trusted test
  root. Add stronger local TLS proof if implementation review requires it.

phase_result: in_progress
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Reconcile the new three-mode auth input before implementing the token lifecycle blocker; remaining important findings still need implementation before another review.

## Implementation-Execute T11 Gated Live OAuth And Quota Runbook Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T11 gated live result:

- added live proof runbook:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/testing/live-oauth-quota.md`
- live gate is recorded as `not-run: approval required`
- no live OAuth login, real quota fetch, real account rotation, or real quota
  pooling command was run
- current CLI has no tested live OAuth/quota command
- runbook explicitly forbids inventing `codex-router live-proof`,
  `codex-router login`, or `codex-router quota` commands before those surfaces
  are designed, implemented, tested, and added to the runbook
- future approved live proof must redact tokens, raw account emails, request
  bodies, response bodies, prompts, memory traces, and tool arguments

T11 proof:

```text
wc -l docs/testing/live-oauth-quota.md: 116 lines
git diff --check: pass, exit 0
executable-surface fake live command guard: pass, exit 0
runbook required-status markers: present, exit 0
cargo fmt --all -- --check: pass, exit 0
cargo clippy --workspace --all-targets -- -D warnings: pass, exit 0
cargo nextest run --workspace: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
cargo deny check: advisories ok, bans ok, licenses ok, sources ok, exit 0
cargo audit: scanned 73 crate dependencies, exit 0
actionlint .github/workflows/ci.yml: pass, exit 0
tests/smoke/installed_codex_mock.sh: 2 ignored smoke tests passed, exit 0
```

Scope note:

- T11 is the approval boundary only. If live proof becomes required before a
  tested live CLI exists, the next action is replan, not ad hoc live execution.

Next implementation slice:
Run final T11 docs/quality checks, then record the orchestrator transition to
`shravan-dev-workflow:implementation-review-swarm` if implementation proof is
complete.

## Implementation-Execute Result

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` complete

Official next workflow:
`shravan-dev-workflow:implementation-review-swarm`

Implementation-execute evidence:

- implementation provenance:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/wip/implementation-provenance.md`
- T11 live-gate runbook:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/testing/live-oauth-quota.md`
- execution brief:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/plan-workflows/2026-06-20-codex-router-main-implementation-execute/implementation-execute-plan-brief.md`
- latest installed-Codex smoke transcript:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/smoke/installed-codex-mock-26264-1781989282085.json`
- fresh verification:
  - `cargo fmt --all -- --check`: pass, exit 0
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass, exit 0
  - `cargo nextest run --workspace`: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
  - `cargo deny check`: advisories ok, bans ok, licenses ok, sources ok, exit 0
  - `cargo audit`: scanned 73 crate dependencies, exit 0
  - `actionlint .github/workflows/ci.yml`: pass, exit 0
  - `tests/smoke/installed_codex_mock.sh`: 2 ignored smoke tests passed, exit 0
  - `git diff --check`: pass, exit 0

Phase recommendation footer:

```text
phase_result: complete
evidence: implementation provenance, T11 runbook, execution brief, latest smoke transcript, full local proof commands from 2026-06-20
recommended_next_workflow: shravan-dev-workflow:implementation-review-swarm
recommended_transition_reason: Implementation execution proof is captured through T11; live account proof remains gated and unrun, so the next unproven lifecycle gate is adversarial implementation review.
```

## Implementation-Review Result

Implementation review artifact:
`/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/plans/reviews/2026-06-20-codex-router-implementation-review.md`

Current workflow:
`shravan-dev-workflow:implementation-review-swarm` complete

Official next workflow:
`shravan-dev-workflow:implementation-execute-plan`

Verdict:
`not_ready`

Accepted blocker findings:

- HTTP/SSE transport can hang, buffers streams, and cannot reach real HTTPS
  upstreams.
- WebSocket upgrades are accepted before auth/path validation and can block the
  serial serve loop.
- Local token lifecycle is incomplete for a production router.

Accepted important findings:

- Audit logging is a schema stub, not the required private router-root sink.
- Quota state and selection runtime do not preserve route-specific or
  process-lifetime balancing.
- Installed-Codex smoke does not prove the HTTP/SSE side of R20.
- Profile write is not a true preview-first home mutation workflow.

Phase recommendation footer:

```text
phase_result: needs_revision
evidence: implementation review report, implementation review packet, reviewer lane outputs, code citations, git diff --check
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Accepted blocker findings make the implementation not ready; fixes and proof belong to implementation execution before another review.
```

Next implementation slice:
Plan and execute review-finding fixes, starting with the transport and
WebSocket blockers before PR readiness.

## Implementation-Execute T8 Loopback Router Runtime Assembly Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 loopback router runtime assembly partial result:

- added `LoopbackRouterRuntimeConfig`
- added `LoopbackRouterRuntime`
- runtime owns loopback listener, SQLite state store, file secret store, local
  auth gate, and HTTP upstream transport
- bounded serve method builds the borrowed repository-backed selector and
  authenticated HTTP proxy service inside the method call
- real local-socket test proves:
  - router binds loopback
  - local token is validated before forwarding
  - account selection reads router-owned SQLite quota/account metadata
  - upstream token reads router-owned file secret store
  - local router token is stripped
  - selected upstream auth is injected
  - mock upstream response is returned to the local client
- no retry, timeout, health, circuit, or provider-gating policy was added

T8 loopback router runtime assembly partial proof:

- RED: `cargo nextest run -p codex-router-proxy assembled_loopback_router_runtime`
  failed because `LoopbackRouterRuntime` and
  `LoopbackRouterRuntimeConfig` were missing
- GREEN: `cargo nextest run -p codex-router-proxy assembled_loopback_router_runtime`:
  1 assembled runtime test passed, 24 skipped by filter
- full sweep after T8 loopback router runtime assembly partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 80 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This is assembled HTTP/SSE runtime proof. Installed-Codex smoke, CLI server
  command, real upstream WebSocket tunneling, live OAuth/quota proof,
  implementation review, and PR readiness remain open.

Next implementation slice:
Wire the CLI server command to this runtime or complete real upstream WebSocket
tunnel proof before T10 installed-Codex mock smoke.

## Implementation-Execute T9 CLI Serve Command Wiring Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T9 CLI serve command wiring partial result:

- added CLI concrete dependencies on `codex-router-proxy` and
  `codex-router-state`
- wired `codex-router serve`
- serve command requires:
  - `--state-db`
  - `--secret-root`
  - `--upstream-base-url`
- serve command supports:
  - `--listen-host`
  - `--port`
  - `--now-unix-seconds`
  - `--max-snapshot-age-seconds`
  - `--max-connections`
- serve command loads the current local router token from the router-owned
  secret root through `LocalRouterTokenService`
- serve command starts `LoopbackRouterRuntime`
- bounded serve test proves one local Codex-shaped request reaches a mock
  upstream with local token stripped and selected upstream auth injected

T9 CLI serve command wiring partial proof:

- RED: `cargo nextest run -p codex-router-cli serve_command_starts_runtime`
  failed because `serve` was an unknown command
- GREEN: `cargo nextest run -p codex-router-cli serve_command_starts_runtime`:
  1 serve command test passed, 13 skipped by filter
- full sweep after T9 CLI serve command wiring partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 81 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This is real CLI HTTP/SSE serve path proof with isolated router state and
  secrets. Installed-Codex smoke, real upstream WebSocket tunneling, live
  OAuth/quota proof, implementation review, and PR readiness remain open.

Next implementation slice:
Build T10 installed-Codex mock smoke scaffolding for the HTTP/SSE path, or
complete real upstream WebSocket tunnel proof first if the smoke requires it.

## Implementation-Execute T10 Installed-Codex Mock Smoke Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T10 installed-Codex mock smoke result:

- installed Codex version observed by the smoke: `codex-cli 0.141.0`
- generated Codex profile now uses profile-v2 overlay file:
  `codex-router.config.toml`
- generated custom provider includes `name = "codex-router"` because installed
  Codex rejects an unnamed provider
- smoke command uses `codex --profile codex-router exec ...` with
  `-c approval_policy="never"` and temp `CODEX_HOME`
- smoke generates the local router token through the router token service and
  shell export helper
- smoke seeds router-owned SQLite state and file secrets, then starts the real
  loopback router runtime against a mock upstream
- mock upstream transcript proves:
  - one real WebSocket handshake
  - selected upstream account bearer injected
  - local router token stripped
  - first upstream frame is `response.create`
  - frame model and stream fields are preserved
- hostile no-token smoke proves unauthenticated local WebSocket traffic does not
  reach upstream
- redacted transcript:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/smoke/installed-codex-mock-26264-1781989282085.json`

T10 smoke proof:

- RED: `cargo test -p codex-router-test-support installed_codex_hostile_no_token_smoke_keeps_upstream_empty -- --ignored --nocapture`
  failed because `run_hostile_no_token_smoke` was missing
- GREEN: same command passed, 1 hostile no-token smoke test passed
- `tests/smoke/installed_codex_mock.sh`: exit 0, 2 ignored smoke tests passed

Latest full proof after T10:

```text
cargo fmt --all -- --check: pass, exit 0
cargo clippy --workspace --all-targets -- -D warnings: pass, exit 0
cargo nextest run --workspace: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
cargo deny check: advisories ok, bans ok, licenses ok, sources ok, exit 0
cargo audit: scanned 73 crate dependencies, exit 0
actionlint .github/workflows/ci.yml: pass, exit 0
tests/smoke/installed_codex_mock.sh: 2 ignored smoke tests passed, exit 0
forbidden-scope and dependency guard checks: pass, exit 0
```

Scope note:

- T10 mock smoke is covered without writing to `~/.codex`.
- Live OAuth/quota proof remains approval-gated and unrun.
- Implementation review, PR readiness, and any live account checks remain open.

Next implementation slice:
Run final T10 full proof gates, then proceed to implementation review planning
or the next reviewed-plan task.

## Implementation-Execute T8 Blocking WebSocket Tunnel Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 blocking WebSocket tunnel partial result:

- added `tungstenite` as the real blocking WebSocket handshake/frame library
- added `BlockingWebSocketTunnel`
- tunnel accepts a local WebSocket connection and captures handshake headers
- tunnel reuses existing `AuthenticatedWebSocketRouter` for:
  - local auth
  - account selection
  - first-frame size/type/JSON/`response.create` validation
  - selected upstream auth injection
  - local/hop/client auth header stripping
- tunnel opens a real upstream WebSocket connection with sanitized headers
- tunnel forwards the first `response.create` frame unchanged
- tunnel forwards a bounded number of upstream frames back to the local client
  for deterministic protocol proof
- no retry, timeout, health, circuit, or provider-gating policy was added

T8 blocking WebSocket tunnel partial proof:

- RED: `cargo nextest run -p codex-router-proxy blocking_websocket_tunnel`
  failed because `BlockingWebSocketTunnel` was missing
- GREEN: `cargo nextest run -p codex-router-proxy blocking_websocket_tunnel`:
  1 blocking tunnel test passed, 25 skipped by filter
- full sweep after T8 blocking WebSocket tunnel partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 82 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This is real WebSocket protocol tunnel proof. The loopback HTTP server and
  CLI `serve` path still do not dispatch WebSocket upgrades through the tunnel.
  Installed-Codex smoke, live OAuth/quota proof, implementation review, and PR
  readiness remain open.

Next implementation slice:
Wire WebSocket upgrade handling into the loopback runtime/CLI serve path, then
build the installed-Codex mock smoke harness.

## Implementation-Execute T8 Runtime WebSocket Dispatch Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T8 runtime WebSocket dispatch partial result:

- added WebSocket URL assembly to `UpstreamEndpoint`
- added explicit runtime config for bounded upstream-to-local WebSocket frame
  forwarding
- added `LoopbackRouterRuntime::serve_protocol_connections`
- runtime protocol serve loop accepts loopback connections and peeks only the
  HTTP head for `Upgrade: websocket` classification
- WebSocket upgrades dispatch into `BlockingWebSocketTunnel`
- non-WebSocket requests continue through the existing HTTP adapter
- runtime-level test proves real local WebSocket client through the bound router
  listener to a mock WebSocket upstream, with:
  - local token stripped
  - selected upstream auth injected
  - first `response.create` frame preserved
  - mock upstream response returned to the local client

T8 runtime WebSocket dispatch partial proof:

- RED: `cargo nextest run -p codex-router-proxy loopback_router_runtime_dispatches_websocket`
  failed because `LoopbackRouterRuntime::serve_protocol_connections` was
  missing
- GREEN: `cargo nextest run -p codex-router-proxy loopback_router_runtime_dispatches_websocket`:
  1 runtime WebSocket dispatch test passed, 26 skipped by filter
- full sweep after T8 runtime WebSocket dispatch partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 83 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This is assembled runtime WebSocket proof. CLI `serve` still needs to switch
  to mixed protocol dispatch before installed-Codex smoke can exercise the
  WebSocket path through the binary command.

Next implementation slice:
Switch CLI `serve` to `serve_protocol_connections`, add CLI-level WebSocket
serve proof, then build T10 installed-Codex mock smoke harness.

## Implementation-Execute T9 CLI Mixed WebSocket Serve Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T9 CLI mixed WebSocket serve partial result:

- added `tungstenite` as a CLI dev-dependency for real WebSocket serve proof
- `codex-router serve` now builds `LoopbackRouterRuntime` with an explicit
  `--max-websocket-upstream-messages` bound
- `codex-router serve` now calls `serve_protocol_connections`
- binary command path now accepts HTTP/SSE and WebSocket upgrade traffic
- CLI-level WebSocket test proves:
  - local WebSocket client connects to the CLI-started router
  - local router token is validated
  - router-owned SQLite/secrets drive account selection and upstream auth
  - local token is stripped from upstream handshake
  - selected upstream auth is injected
  - first `response.create` frame is preserved
  - upstream WebSocket response returns to local client

T9 CLI mixed WebSocket serve partial proof:

- RED: `cargo nextest run -p codex-router-cli serve_command_dispatches_websocket`
  failed because `--max-websocket-upstream-messages` was unknown
- GREEN: `cargo nextest run -p codex-router-cli serve_command_dispatches_websocket`:
  1 CLI WebSocket serve test passed, 14 skipped by filter
- full sweep after T9 CLI mixed WebSocket serve partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 84 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This is CLI/binary WebSocket serve path proof. Installed-Codex smoke, live
  OAuth/quota proof, implementation review, and PR readiness remain open.

Next implementation slice:
Build T10 installed-Codex mock smoke harness using the profile helper, token
export helper, and CLI `serve` against a mock upstream.

## Implementation-Execute T9 Token Export Command Wiring Partial Checkpoint

Current workflow:
`shravan-dev-workflow:implementation-execute-plan` in progress

T9 token export command wiring partial result:

- wired `token export` into the process-independent CLI command layer
- command requires explicit `--router-root`
- command loads current local router token through the hardened file secret
  store and `LocalRouterTokenService`
- command supports `--shell posix`
- success output is exactly one `CODEX_ROUTER_TOKEN=...` assignment with no
  surrounding prose
- command output is suitable for later installed-Codex smoke harness activation

T9 token export command wiring partial proof:

- RED: `cargo nextest run -p codex-router-cli token_export_command` failed
  because `token` was an unknown command
- GREEN: `cargo nextest run -p codex-router-cli token_export_command`: 2 token
  export command tests passed, 11 skipped by filter
- full sweep after T9 token export command wiring partial:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo nextest run --workspace`: 79 tests passed, 0 skipped
  - `cargo deny check`: pass
  - `cargo audit`: pass
  - `actionlint .github/workflows/ci.yml`: pass
  - forbidden-scope and dependency guard checks: pass

Scope note:

- This is CLI token export only. Installed-Codex smoke, server runtime assembly,
  real upstream WebSocket tunneling, live OAuth/quota proof, implementation
  review, and PR readiness remain open.

Next implementation slice:
Continue server/runtime assembly for installed-Codex mock smoke, or complete
real upstream WebSocket tunnel proof before T10.
