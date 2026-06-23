# Reset-Aware Quota Burn-Down Routing Implementation Plan

Date: 2026-06-23
Status: draft implementation plan from corrected spec
Goal id: 2026-06-23-quota-burndown-routing

## Source Coverage

Source spec:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`

Coverage:
1174 lines. The plan preserves the corrected product decision:

- burn-down scoring is the center of the work
- 5h and weekly quota windows drive classification, scoring, routing, and status
- `probe_required` is not fallback capacity
- unknown/no-data/missing-reset accounts never enter weighted routing
- startup and request routing never block on live provider quota refresh or probe
- background probe/refresh may later persist verified selector rows to SQLite
- JSON is a debug/proof surface, not the product center

## Goal

Implement reset-aware quota burn-down selection and quota status so codex-router
can route real Codex traffic using last-known persisted quota, explain the
decision in a concise table, keep unknown accounts out of normal routing until a
background probe proves them usable, and prove the whole path through installed
Codex HTTP/SSE and WebSocket traffic.

## Non-Goals

- Do not change OAuth/keychain beyond what the existing credential/probe path
  requires.
- Do not add request-path live quota polling or probe calls.
- Do not teach `WeightedDeficitSelector` quota semantics.
- Do not merge the PR as part of this plan.

## Execution DAG

```text
gate 0: verify repo/source state
  |
  +-- lane A: pure burn-down assessment
  |     write: crates/codex-router-selection/src/burn_down.rs
  |            crates/codex-router-selection/src/lib.rs
  |     proof: unit tests for math, collapse, probe_required, candidate order
  |
  +-- lane B: proxy adapter and runtime routing
  |     write: crates/codex-router-proxy/src/account_selection.rs
  |            crates/codex-router-proxy/src/http_sse.rs
  |            crates/codex-router-proxy/src/websocket.rs
  |     proof: integration tests for persisted rows, no request-path probe,
  |            fail-fast no verified usable account, WebSocket pinning
  |
  +-- lane C: quota status UX
  |     write: crates/codex-router-cli/src/quota.rs
  |            crates/codex-router-cli/Cargo.toml if selection dependency needed
  |     proof: golden tests for table/plain/json, Unicode bars, no account id,
  |            no pp/bottleneck, needs probe rows
  |
  +-- lane D: background probe/refresh persistence
        write: crates/codex-router-cli/src/quota.rs
               crates/codex-router-state/src/quota_snapshot.rs if status enums
               need extension
        proof: refresh worker/probe tests prove persisted success promotes
               later routing and failures remain probe_required

integration gate: parent reviews combined selection/status/probe contract
  |
validation gate: cargo fmt, cargo test targeted packages
  |
smoke gate: codex-router serve with persisted fixtures and mock upstream
  |
e2e gate: installed Codex through router over HTTP/SSE and WebSocket
  |
implementation-review-swarm
  |
PR update/readiness proof, no merge
```

Parallelization:
Lanes A and C can start after gate 0 because status can initially consume the
new DTO shape from fixtures. Lanes B and D depend on A's DTO names and collapse
semantics. E2E depends on all implementation lanes.

## Tasks

### T0. Repo and Source Gate

Read and record:

- current git branch, head, remote state, and dirty files
- spec line count and current `probe_required` wording
- existing selector/status/WebSocket/test-support anchors

Do not stage unrelated dirty implementation files from earlier work unless they
are intentionally adopted by a task below.

Proof:

- `git status --short --branch`
- `wc -l` for spec and plan
- `rg` proves no current spec wording says unknown all-account fallback is
  routable

### T1. Pure Burn-Down Assessment

Implement `codex-router-selection::burn_down` with pure DTOs and no dependency
on state, proxy, CLI, or secret stores.

Required behavior:

- classify per-account availability:
  `usable | reserve | blocked | probe_required | excluded`
- compute 5h/weekly pressure, surplus, salvage, risk penalty, limiting window
- apply weekly pressure before 5h reset urgency
- apply bounded 5h/weekly near-reset salvage
- make missing/no/unknown quota `probe_required`
- never compute routing weight for `probe_required`
- return selected pool `usable | reserve | none`
- return weighted candidates only for selected usable/reserve pool
- return neutral `preferred_next` from empty weighted-deficit state and
  deterministic candidate order

Proof:

- unit tests for scenarios A-F from the spec
- unit tests for stale penalty, missing reset, missing expected 5h/weekly,
  exhausted/ineligible, no effective marker, salvage tie key, deterministic
  order
- negative test: `probe_required` is absent from weighted candidates

### T2. Proxy Runtime Selection

Replace minimum-headroom-only selection with the burn-down route-band
assessment.

Required behavior:

- adapt `SelectorQuotaInput` from SQLite into burn-down DTOs
- include account enabled and active credential generation facts
- preserve previous-response affinity fail-closed behavior
- feed only `weighted_candidates` into `WeightedDeficitSelector`
- fail fast with audit-safe no-verified-account error when selected pool is
  `none`
- schedule or signal background probe for probe-required accounts without
  blocking the request
- do not call provider quota/probe or credential refresh as part of selecting an
  unknown account

Proof:

- integration tests with persisted 5h/weekly selector windows
- no verified usable account test proves zero provider quota/probe calls before
  failure
- route selection test proves weekly-danger account is held in reserve while a
  5h-near-reset/weekly-healthy account is used
- affinity owner hit and owner failure tests still pass

### T3. WebSocket Routing Contract

Align WebSocket selection with the same route-band burn-down assessment.

Required behavior:

- local auth and unsupported path fail before selection
- `/v1/responses` path fixes route band to `responses`
- first frame parsing before selection reads only top-level `type` and
  top-level `previous_response_id`
- selected account is pinned for connection lifetime
- no selected account change mid-stream
- no live quota probe on first frame or before upstream open

Proof:

- WebSocket unit/integration tests for local auth failure, unsupported path,
  wrong first-frame type, malformed affinity, owner-resolution failure
- canary test proves non-allowlisted first-frame fields do not influence
  selection/logging before upstream validation
- pinning test proves one selected account for the connection

### T4. Quota Status UX

Make `codex-router quota status` consume the same burn-down assessment output.

Required behavior:

- default table is account-centric
- one logical row per account with 5h and weekly columns
- Unicode bars in table mode; ASCII bars in plain mode
- no default account id, raw score, `pp`, or `bottleneck`
- show `preferred`, `available`, `held`, `blocked`, or `needs probe`
- display `probe_required` rows as not usable, never fallback
- JSON exposes enough debug/proof fields to reconstruct the table and selection
  reasoning, while remaining secondary to the human table

Proof:

- table/plain/json golden tests
- negative assertions for `account_id`, `pp`, `bottleneck`, raw score, tokens,
  auth headers, and unsafe labels in human output
- JSON schema/field tests for stable enums and probe-required state

### T5. Background Probe/Refresh Persistence

Use the existing background quota refresh worker/provided provider interface as
the probe mechanism unless implementation discovery proves a narrower adapter is
needed.

Required behavior:

- startup can trigger background refresh/probe promptly
- periodic refresh/probe persists verified quota windows to SQLite
- failed probe records safe error/status without making account routable
- request path never waits for probe result
- later request uses newly persisted selector rows if the background probe
  succeeded

Proof:

- fake provider tests for success, provider failure, auth failure, parse failure
- non-blocking tests for server boot/listen, first routed request, and quota
  status render while provider is delayed/failing
- persistence test proves successful probe promotes later routing

### T6. Installed Codex E2E

Extend `codex-router-test-support` installed Codex smoke to force a reset-aware
choice and prove both transports.

Required behavior:

- generated codex-router profile
- served local router and mock upstream
- multiple persisted accounts with 5h/weekly rows
- HTTP/SSE and WebSocket both exercised
- status output, selected safe label/hash, routing reason, and WebSocket pinning
  agree
- transcripts are redacted

Proof:

- installed Codex smoke command exits 0
- mock upstream transcript shows selected account and no local router token leak
- WebSocket transport does not fall back silently to HTTP-only proof

## Requirements / Proof Matrix

| Requirement | Source | Task | Proof layer | Evidence |
| --- | --- | --- | --- | --- |
| Burn-down scoring protects weekly quota and salvages near-reset quota | spec R4/R5, scenarios A-D | T1 | unit | assessment tests |
| Unknown/no quota is probe-required, not fallback | spec R3, scenario E/F | T1/T2/T5 | unit + integration | no weighted candidate, fail-fast, background probe persistence |
| Startup/request path never blocks on quota probe | spec R1 | T2/T5 | integration + smoke | delayed provider tests, boot/listen smoke |
| Runtime and status share assessment semantics | spec R6 | T1/T4 | unit + golden | same DTO/output fixture tests |
| Human status is useful and concise | status contract | T4 | golden + smoke | table/plain output snapshots and negative assertions |
| WebSocket supports Codex `/v1/responses` with pinning | WebSocket contract | T3/T6 | integration + e2e | WebSocket tests and installed Codex smoke |
| Secrets and unsafe identifiers do not leak | security context | T4/T6 | unit + smoke | redaction canaries and transcript inspection |
| PR is ready but not merged | goal terminal | PR wrapup | PR gate | checks, review threads, mergeability reported |

Red/green requirement:

- T1 through T5 should add or update failing tests first where practical.
- T6 can be written as a smoke/e2e harness extension and then made green.

## Validation Commands

Exact command list may be refined after implementation, but the executor must
at minimum run:

```text
cargo fmt --all -- --check
cargo test -p codex-router-selection
cargo test -p codex-router-proxy
cargo test -p codex-router-cli
cargo test -p codex-router-state
cargo test -p codex-router-test-support
cargo test --workspace
```

Smoke/e2e proof must include the installed Codex command used by
`codex-router-test-support` or an equivalent repo-owned harness command, with
HTTP/SSE and WebSocket evidence captured.

## Split / Replan Triggers

Return to planning before coding further if:

- implementing background probe requires new durable state beyond selector
  quota windows and refresh status
- WebSocket first-frame parsing cannot satisfy the allowlist without changing
  tunnel architecture
- installed Codex cannot be forced to exercise WebSocket in the local harness
- the current dirty worktree contains conflicting unrelated edits in the same
  files that cannot be safely adopted

## Security Context

Sensitive surfaces:

- OAuth access/refresh tokens
- router bearer token
- upstream auth headers
- account id and safe labels
- request/response bodies and WebSocket first frames
- logs/traces/smoke transcripts

Security plan:

- burn-down assessment receives no tokens
- request-path unknown probe is forbidden
- logs/status/transcripts emit safe labels/reason enums only
- WebSocket preselection failures prove zero credential resolution and zero
  upstream open

## Recommended Next Skill

Run `shravan-dev-workflow:plan-review-swarm` against this plan and the corrected
spec. If review has no accepted blockers, route to
`shravan-dev-workflow:implementation-execute-plan`.

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`
recommended_next_workflow: `shravan-dev-workflow:plan-review-swarm`
recommended_transition_reason: Plan maps the corrected burn-down/probe-required spec into implementation tasks and proof gates.
