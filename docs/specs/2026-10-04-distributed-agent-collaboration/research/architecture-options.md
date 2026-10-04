# Architecture options: authority, replication and storage

Research for [the main file](../2026-10-04-distributed-agent-collaboration.md), 2026-10-04.

**Status labels:**
- **verified:** read in current source, with a repo-relative path and line, or in vendor docs retrieved 2026-10-04;
- **inference:** reasoning that hasn't been tested.

## 1. Two questions that look like one

Moving work between machines involves two different things, and they must not be coupled:

| | Agent session (execution) | Board records (authority) |
|---|---|---|
| What moves | a running conversation and its native logs | projects, tasks, seats, posts, moves |
| Moved by | an execution lease; log shipping or resume | admission (central) or a write lease (local) |
| Owner question | "which machine runs this agent now?" | "who may write this record now?" |

Moving a session must never move project authority as a side effect (inference, consistent with every recovery source reviewed).

## 2. Who admits a project's writes (decision D1)

```text
 A  CENTRAL ADMISSION                        B  LEASED LOCAL PROJECT
 the fabric checks every write               the lease-holding machine checks writes
 machines: read copy + pending requests      fabric: lease registry + replicas
 fabric down ─► writes wait as pending       fabric down ─► project keeps accepting
 no handover, no tail loss                   needs fencing, handover, tail-loss rules
       both keep a project's Lead + helpers on one machine (R3)
```

### What each option costs (inference)

| Concern | A: central | B: leased local |
|---|---|---|
| Writes during a fabric outage | wait as pending | accepted locally |
| Latency per board write | one network round trip | local |
| Rules spanning projects (workspace members, board ADRs, cross-project links) | one transaction | must be checked eventually, or by the fabric |
| Handover | none | drain, replicate, release; or a takeover with a known gap |
| Holder sleeps with unreplicated writes | can't happen (nothing is accepted locally) | those writes already *succeeded*; replaying them later as requests can reject work that others built on, so the gap must be explicit |
| Stale holder writes after a takeover | can't happen | needs epoch fencing at every write |
| Board views (inbox, owner queue) | one source | aggregated across homes, with "as of" watermarks |

### Failure modes for B (inference)

1. **Remote participants stall when the holder sleeps.** Reviewers, the owner and automations aren't in a project's local batch.
2. **The board's views go stale** across projects homed on different machines.
3. **Schema-version skew** across project files written by different Router versions.
4. **Cross-project links** can't be checked in one transaction.
5. **A laptop lost before shipping** loses everything since the last replicated record.
6. **A stale holder pushes over its successor** under last-push-wins sync.
7. **The lease registry is unreachable,** so renewals fail and admission must stop at expiry.
8. **Read marks don't replicate.** Today acknowledgments change bookmarks without producing activity (`crates/message-board-storage/src/inbox_records.rs:216–257`, verified).

**Recommendation:** A first, with the board built so B can be added per project (inference). The board's neutral contracts (identity, ordering, change feed) serve both options. Spike M1.0 tests admission and outage behaviour before D1 is decided.

## 3. Turso (R14)

**What Turso offers, verified in its docs on 2026-10-04:**
- **Embedded replicas (libSQL):** reads are local; by default writes go to the remote primary, then reflect back to the local replica (read-your-writes). An optional offline mode allows local writes. https://docs.turso.tech/features/embedded-replicas/introduction
- **Turso Sync (the new Turso engine):** reads and writes are local, synced explicitly with `push()` / `pull()`. The wire format is logical change-data-capture. A pull applies remote changes, then replays changes not yet pushed. https://docs.turso.tech/sync/usage
- **Conflicts are "last push wins".** https://docs.turso.tech/sync/conflict-resolution
- **A local sync server** (`tursodb --sync-server`) is documented for testing. https://docs.turso.tech/sdk/rust/quickstart
- **The new engine supports concurrent writes (MVCC).** https://docs.turso.tech/sdk/ts/reference
- **Rust access is the `turso` crate** (and `libsql` for embedded replicas). Each has its own API, not SQLx. https://docs.turso.tech/sdk/rust/quickstart
- **Maturity:** the Turso engine is pre-1.0. https://github.com/tursodatabase/turso

### How Turso maps onto the shape (inference)

| Store | Turso feature | What the fabric or Router must add |
|---|---|---|
| Workspace + board (server-first) | a Turso database on the fabric; machines keep local read replicas | every write goes through the fabric's admission code (Rust rules), never a direct client write |
| Project store, if D1 = B | one Turso database per project, written by the lease holder, pushed to the fabric; other machines pull | **single-writer fencing:** the fabric must reject a push from anyone not holding the current lease epoch, or last-push-wins lets a stale holder overwrite its successor |
| Automations (if timers live on the fabric) | a Turso database on the fabric | run admission stays in Rust |
| Interactions (local inbox/outbox) | local Turso databases; the relay carries messages, not database pages | dedup and receipts are application logic |

### Track record, 2026-08-04 → 2026-10-04 (researched 2026-10-04; issue links are dated by their filing date)

**What shipped (verified):**
- Turso 0.8.0 and 0.8.1 released 2026-09-29. 0.8.2 is in prerelease. The engine is still pre-1.0. https://github.com/tursodatabase/turso/releases
- 0.8.0 shipped sync fixes and stopped calling MVCC experimental (https://github.com/tursodatabase/turso/releases/tag/v0.8.0). The engine overview page still lists MVCC as experimental (https://docs.turso.tech/tursodb, retrieved 2026-10-04).
- **Support levels differ:**
  - embedded replicas (libSQL) are "fully supported in production";
  - no GA declaration was found for Turso Sync;
  - offline writes were announced as a public beta (2025-03-31), and no graduation was found.

**Sync correctness reports (verified):**
- MVCC replay put values into the wrong columns after a column was dropped or renamed: https://github.com/tursodatabase/turso/issues/9415 (2026-09-30). The fix https://github.com/tursodatabase/turso/pull/9419 merged 2026-10-01; a merge isn't proof the fix is deployed to Turso Cloud.
- **Still open: https://github.com/tursodatabase/turso/issues/9430 (2026-09-30).** Sync matches columns **by name only**, so a remote column swap leaves the local copy permanently out of sync. The author couldn't reproduce it on the local sync server, because it doesn't behave exactly like Turso Cloud.
- **Also open:**
  - pull fails with indexed generated columns: https://github.com/tursodatabase/turso/issues/9426 (2026-09-30);
  - TEMP-table schema leakage: https://github.com/tursodatabase/turso/issues/9387 (2026-09-27);
  - repeated pull failure when append-only triggers replay identical rows: https://github.com/tursodatabase/turso/issues/9376 (2026-09-26).

**Operations (verified):**
- A self-hosted sqld user reported an hour-long write stall in production: https://github.com/tursodatabase/libsql/issues/2286 (2026-10-03).
- A Turso Cloud API incident on 2026-08-04 lasted about 50 minutes. https://status.turso.tech/incidents
- The local sync server is documented for **development and testing**, without auth. https://docs.turso.tech/sync/local-sync-server

**Trajectory (verified):** Turso is joining Supabase (announced 2026-10-02). The platform continues, with stated commitments to open source. https://turso.tech/blog/turso-is-joining-supabase

**Community signal:** thin. X recent search covers only 7 days and found no first-hand production reports of offline sync. A Grok-based X search couldn't run in this pass.

**Implications for us (inference):**
- **Embedded replicas** (central primary, local reads) have the strongest support statement. They match decision A: the board on the fabric, with local read copies.
- **Turso Sync** (local writes, push/pull) is newer and still producing divergence bugs, especially around **schema changes** (DDL). Our stores migrate their schemas, so DDL replication is a direct risk for option B's project files.
- **A self-hosted sync server** isn't production-supported yet. A fabric that self-hosts sync would need its own proof, or Turso Cloud as the primary.

### What spike M1.7 must answer

1. **Self-hosting.** Can the fabric run its own sync server for production use, not only for testing?
2. **Single-writer fencing.** Where is push rejected for a stale epoch: at the sync server, or behind a fabric gateway?
3. **Admission.** How do Rust admission rules wrap writes when sync replays changes logically?
4. **SQLx.** Router uses SQLx with checked queries, migrations and exact schema validation (`crates/message-board-storage/src/board_schema_migrations.rs`, verified). Moving a store to Turso means a new storage layer and a new migration story. Which stores move, and when (decision D7)?
5. **Maturity.** What does pre-1.0 mean for operating the fabric?

## 4. Other SQLite replication tools (docs and repos checked 2026-10-04)

| Tool | What it is | Fit |
|---|---|---|
| LiteFS | lease-elected primary, writes forwarded, async replication; one lease per cluster; Linux FUSE; beta | closest topology to B, but Linux only and cluster-wide; maintenance risk (maintainer statement, 2024-09-09) |
| Litestream | streams the SQLite WAL to object storage; disaster recovery; one replica per database in v0.5 | backup only, no leases. Needs WAL mode; Router's stores use DELETE journaling (`crates/message-board-storage/src/board_connection.rs:29`, verified) |
| cr-sqlite | CRDT multi-writer merging | wrong model: we need one writer, not merges; restricts foreign keys and uniqueness |
| rqlite / dqlite | Raft clusters with one leader | needs a quorum; two machines can't form one well; rewrites transactions |
| Durable Objects | a strongly consistent SQLite per object (actor) | a good actor-per-project model, but placement lives in Cloudflare, not on our machines |

## 5. Code facts (verified at main `7a8cbd89`)

- **One activity sequence per Router**, for all boards: `crates/message-board-storage/src/storage_support.rs:255–264`.
- **Board actors are self-declared,** and a human actor skips the participant check: `crates/message-board-storage/src/message_write_operations.rs`, `post_message`.
- **Each Router opens separate stores:** `session-registry.sqlite`, `project-board.sqlite`, `automation.sqlite`, `provider-operations.sqlite` (`crates/codex-router-host/src/collaboration_runtime.rs:261–334`).
- **Live event replay is in memory only:** `crates/collaboration-service/src/provider_session_event_hub.rs:1`.
