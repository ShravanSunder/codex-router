# Distributed agent collaboration: research and requirements

**Status:** research and requirements, 2026-10-04. This is not an implementation plan. It records what the owner has settled, the shape those decisions imply, what research established, the decisions still open, and how the work splits into lanes.

**Audience:** the owner's chief of staff, who assigns the lanes, and every Lead who picks one up.

**Companion files:**
- [work-tree.md](work-tree.md): the work as a tree in the board's own entity model, with its dependency DAG
- [research/architecture-options.md](research/architecture-options.md): central vs leased authority, the server model, SQLite replication tools
- [research/messages-and-automations.md](research/messages-and-automations.md): store-and-forward messages, automation rules, current code facts
- [research/sessions-and-log-shipping.md](research/sessions-and-log-shipping.md): what is known about moving and recovering agent sessions, and the open research

## 1. The problem

Agent work now runs on more than one machine: a laptop that sleeps, and an always-on machine. Today each machine runs its own Router, and the two Routers aren't linked:
- the shared board lives on one machine;
- direct messages to another machine's sessions are refused (`crates/collaboration-service/src/native_message_dispatch.rs:151`, `wrongService`);
- agents on the other machine reach the board through ad-hoc relays.

Agents need to coordinate across machines with the same guarantees they have on one machine: one record of who owns what, messages that arrive, and wakes that fire.

## 2. Owner requirements (settled, 2026-10-04)

| # | Requirement |
|---|---|
| R1 | A **fabric** exists: the owner's own always-on Rust service for cross-machine coordination, deployed somewhere. It holds **routing information and a registry**. It is a separate system from the per-machine Router. |
| R2 | The **workspace and board are server-first**: they live on the fabric. |
| R3 | **A project's Lead and its helpers run on one machine**, so consistency and collaboration within a project stay under local control. |
| R4 | **ADRs are immutable**, so they replicate freely. |
| R5 | **Local-first entities write locally, then replicate**: facts, outbox entries and session logs always; project records too if D1 = B (the owner's stated model). A machine taking over work fetches the latest first. Server-first entities (R2) are submitted to the fabric and wait as pending while it is unreachable. |
| R6 | Ownership moves by **leases**. |
| R7 | **Storage splits into interactions** (inbox, outbox, messages, approvals, questions) **and automations** (schedules, wakes, triggers). Automations act *through* interactions. |
| R8 | **Messages are local-first,** with an inbox and an outbox. |
| R9 | **Router owns the record shapes** (types in Rust); skills and repos supply content that is validated against them. |
| R10 | **Log shipping is a separate research track.** |
| R11 | **Work items can be spikes or builds.** |
| R12 | The board design is **fabric-compatible.** It works on one machine today and splits across the fabric without a rewrite. |
| R13 | **UUIDv7 for every identity that Router or the fabric mints:** machines, Router installations, registry rows, leases, records, messages, automations, runs, and stable session IDs. Provider-issued native session IDs and endpoint names stay as scoped identifiers that the registry maps from the stable UUIDv7 session ID (the existing contract in `crates/collaboration-protocol/src/endpoint_identity.rs:38–78`). Deduplication uses business keys alongside the UUIDv7 IDs, never another UUID version. |
| R14 | **Turso** is the storage and sync engine to build around. Spike M1.7 validates the fit and names what the fabric must add around it (see [research/architecture-options.md](research/architecture-options.md) §3). |

**A fact that shapes the design:** agents use online models, so no machine does agent work while fully offline. A sleeping machine runs no agents; an online machine can reach the fabric.

## 3. The shape these requirements imply

```text
┌─ FABRIC ─ owner's Rust service, always on ───────────────────────────┐
│ registry: machines · Routers · endpoints · sessions · capabilities   │
│ routing: target (session | role) → machine / endpoint                │
│ workspace + board (server-first) · project directory                │
│ leases: execution leases (+ project write leases if D1 = B)          │
│ message relay: holds messages while a machine sleeps                 │
│ automations: definitions · timers · run admission (if D2 = fabric)   │
└──────▲ requests up, records down ─────────────▲ relayed messages ───┘
       │                                         │
┌─ ROUTER on the laptop ──────────┐   ┌─ ROUTER on the always-on machine ┐
│ project P1: its Lead + helpers  │   │ project P2: its Lead + helpers   │
│ sessions run here (native logs) │   │                                  │
│ facts written here, shipped up: │   │                                  │
│   receipts · hook facts ·       │   │                                  │
│   owner words · read marks      │   │                                  │
│ inbox / outbox                  │   │ inbox / outbox                   │
└─────────────────────────────────┘   └──────────────────────────────────┘
```

### Placement rule (ADR-1)

- **Decisions are server-first.** These are shared things where something must decide between agents: projects, milestones, tasks, seats, status moves, ordering, leases.
- **Facts are local-first.** Each has one natural producer and is written where it happens, then shipped as an append: receipts, hook facts, owner words, read marks, session logs, outbox entries.
- **Local-only state never leaves the machine:** running turns and processes, credentials, checkouts (git syncs those), caches.

### Collaboration model

The server model in Matt Weidner's ["Architectures for Central Server Collaboration"](https://mattweidner.com/2024/06/04/server-architectures.html) fits:
- **Meaningful actions go up and records come down.** Agents send typed moves and posts; the authority sends back ordered records.
- **Moves with preconditions are rejected and retried** (seats, status, approvals). The agent re-reads and decides again; there is never a silent merge.
- **Appends can't conflict** (posts, log entries, receipts).
- **No CRDTs are needed.** With one authority they are an optimization, not a requirement.

## 4. Board-level ADRs

| ADR | Decision | Status |
|---|---|---|
| ADR-1 | Decisions server-first, facts local-first | accepted |
| ADR-2 | A project's Lead and helpers run on one machine | accepted |
| ADR-3 | The fabric is the owner's separate Rust service; the board is fabric-compatible | accepted |
| ADR-4 | Messages: outbox → fabric relay → inbox, with receipts at each stage | accepted |
| ADR-5 | Interactions are separate from automations; automations act through interactions | accepted |
| ADR-6 | Router owns record shapes | accepted |
| ADR-7 | Log shipping and recovery is a separate research track | accepted |
| ADR-8 | Who admits project writes (D1) | proposed |
| ADR-9 | Where automation timers live (D2) | proposed |

## 5. Open decisions

| D | Decision | Options | Recommendation | What waits |
|---|---|---|---|---|
| D1 | Who admits a project's writes | **A** central: the fabric admits every board write; machines hold read copies and pending requests. **B** leased local: the machine holding the project's write lease admits locally and replicates. **The owner's stated model is B** (projects local-first under a machine-held lease). | **The Lead and the Advisor recommend A** for now, with the board built so B can be added per project later. B's gain is accepting writes while the fabric is unreachable; it pays with fencing, handover and tail-loss rules. Turso's replica support (strong) vs Sync support (not GA) points the same way (research §3). Spike M1.0 informs this. **The owner decides.** | project write lease (P1), epoch fields (P2) |
| D2 | Where automation timers live | on the fabric (one clock, one admission owner), or per project under the project lease | **fabric.** No timer transplant on moves, one clock, and the automation store stays separate. | timer placement only; target semantics can proceed |
| D3 | A fixed-target automation whose session was recovered as a new conversation | deliver labelled, hold, or expire | — | P4 recovery rules |
| D4 | Storage-format changes | a per-project sequence; separate server-first and project stores | yes to both | P2 store split |
| D5 | Automations may target a role (seat), resolved by the board | yes / no | **yes** | P4 targeting |
| D6 | Owners for the fabric (P1), interactions (P3), automations (P4), recovery (P5) | — | — | those lanes |
| D7 | Turso adoption scope: the fabric only, project stores too, or every Router store | decide after spike M1.7 | the fabric only first; project stores if D1 = B | P1 storage, P2 store split |
| D8 | Where the fabric's code lives (its repository) | — | a new repository | P1 home |

## 6. Lanes

Each lane owns one project from [work-tree.md](work-tree.md) and hands off through named contracts. Two lanes don't edit the same protocol or storage seam.

| Lane | Owns | Starts now with | Waits on |
|---|---|---|---|
| **P1 Fabric control plane** | caller identity, registry, routing, execution leases, relay custody; the project write lease only if D1 = B | registry and identity contracts | P2's role-selector contract (for routing); D1 (project lease only) |
| **P2 Board** (board Lead) | board entities, role selectors, lifecycle facts, neutral record identity/ordering/change feed, notify-intent schema | replay cases, spec revision, role selectors, lifecycle facts | D1 (epoch variant), D4 (store split) |
| **P3 Interactions** | durable intent, inbox/outbox custody, dedup, ordering, staged receipts | the interaction-history → SQLite migration already in progress; a cutover spike for direct messages | P1 routing and identity (cross-machine relay) |
| **P4 Automations** | definitions, timers, run admission, wake/run intent | target-intent spike (role vs fixed) | P3 receipt semantics; D2 |
| **P5 Recovery + log shipping** | provider session recovery research; project/automation store shipping research | both research spikes | — (publishes constraints to P1 and P4) |

**Contract seams between lanes:**
- **Board → fabric:** role assignment (the board says who holds a role); the fabric resolves it to a live session and endpoint.
- **Board → interactions:** notify intents, linked to the record that caused them, with distinct receipts.
- **Automations → interactions:** wake and run intents.
- **Fabric → everyone:** authenticated caller identity; routing.
- **Recovery → fabric and automations:** session-binding and recovery-level constraints.

## 7. Assignment cards

One card per lane, each with the same seven fields. The chief of staff assigns an owner to each lane; the board Lead owns P2. Every Lead follows the repository's AGENTS.md (the release, SQLite and validation rules).

### P1 Fabric control plane
- **Owner role:** a Lead (Rust), with a Sol Sidekick for spikes.
- **Home:** the fabric's own repository (decision D8). Until D8 is made, its specs live in `docs/specs/<date>-fabric-control-plane/` in this repository.
- **First artifacts:** the spike M1.7 report (Turso fit); the spike M1.0 report (admission under outage); a spec for the registry and caller-identity contract (M1.1, M1.3).
- **Depends on:** P2's role-selector contract (M2.5) for M1.2 routing; D1, only for M1.6.
- **Write and proof boundary:** fabric code and specs only. It changes no Router store.
- **Done when:** a Router on each of two machines registers, authenticates, resolves a role to a live session, and holds an execution lease. Proved by an integration test across two machines.
- **Handoff evidence:** contract specs plus the test output, linked from the work thread.

### P2 Board (board Lead)
- **Owner role:** the board Lead.
- **Home:** `docs/specs/2026-10-04-board-orchestration-design/` (design), then `crates/message-board*` and `crates/collaboration-service` (board parts).
- **First artifacts:** the replay cases (M2.0); the spec revision (M2.1).
- **Depends on:** D1 for M2.2b; D4 for M2.4; P3's intent contract (M3.2) for M2.7.
- **Write and proof boundary:** board crates and board specs.
- **Done when:** these guarantees hold on one machine, each with a test.
  1. UUIDv7 record IDs that carry their scope.
  2. Per-project ordering.
  3. An admission context on project writes (expected revision, plus an epoch if D1 = B).
  4. A complete change feed, including read marks, with watermarks.
  5. Typed moves, rejected with a reason and never silently merged.
  6. Cross-project links as immutable IDs; `dependsOn` within a project.
  7. Separate server-first and project-scoped stores (D4).
  8. A schema version per store.
  9. Notify intents handed to interactions; the board never delivers.
  10. Every actor recorded with its machine and session.
  11. Role selectors with an assignment revision and resolution provenance.
  12. Typed lifecycle facts: close cutoff, design complete, dropped.
- **Handoff evidence:** the role-selector contract to P1; the notify-intent schema to P3; test output for each guarantee.

### P3 Interactions
- **Owner role:** the existing interaction-SQLite Lead continues M3.0. A Lead (it may be the same one) takes M3.1–M3.3.
- **Home:** `docs/specs/<date>-interactions/`; code in `crates/collaboration-service` (the interaction broker) and the interactions store crate the lane defines.
- **First artifacts:** the M3.1 cutover spike; the M3.2 intent-and-receipt contract.
- **Depends on:** P1 routing and identity (M1.2, M1.3) for M3.3.
- **Write and proof boundary:** the interactions store and service. It doesn't edit board schemas or automation timer state.
- **Done when:** a message crosses machines through outbox → relay → inbox, with dedup, per-pair order, expiry and staged receipts. Proved by tests, including a dropped-connection retry.
- **Handoff evidence:** the M3.2 contract (consumed by P2's M2.7 and P4's M4.4), plus test output.

### P4 Automations
- **Owner role:** a Lead.
- **Home:** `docs/specs/<date>-automations-cross-machine/`; code in `crates/automation-storage` and `crates/agent-automation`.
- **First artifacts:** the M4.2 target-intent spike; the M4.1 timer-home decision input for D2.
- **Depends on:** P3's M3.2 receipt contract; P2's M2.5 role selectors; D2 for M4.3.
- **Write and proof boundary:** the automation store and service. It doesn't edit interaction custody or board records.
- **Done when:** wakes and runs emit interactions; role and fixed targets behave per spec across a session move; there's no double or missed fire across a handover. Proved by tests.
- **Handoff evidence:** the target-intent spec, plus test output.

### P5 Recovery + log shipping (research)
- **Owner role:** a research Lead, with Sol Sidekicks.
- **Home:** `docs/specs/<date>-recovery-and-log-shipping/` (findings); experiment scratch under the lane's `tmp/`.
- **First artifacts:** the M5.1, M5.2 and M5.3 spike reports.
- **Depends on:** nothing. It is a soft input to P1 (session bindings) and P4 (run recovery).
- **Write and proof boundary:** research documents and experiments only. It changes no product code.
- **Done when:** each provider has a stated recovery level, with primary sources and an experiment result, and the options for shipping project and automation stores are compared.
- **Handoff evidence:** a constraints list published to P1 and P4.

### Cross-cutting
| Milestone | Owner | Home | Depends on | Done when / evidence |
|---|---|---|---|---|
| M-X1 compatibility | each lane, for the surfaces it changes | that lane's spec | the lane's own milestones | existing stores import or keep working; schema-version gates are tested |
| M-X2 CLI/MCP parity | each lane, for its commands | that lane's spec | the lane's contracts | CLI and MCP schemas are exported; parity tests pass |
| M-X3 observability | P1 (fabric) and P2 (board views) | their specs | P1, P2 | stale, unknown and expired states are visible in views; tests pass |
| M-X4 two-machine proof | an integration Lead the chief of staff assigns | `docs/specs/<date>-two-machine-proof/` | P1–P4 done; P5 is a soft input only | the scenario in work-tree.md M-X4 passes on two real machines; recorded run output |
| M-X5 release | the integration Lead | the repository release process (AGENTS.md) | M-X4 | the release is published and the install is verified |

## 8. What is not settled, and why it matters

- **Caller identity is now on the critical path.** On one machine, board actors are self-declared, and that's acceptable. A fabric that several machines call must know which machine and session is calling.
- **Lease expiry doesn't stop work already running.** An agent turn accepted before a lease moved can still finish its effects. Recovery reconciles unknown outcomes instead of resending.
- **Session portability is unproven.** Moving a running turn is supported by no provider. See [research/sessions-and-log-shipping.md](research/sessions-and-log-shipping.md).
