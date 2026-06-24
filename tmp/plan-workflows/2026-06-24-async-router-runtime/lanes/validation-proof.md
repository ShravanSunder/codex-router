# Lane: validation-proof

Status: answered
Reasoning effort: high
Security context: applicable
Candidate evidence label: `candidate-validation-proof-async-runtime-2026-06-24`

## Source Coverage

- Spec rows: R9 proof expectations, Issue Closure Contract, Permanent
  Regression Guardrails, and Acceptance Gate.
- Review ledger rows: compound failure, real serve traversal, soak actor model,
  redacted artifact, guardrail scope, and pump-side side-effect findings.

## Matrix Shape Required

The plan must preserve one row per hard gate. Rows may not be collapsed into
generic tasks such as "add integration tests" or "run e2e".

Each row must include:

- row id
- source spec anchor
- owning task/slice
- proof layer
- fixture or harness
- command or execution surface
- expected observation
- durable evidence artifact
- stale-proof guard
- red/green requirement
- status checkbox

## Required Proof Families

Unit:

- route classification rejects unsupported HTTP/WebSocket paths before
  selection/upstream
- local-auth matrix covers tokenless default, explicit token-required mode,
  missing/wrong/old tokens, and auth-smuggling carriers
- first-frame validation covers malformed, oversized, unexpected, and
  auth-smuggled frames
- affinity metadata extraction and header sanitation preserve existing behavior

Integration:

- multiple concurrent WebSocket sessions progress independently
- one stalled upstream WebSocket while another WebSocket completes
- one stalled upstream WebSocket while HTTP/SSE completes
- same-session bidirectional interleave before `response.completed`
- compound close-while-pending regression reproduces old failure and proves
  async closure
- upstream-close/local-idle cleanup
- blocked write/backpressure cleanup in both directions
- post-upgrade/pre-upstream failure classes close locally with redacted outcome
- fragmented upgrade, unsupported route rejection, local-auth stripping,
  previous-response affinity, token-mode revocation, registry cleanup,
  credential-refresh cancellation, and pump-side side-effect non-blocking

Smoke:

- tokenless default installed-Codex smoke succeeds without
  `CODEX_ROUTER_TOKEN`
- explicit local-token hardening smoke rejects bad tokens and smuggling before
  upstream
- installed-Codex path traverses real `codex-router serve`

E2E:

- three independent installed Codex CLI processes/runtimes through one shared
  router PID
- long-running five-minute three-runtime overlap
- at least three post-handshake interactions or multi-frame exchanges per
  runtime during overlap
- at least one tool-call-style or equivalent multi-step interleave during
  overlap
- active-session high-water mark 3 and zero active after completion
- upstream and local socket cleanup with normal close reasons
- one redacted evidence artifact with required correlation fields and forbidden
  material checks

Structural / guardrail:

- no production `std::net`, `reqwest::blocking`, blocking tungstenite,
  production `httparse`, blocking `Box<dyn Read + Send>`, or direct proxy
  `rusqlite` on release `serve` reachability path
- positive ownership check: Hyper owns local HTTP, Hyper upgrade owns local
  WebSocket upgrade, Hyper client/body owns upstream HTTP/SSE,
  `tokio-tungstenite` owns WebSocket streams
- no helper/private alternate parser, handshake, frame runtime, legacy blocking
  runtime, hidden feature/env selector, or second production `serve`
- pump-side side-effect structural/behavioral guardrail
- CI/repo-local guardrail command before done claim

Acceptance / PR:

- plan-review must verify matrix fidelity before implementation
- implementation-review must verify proof and guardrails
- PR-ready wrapup must report fresh CI/check/review-thread/mergeability state
  without merging

## Existing Harnesses To Extend

- `crates/codex-router-proxy/src/lib.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`
- `.github/workflows/ci.yml`

## New Harnesses Required

- deterministic old-failure reproducer
- async close-while-pending fixture with registry/close-reason observation
- same-session interleave mock upstream
- blocked-write/backpressure cleanup harness
- credential refresh half-commit cancellation harness
- pump-side slow sink regression harness
- release-serve reachability/dependency structural checker
- three-installed-Codex concurrent soak orchestrator and redacted evidence
  artifact generator

## Evidence Requirements

Evidence artifacts must include row id, git HEAD, command/harness name, UTC
timestamp, touched binary/test target, pass/fail, expected observation summary,
and redaction checks.

They must not include raw prompts, tool arguments, response bodies, local
tokens, refresh tokens, provider payloads, account labels, or provider bodies.

## Split / Replan Triggers

- Split if any row tries to satisfy multiple mandatory gates.
- Split if old-failure reproduction is nondeterministic.
- Split if real-serve close-while-pending cannot observe registry cleanup and
  close reasons.
- Split if structural scanning cannot distinguish release-reachable code from
  test/dev-only helpers.
- Split if credential-refresh cancellation requires a new transaction/commit
  seam.
- Replan if proof requires live OAuth/provider traffic by default.

## Completion Receipt

Answered with proof rows grouped by unit, integration, smoke, e2e, structural,
and PR/acceptance gates.

Confidence: medium-high
