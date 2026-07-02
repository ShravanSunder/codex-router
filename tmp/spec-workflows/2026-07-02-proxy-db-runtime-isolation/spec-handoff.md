# Spec Handoff: Proxy DB Runtime Isolation

Date: 2026-07-02
Audience: planning agent using `shravan-dev-workflow:plan-creation-swarm`
Worktree: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.impl-cli-dx-iocraft`
Branch: `impl-cli-dx-iocraft`

## Handoff Objective

Turn the accepted draft spec into an implementation plan. Do not implement code in the planning pass.

Primary source spec:

- `docs/specs/2026-07-02-proxy-db-runtime-isolation.md`

Supporting ledger:

- `tmp/spec-workflows/2026-07-02-proxy-db-runtime-isolation/swarm-ledger.md`

## Current Workspace Warning

The iocraft worktree is dirty with pre-existing implementation edits and untracked files. Treat those as current branch evidence, not as validated final implementation.

Observed dirty files at handoff creation:

- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/presentation/mod.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/sessions.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-state/src/lib.rs`
- `crates/codex-router-state/src/selection_projection.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `prototypes/ux_prototype/src/main.rs`
- `crates/codex-router-cli/src/presentation/quota.rs`
- `docs/specs/2026-07-02-proxy-db-runtime-isolation.md`
- `scripts/sync-debug-sqlite.sh`

Important boundary: do not modify `main`. The correct worktree is the iocraft branch above.

## Decisions Already Made

- `ProxyRuntime` owns socket lifecycle, local auth, WebSocket revocation, process-local active reservations, account holds, weighted selector state, and immediate client-facing decisions.
- Runtime active-load truth is process-local. SQLite active lease rows are durable mirror/history/proof, not live routing authority.
- Selection must preserve one atomic logical boundary: read snapshot, overlay process-local reservations, assess candidate accounts, acquire reservation for the winner.
- Selection read models must be pure and bounded. No stale cleanup, event append, rollup refresh, quota refresh, schema creation, or migration from selection admission.
- SQLite writes belong behind explicit write-side owners: `DbWriteActor`, `MaintenanceActor`, or a credential-specific policy boundary.
- WebSocket quota exhaustion must synchronously retire/exclude the exhausted account in runtime memory before any reconnect signal leaves the router.
- Credential refresh is a separate policy boundary. Generic fire-and-forget actorization of credential generation activation is not allowed by the spec.
- CLI/session/quota read surfaces must not write production SQLite by accident. Dev testing should use repo-local copied SQLite under `tmp/dev-state`.
- Detached Hyper loopback connection diagnostics must preserve a scrubbed source-chain/root-cause class. The current `failed serving Hyper loopback connection` wrapper is insufficient.

## Non-Goals

- Do not prescribe exact Tokio runtime/thread topology in the spec-to-plan handoff.
- Do not broaden WebSocket payload inspection.
- Do not weaken credential refresh generation atomicity.
- Do not introduce `rusqlite` for codex-router-owned production state.
- Do not make CLI mirror data the live routing source of truth.
- Do not hide stale/unavailable state from operator surfaces.
- Do not turn this handoff into an implementation plan; the receiving agent should produce the plan.

## Source Evidence To Read Before Planning

Required spec artifacts:

- `docs/specs/2026-07-02-proxy-db-runtime-isolation.md`
- `tmp/spec-workflows/2026-07-02-proxy-db-runtime-isolation/swarm-ledger.md`

Required code anchors:

- `crates/codex-router-proxy/src/server.rs`
  - Runtime assembly and detached Hyper connection diagnostics.
  - Current long-lived writable/read-only store seam.
- `crates/codex-router-proxy/src/account_selection.rs`
  - Selection lock, process-local reservations, active lease mirror reporter.
- `crates/codex-router-proxy/src/websocket.rs`
  - WebSocket first-frame routing, active reservation lifecycle, quota exhaustion reconnect/all-exhausted path.
- `crates/codex-router-state/src/selection_projection.rs`
  - Read-only projection split, maintenance-capable trait surface, current failure defaults.
- `crates/codex-router-state/src/sqlite.rs`
  - Writable open/migrate/schema ensure, read-only open/query-only mode, active-client count read/write variants, rollup refresh.
- `crates/codex-router-auth/src/resolver.rs`
  - Credential read/refresh/secret write/generation activation boundary.
- `docs/specs/2026-06-26-quota-routing-safety-spec.md`
  - Existing active-load source-of-truth and storage/telemetry/security constraints.
- `docs/specs/2026-06-27-account-quota-burn-rate-selection.md`
  - Existing selector, WebSocket containment, SQLx state-domain, and operator/telemetry contracts.

## Open Decisions The Plan Must Resolve

1. Where does authoritative in-memory exhaustion/quarantine state live?
2. What exact freshness ceiling is acceptable for last-known-good snapshots?
3. What is the fail-closed behavior for each stale/unavailable input class?
4. Which provider-error durable writes require acknowledgement before a router-owned safety signal?
5. Should credential refresh remain synchronous at egress, move to a prewarmed `CredentialRuntime`, or fail fast when cached credentials are expired?
6. What queue overflow policies apply to each write acknowledgement class?
7. How will Victoria/OTEL prove no proxy/socket stalls behind DB maintenance?
8. Which Hyper loopback error classes should be downgraded to debug/noise versus warning/error, and what source-chain detail is safe to log?

## Proof Expectations To Preserve

The plan must include proof gates for:

- read-only projection not invoking maintenance writes;
- snapshot failure and degraded-mode policy;
- concurrent assess/snapshot/reserve atomicity;
- read-only SQLite opens not creating, migrating, or requesting write locks;
- writable maintenance APIs not reachable from selection admission;
- WebSocket quota exhaustion not waiting on stale cleanup or rollup refresh;
- WebSocket quota exhaustion not reconnecting to the same exhausted account;
- loopback Hyper failures preserving scrubbed root-cause class;
- reservation cleanup and active-client mirror release still running on failed connections;
- credential refresh generation activation atomicity;
- installed/mock Codex WebSocket concurrency;
- Victoria/OTEL queue lag, degraded mode, and negative canary proof;
- dev-state proof that debug workflows use copied SQLite, not production state.

## Planning Slices From Spec

The receiving planning agent should convert these into a sequence with tests and review gates:

- Slice A: enforce read-only selection and remove maintenance from admission.
- Slice B: runtime in-memory quota exhaustion quarantine before WebSocket reconnect.
- Slice C: write-side actor interfaces and queue health telemetry.
- Slice D: credential runtime policy decision and proof.
- Slice E: dev-state SQLite copy workflow for performance and TUI/session/quota testing.
- Slice F: installed/mock Codex WebSocket and Victoria proof harness.
- Slice G: scrubbed Hyper loopback failure diagnostics and cleanup proof.

## Exact Next Task

Use `shravan-dev-workflow:plan-creation-swarm` to create an implementation plan from `docs/specs/2026-07-02-proxy-db-runtime-isolation.md`.

The plan should be grounded in the current iocraft branch, explicitly account for the dirty worktree, and preserve the spec's separation between product requirements, runtime/DB ownership boundaries, security controls, and proof gates.

