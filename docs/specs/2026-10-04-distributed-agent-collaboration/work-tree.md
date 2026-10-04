# Work tree: distributed agent collaboration

This tree is written in the board's own entity model (workspace › board › project › milestone › task, with `dependsOn` forming a DAG). Planning in the model is also a test of it. It was revised 2026-10-04 after an independent review of the first draft.

**Marks:**
- ✅ settled or done
- ⏳ in progress
- ○ not started
- ❓ gated by an owner decision (D-number, see the main file §5)

**Kinds.** Every milestone is a **spike** or a **build**:
- **spike:** time-boxed; its output is findings plus an ADR or decision input; done when its question is answered;
- **build:** ships code with proof.
Spikes are marked `[spike]`.

## The tree

```text
WORKSPACE  agent-collaboration
└─ BOARD  "Distributed agent collaboration"            kind: initiative
   goal:     agents on any machine coordinate through one fabric
   doneWhen: two projects, homed on different machines, each with its
             Lead + helpers co-located (R3), share one board; a reviewer
             from the other machine reviews one of them; board writes,
             messages and wakes work across machines (proved by M-X4)
   │
   ├─ ADRs: see the main file §4
   │
   ├─ P1  FABRIC CONTROL PLANE
   │   ├─ M1.0 ○ [spike] Admission under outage: accepted vs pending writes
   │   │        while the fabric is unreachable; stale-holder behaviour;
   │   │        fencing; recovery-point loss                      → D1
   │   ├─ M1.1 ○ Registry contract: machines, Routers, endpoints, sessions
   │   │        (stable session id → native binding), capabilities, freshness
   │   ├─ M1.3 ○ Caller identity: which machine and session is calling
   │   │        dependsOn M1.1
   │   ├─ M1.2 ○ Routing: role assignment / session → machine + endpoint
   │   │        dependsOn M1.3, P2.M2.5
   │   ├─ M1.4 ○ Execution leases (which machine runs a session)
   │   │        dependsOn M1.3
   │   ├─ M1.6 ❓ Project write leases + fencing          dependsOn D1 (= B only)
   │   ├─ M1.7 ○ [spike] Turso fit: self-hosted sync server; single-writer
   │   │        push fencing (last-push-wins otherwise); admission rules before
   │   │        writes; Rust API vs SQLx and migrations; maturity (pre-1.0) → D7
   │   └─ M1.5 ○ Fabric deployment + backup of its own store   dependsOn M1.7
   │
   ├─ P2  BOARD, fabric-compatible                       owner: board Lead
   │   ├─ M2.0 ⏳ [spike] Replay cases: past deliveries through the model
   │   ├─ M2.1 ○ Spec revision: entity model + fabric guarantees
   │   ├─ M2.2 ○ Neutral record identity, per-project ordering, change
   │   │        feed incl. acks/read marks, watermarks      dependsOn M2.1
   │   ├─ M2.2b ❓ Epoch / admission variant                dependsOn D1
   │   ├─ M2.4 ❓ Store split: server-first vs project-scoped
   │   │        dependsOn M2.1, D4
   │   ├─ M2.5 ○ Role selectors + assignment revision + resolution provenance
   │   │        dependsOn M2.1
   │   ├─ M2.6 ○ Lifecycle facts: close cutoff, design complete, dropped
   │   │        dependsOn M2.1
   │   └─ M2.7 ○ Notify intents (the board never delivers)
   │            dependsOn M2.1, P3.M3.2 (intent + receipt contract)
   │
   ├─ P3  INTERACTIONS
   │   ├─ M3.0 ⏳ interaction-history JSON → SQLite (authorized first step)
   │   ├─ M3.1 ○ [spike] Direct-message cutover: today's messages live in
   │   │        the automation store; ownership, handoff, retention, failure
   │   ├─ M3.2 ○ Intent + receipt contract: custody → durable accept →
   │   │        native accept → completion; dedup; per-pair order; expiry
   │   └─ M3.3 ○ Cross-machine store-and-forward through the fabric relay
   │            dependsOn M3.2, P1.M1.2, P1.M1.3
   │
   ├─ P4  AUTOMATIONS
   │   ├─ M4.1 ❓ [spike] Timer home                        → D2
   │   ├─ M4.2 ○ [spike] Target intent: role vs fixed; resolve before
   │   │        admission; reroute rules; dedup business key
   │   ├─ M4.3 ○ Timer placement build                     dependsOn D2
   │   └─ M4.4 ○ Wakes and runs emit interactions
   │            dependsOn M3.2, M4.2, P2.M2.5
   │
   ├─ P5  RECOVERY + LOG SHIPPING                        research track
   │   ├─ M5.1 ○ [spike] Native session recovery per provider: recovery
   │   │        level, capture cut, manifest, recovery-point/time goals
   │   ├─ M5.2 ○ [spike] Provider experiments: cross-host resume
   │   └─ M5.3 ○ [spike] Project + automation store shipping: what
   │            replicates, snapshot + change feed, restore readiness
   │        (all three run in parallel; they feed P1.M1.1 and P4 as constraints)
   │
   └─ CROSS-CUTTING
       ├─ M-X1 ○ Existing data + single-machine compatibility: import,
       │        schema/version gates, rollback, current Router unchanged
       ├─ M-X2 ○ CLI / MCP / API parity: schema export, renames, cutover
       ├─ M-X3 ○ Observability: stale views, watermarks, unknown effects,
       │        lease expiry, operator reconciliation
       ├─ M-X4 ○ Two-machine proof: two projects (Lead + helpers co-located
       │        on each machine); a cross-machine reviewer; remote board
       │        write, message, wake, sleep/outage, dedup
       └─ M-X5 ○ Release + cutover: deployment order, migrations, restore
```

## The spine (milestone DAG)

```text
 M2.1 ─► M2.2 ─► M2.2b ◄─ D1           M1.1 ─► M1.3 ─┬─► M1.2 ─► M3.3
   │                                                  ├─► M1.4
   ├─► M2.4 ◄─ D4                                     └─ (D1=B) ─► M1.6
   ├─► M2.5 ─────────────────────────────► M1.2, M4.4
   ├─► M2.6
   └─► M2.7 ◄─ M3.2 ─► M3.3, M4.4          M4.2 ─► M4.4 ;  D2 ─► M4.3
 M3.0 ⏳ ─ ─ (no edge to M3.1: the authorized step doesn't unblock cutover)
 M5.1 · M5.2 · M5.3  ─ ─►  soft inputs to M1.1 and P4
 P1 · P2 · P3 · P4 done ─► M-X4 (two-machine proof) ─► M-X5 (release)
 P5 ─ ─► soft input only (not a prerequisite for M-X4)
```

## Start now (no owner decision needed)

- **P1:** M1.0, M1.1, M1.3, M1.7
- **P2:** M2.0, M2.1, then M2.2, M2.5, M2.6
- **P3:** M3.0 (running), M3.1, M3.2
- **P4:** M4.2
- **P5:** M5.1, M5.2, M5.3

## Gated

- **D1:** M1.6 (project write leases), M2.2b (epoch variant)
- **D2:** M4.3 (timer placement)
- **D4:** M2.4 (store split)

## Entity-model additions this plan uses

- `kind: spike | build` on milestones and tasks.
- Every identity Router or the fabric mints is a UUIDv7 (R13).
- A spike's doneWhen is "question answered", plus a recorded finding: an ADR or a decision input.
