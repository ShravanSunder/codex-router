# Async Router Runtime Plan Review Packet

Date: 2026-06-24
Repo: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router`
Branch: `main`
HEAD: `01ee554`
Mode: read-only plan review. Do not implement. Do not edit files.

## Review Target

- Plan: `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
  - line count: 709
  - controller coverage: lines 1-709 read in four chunks
- Plan ledger: `tmp/plan-workflows/2026-06-24-async-router-runtime/plan-ledger.md`
  - line count: 112

## Accepted Source Artifacts

- Spec: `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
  - line count: 752
  - controller coverage: lines 1-752 read in four chunks
- Spec review ledger: `tmp/spec-workflows/2026-06-24-async-router-runtime/review-ledger.md`
  - line count: 240
  - controller coverage: lines 1-240 read
- Goal state:
  - `tmp/workflow-state/2026-06-24-async-router-runtime/details.md`
  - `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`

Independent read requirement:

- Open and read the implementation plan yourself.
- Open and read the accepted spec yourself.
- Use controller summaries only as routing hints, not truth.

## Review Question

Does the plan faithfully and executably implement the accepted async pure-proxy
runtime spec, including permanent proof that multiple concurrent installed Codex
WebSocket clients work through one `codex-router serve` PID without fallback,
stalls, leaks, or hand-rolled production protocol runtime?

## Required Source Anchors

- Required stack and no alternate production protocol owner: spec lines 62-124
- R1 async runtime ownership: spec lines 126-139
- R2 Hyper HTTP/SSE: spec lines 140-148
- R3 async WebSocket pure proxy: spec lines 150-199
- R4 no hidden buffering/backpressure rewrite: spec lines 200-208
- R5 SQLx state/auth boundary: spec lines 210-233
- R6 auth/header invariants: spec lines 235-253
- R7 session revocation: spec lines 254-265
- R8 observability/pump side effects: spec lines 267-287
- R9 proof expectations: spec lines 289-379
- Issue Closure Contract: spec lines 381-465
- Permanent Regression Guardrails: spec lines 467-532
- Boundary/separability map: spec lines 534-602
- Lifecycle contract: spec lines 604-643
- Non-goals: spec lines 645-665
- Security context: spec lines 701-722
- Acceptance gate: spec lines 729-752
- Spec review accepted findings: review ledger lines 20-240

## Plan Claims To Verify

- T0-T8 slices fully cover the source obligations.
- T1/T2 are serial and define a single production async `serve` path plus
  SQLx-owned async state/auth boundary before transport implementation.
- T3/T4 can proceed only after shared state/auth contracts settle.
- T5 owns WebSocket duplex pumps, registry, revocation, close semantics, and
  non-blocking pump-side side effects together.
- T6 includes both structural negative bans and positive release ownership
  checks across full release `serve` reachability.
- T7/T8 traverse real `codex-router serve` and installed Codex runtimes, not
  only in-process fixtures or mock clients.
- The proof matrix has one row per hard R9, Issue Closure, Permanent Guardrail,
  and Acceptance Gate obligation.

## Live Repo Evidence To Inspect

- `Cargo.toml`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/upstream.rs`
- `crates/codex-router-proxy/src/credential_runtime.rs`
- `crates/codex-router-auth/src/resolver.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`
- `.github/workflows/ci.yml`

## Security Context

Assets / privileges:

- local router auth token and generation
- upstream OAuth access/refresh tokens
- account ids and account labels
- quota snapshots and affinity records
- prompt/tool/message payloads flowing through Codex traffic

Entry points:

- `codex-router serve`
- local loopback HTTP/SSE
- local loopback WebSocket upgrade and frames
- SQLite state
- secret store
- installed-Codex smoke/e2e harnesses
- CI/repo-local validation commands

Untrusted inputs:

- local client HTTP requests and WebSocket frames
- upstream HTTP/WebSocket frames
- config/env/CLI arguments
- SQLite contents after previous runs
- secret-store contents
- test harness artifacts

Trust boundaries / auth assumptions:

- local Codex client to loopback router
- router to upstream provider
- proxy runtime to auth/state/secret-store
- logs/traces/audit/evidence artifact boundary

Security invariants:

- bind remains loopback-only
- unsupported routes fail closed before account selection
- local auth fails before upstream egress in token-required mode
- tokenless default remains tokenless loopback
- local auth is stripped before upstream
- upstream auth injection happens only after selection
- WebSocket first-frame parsing is bounded
- logs/evidence are allowlisted and redacted
- pumps do not await SQLite, secret-store, or durable audit persistence before
  forwarding or close progress

Security non-goals:

- OAuth/login/keychain redesign
- live-provider proof by default
- session picker/resume UX
- quota algorithm redesign

## Output Schema

Return:

- Lane name
- Backend used
- Verdict: ready, needs revision, or blocked
- Findings grouped as blocker, important, question, nit
- For each finding: evidence, failure scenario, smallest plan edit, proof/test,
  confidence
- For security findings: validation status as validated, unvalidated with proof
  gap, or rejected
- Coverage ledger when applicable:
  - source obligation
  - plan home/slice
  - proof row/checkpoint
  - status: covered, deferred with reason, missing, contradicted
- Completion receipt with source anchors, confidence, and remaining uncertainty

Do not mark findings accepted. Parent verifies and reduces candidates.
