# Copy-Paste Prompt For Planning Agent

Use `shravan-dev-workflow:plan-creation-swarm`.

You are planning implementation only. Do not edit code. Do not commit. Do not modify `main`.

Worktree:

`/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.impl-cli-dx-iocraft`

Branch:

`impl-cli-dx-iocraft`

Objective:

Create an implementation plan for the Proxy DB Runtime Isolation spec. The goal is to keep `codex-router serve` proxy/WebSocket behavior responsive and correct when SQLite is slow, locked, or doing maintenance, while preserving security and routing contracts.

Primary spec:

`docs/specs/2026-07-02-proxy-db-runtime-isolation.md`

Supporting handoff:

`tmp/spec-workflows/2026-07-02-proxy-db-runtime-isolation/spec-handoff.md`

Supporting swarm ledger:

`tmp/spec-workflows/2026-07-02-proxy-db-runtime-isolation/swarm-ledger.md`

Before planning:

1. Read the entire primary spec.
2. Read the handoff and ledger.
3. Inspect current git status. The worktree is expected to be dirty; treat existing edits as branch evidence, not validated final implementation.
4. Read the required code anchors:
   - `crates/codex-router-proxy/src/server.rs`
   - `crates/codex-router-proxy/src/account_selection.rs`
   - `crates/codex-router-proxy/src/websocket.rs`
   - `crates/codex-router-state/src/selection_projection.rs`
   - `crates/codex-router-state/src/sqlite.rs`
   - `crates/codex-router-auth/src/resolver.rs`
   - `docs/specs/2026-06-26-quota-routing-safety-spec.md`
   - `docs/specs/2026-06-27-account-quota-burn-rate-selection.md`

The plan must preserve these decisions:

- `ProxyRuntime` owns sockets, local auth, WebSocket revocation, process-local reservations, account holds, weighted selector state, and immediate client-facing decisions.
- Runtime active-load truth is process-local; SQLite active leases are mirror/history/proof.
- Selection admission must not call SQLite open, migrations, schema ensures, stale cleanup, rollup refresh, or large event scans.
- Selection must preserve atomic assess/snapshot/reserve behavior.
- WebSocket quota exhaustion must synchronously retire/exclude the exhausted account in runtime memory before reconnect/all-exhausted/state-unavailable leaves the router.
- Credential refresh is a separate policy boundary and cannot be generic fire-and-forget persistence.
- CLI/session/quota read surfaces must use read-only/debug SQLite unless explicitly running a writer.
- Detached Hyper loopback connection failures must log a scrubbed source-chain/root-cause class, because the current `failed serving Hyper loopback connection` wrapper hides the real cause.

The plan must resolve or explicitly defer these open decisions:

1. Where authoritative in-memory exhaustion/quarantine state lives.
2. Snapshot freshness ceilings and fail-closed behavior.
3. Provider-error write acknowledgement policy.
4. Credential runtime policy.
5. Queue overflow policy by write class.
6. Victoria/OTEL proof for socket responsiveness under DB pressure.
7. Hyper loopback error classification and safe log detail.

The plan must include proof gates for:

- read-only projection not invoking maintenance writes;
- snapshot degraded-mode behavior;
- concurrent assess/snapshot/reserve atomicity;
- read-only SQLite not creating/migrating/requesting write locks;
- no selection admission reachability to maintenance APIs;
- WebSocket quota exhaustion not waiting on stale cleanup/rollup refresh;
- no reconnect to the same exhausted account;
- Hyper loopback failures preserving scrubbed root-cause class;
- reservation cleanup on failed connections;
- credential generation activation atomicity;
- installed/mock Codex WebSocket concurrency;
- Victoria/OTEL queue lag/degraded-mode/negative-canary proof;
- repo-local debug SQLite copy workflow, not production state.

Deliverable:

Write a plan artifact under `tmp/spec-workflows/2026-07-02-proxy-db-runtime-isolation/` or the repo's established planning directory. Include a requirement-to-task-to-proof traceability table. Do not implement.

