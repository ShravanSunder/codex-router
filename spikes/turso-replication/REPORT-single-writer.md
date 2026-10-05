# Single-writer project stores: lease switching PoC

**[observed]** Executed 2026-10-05 on Apple Silicon macOS, in the assigned worktree/branch `codex-router.spike-turso-replication` / `spike/turso-replication`, starting at `1c791662`. No Router crate changed. Tools and all databases live under this worktree's `tmp/`; all servers bind to loopback. No Cloud, global installation, production process, account store, or secret was used.

**[inference]** This report supersedes the earlier report's project-store recommendation. The authoritative model in the follow-up packet is fleet entities written centrally and exactly one leased machine writer per project. The earlier two-offline-writer uniqueness/delete-edit cases are outside that model and were not repeated. Failure to fence a *returning old holder* remains relevant.

**[observed]** New proof: steady replication, clean switch, killed-holder switch with whole-push rejection and preserved tail, schema-version refusal, a working table rebuild, native SQLx checked queries, and a lower-SDK path using the application Tokio runtime. These are loopback PoC results, not production support or full Router integration proof.

## Topology and reproduction

**[inference]** Choose candidate (i): a fixed fabric Sync hub with a small Rust lease gateway. The leased machine remains the only producer of project writes; the hub accepts its replicated changes, while other machines pull read copies. Changing ownership does not relocate the hub.

```text
 central fleet-control.db: project -> holder, epoch, expiry
                    |
 A or B local writer -- push + capability/epoch --> Rust gateway --> Sync hub
 other machine       <-- pull/read copy --------- Rust gateway <--- Sync hub
```

**[observed]** The gateway stores the lease in a separate, centrally owned Turso file, outside the replicated project store. It maps PoC bearer capabilities to a holder, epoch and reader/writer role. Every non-SELECT pipeline request requires the current unexpired writer claim. It holds the same mutex across lease lookup and the complete upstream HTTP exchange; lease transfer takes that mutex too. Stale mutations get HTTP 409 before forwarding. Pull and narrowly recognized SELECT-only SDK requests can use reader capabilities. Unknown identities are rejected.

**[observed]** Pins: project Sync and CLI `0.8.1`; Tokio `1.52.3`; SQLx `0.9.0`; gateway Axum `0.8.8` and Reqwest `0.12.24`. The WIP driver is pinned at `b7e5fab909f5403f67f69d69d621f8f72e472a1f` and uses Turso/internal SDKs `0.7.0-pre.3`. Exact direct pins and the transitive lockfile are committed. Tokio and serde_json pins were advanced in this standalone project to satisfy the driver; repo dependencies were untouched.

**[observed]** End-to-end reproduction, run from the repository root:

```bash
# Only if the existing worktree-local release tools are absent:
bash spikes/turso-replication/setup-tools.sh
bash spikes/turso-replication/run-single-writer.sh
```

**[observed]** The second command was run and exited 0. It runs no old two-writer scenario. The four project scenario groups, native SQLx driver journey and lower-SDK runtime journey passed; invalid SQL failed compilation as expected. Formatting and all three scoped Clippy lanes passed with `-D warnings`.

**[observed]** Committed receipts: [project switching](evidence-single-writer/single-writer.txt), [native SQLx](evidence-single-writer/sqlx-turso.txt), [compile rejection](evidence-single-writer/negative-query.txt), [driver dependency lane](evidence-single-writer/driver-link-lane.txt), [runtime](evidence-single-writer/runtime.txt). Fresh data/server logs remain under `tmp/turso-single-writer/`, `tmp/turso-driver/`, and `tmp/turso-runtime/`. The harness prints every loopback server command, port and database path.

**[inference]** Candidate (ii), moving a sqld primary, adds server-role/WAL-history reconfiguration at every switch. It was not necessary to prove this shape. A fixed hub uses the documented push/pull surface and demonstrated gateway fence directly.

**[docs — https://github.com/tursodatabase/libsql/blob/libsql-server-v0.24.32/libsql-server/src/lib.rs#L617-L624, 2026-10-05]** sqld selects primary versus replica while building its server from RPC client configuration. No public promotion operation was established in that inspected path, CLI help or the [DeepWiki locator](https://deepwiki.com/search/for-a-project-database-with-ex_9e6436c7-6fa6-4f44-a2db-31dd8a987ab2). This is a bounded source finding, not proof that promotion is impossible.

## 1. Steady state and freshness

**[observed]** Command:

```bash
spikes/turso-replication/target/debug/single-writer-spike steady
```

**[observed]** A commits 15 numbered records locally, in three groups of five. The writer pushes each group; B then pulls. All writer and reader rows are compared after each pull. Trimmed final receipt:

```text
batch 0: committed_local=5; reader before push=[]
push_us=18003; reader before pull=[]
pull=true; pull_us=19749; reader sequences=1..5
batch 1: committed_local=10; reader before push=1..5
push_us=13470; reader before pull=1..5
pull=true; pull_us=18804; reader sequences=1..10
batch 2: committed_local=15; reader before push=1..10
push_us=15873; reader before pull=1..10
pull=true; pull_us=18926; reader sequences=1..15
SINGLE_WRITER steady PASS
```

**[observed]** Local commit is visible to the writer immediately. A successful push does not update another local reader until it pulls. The samples include the Rust gateway and loopback HTTP; they are three observations, not latency percentiles, a WAN benchmark, or a freshness SLA.

**[inference]** Freshness is protocol/application controlled: local commit -> acknowledged push -> completed reader pull. Without scheduling pull, the read view can remain stale indefinitely. The fabric should expose a replicated record watermark and distinguish local acceptance from replicated custody.

**Verdict [inference]: `works`.** One writer's stream replicates to the hub and another machine's read copy with explicit push/pull freshness.

## 2. Clean lease switch

**[observed]** Command:

```bash
spikes/turso-replication/target/debug/single-writer-spike clean
```

**[observed]** A writes sequences 1–10, pushes, and verifies pending CDC is zero. The central lease transfers from A/epoch 1 to B/epoch 2. B reopens its existing replica with the new writer capability, pulls through the last drain, then writes 11–15 and pushes. A's old writer handles close; A reopens with a reader capability and pulls. Old-holder local admission is checked and refused.

```text
CLEAN drained pending=0
LEASE drained release transfer A/1 -> B/2 persisted
old holder local admission=Err(stale lease holder=A/1 current=B/2)
B catch_up=true
final A reader = B writer:
  (1,A-1)..(10,A-10),(11,B-11)..(15,B-15)
exact sequence 1..15, no loss/duplicates
SINGLE_WRITER clean PASS
```

**[observed]** Assertions compare every sequence/body pair on both machines and require the exact sequence 1..15. The current store is never written by two holders simultaneously. In this PoC the schema/local-admission harness controls which native database connection is used for writes; the engine itself is not an OS-enforced read-only file.

**[inference]** A production handover needs an enforced drain/release state, a last-replicated watermark and a caught-up/schema-ready state before granting local write admission. The sample's controller explicitly orders and checks these steps; the gateway's mutex prevents lease change between its push check and upstream completion. This is not a test of a concurrent in-flight handover or of losing the push acknowledgment after commit.

**Verdict [inference]: `works with caveat`.** Clean switching preserves the entire stream; the application owns the ordered handover/admission protocol.

## 3. Unclean switch, whole-push fencing and rejected tail

**[observed]** Command:

```bash
spikes/turso-replication/target/debug/single-writer-spike unclean
```

**[observed]** A first pushes sequences 1–5. An owned `offline-writer` child opens A's existing synced file, commits 6–8 without pushing, reports pending CDC=3 and waits. The parent kills that process with SIGKILL after the durable receipt; the child does not gracefully close or checkpoint. No further A write is admitted. The fabric's injected test clock advances from 0 to 101 past expiry 100, then transfers A/1 to B/2. No sleep or wall-time race determines expiry in this proof.

**[observed]** B catches up to exactly 1–5, then creates its own records B-6 and B-7 under epoch 2 and pushes. A returns by reopening the killed writer's real on-disk file with its old epoch-1 capability. The first mutating request of A's push is rejected before forwarding. Counters, B's state, A's rows and pending CDC are inspected.

| Step | A's database | Hub/current B | Pending on A |
|---|---|---|---|
| Before dark | A-1..A-8 | A-1..A-5 | 3 |
| Reopen after B takes over | A-1..A-8 | A-1..A-5, B-6, B-7 | 3 |
| After refused push | A-1..A-8 | A-1..A-5, B-6, B-7 | 3 |
| Retained rejected file reopened | A-1..A-8 | unchanged | original CDC retained |
| Reset active read copy | A-1..A-5, B-6, B-7 | equal | fresh hydrated replica |

**[observed]** Trimmed output:

```text
child before kill TAIL_DURABLE pending=3 local_sequences=1..8
holder killed ... status=signal: 9 (SIGKILL)
new holder hydrated=[(1,A-1)..(5,A-5)]
old return before push=[(1,A-1)..(8,A-8)] pending=3
GATEWAY REFUSED whole pipeline bytes=363 ... A/1 current=B/2
stale push=Err(... status=409 ... stale lease ...)
gateway_before=(6 forwarded,0 denied), after=(6 forwarded,1 denied)
after refusal old rows=[(1,A-1)..(8,A-8)] pending=3
retained rejected DB readable=[(1,A-1)..(8,A-8)]
reset new read copy=[(1,A-1)..(5,A-5),(6,B-6),(7,B-7)]
retained rejected A.db still=[(1,A-1)..(8,A-8)]
SINGLE_WRITER unclean PASS
```

**[observed]** No mutating upstream request is forwarded for the stale push, not even its preparatory acknowledgment-table request. SDK `push()` fails; its local pending tail stays intact. A never calls `pull()` on this rejected dirty replica. The current holder's pulled state is compared before/after and is unchanged by the stale attempt.

**[observed]** The PoC writes `unclean/rejected-tail.json` with rejection status, machine, old/current epoch, last replicated sequence and all rejected rows. The original `A.db` and its sync metadata/sidecars stay at the original path. It is checkpointed, closed and reopened through the native local engine to prove readability; no rejected operation is deleted or replayed into current state. A new `A-current.db` bootstraps from the hub as a reader. `A-active.json` points at that file and names the retained rejected file/receipt. Reset changes the active copy rather than overwriting rejected evidence.

**[inference]** Accepted-but-unreplicated tail loss is an unavoidable recovery-point gap on unclean takeover. The fabric does not silently erase it or automatically merge it: A-6..A-8 survive as explicitly rejected evidence while B proceeds from the last replicated prefix. Record/business identities must distinguish the two epochs in production; numeric stream positions here are test data, not proposed Router identities.

**[inference]** The whole-request fence solves the earlier body-only-update/delete trigger problem at the producer/push boundary. It also avoids poisoned pending replay during reset by retaining the old generation and bootstrapping a separate clean file. Authentication, token secrecy and isolating the backend from direct clients are still necessary; the PoC's literal capability strings and loopback backend are test scaffolding, not production credentials or a network security boundary.

**Verdict [inference]: `works with caveat`.** Required whole-push refusal, preserved readable rejected tail, and reset are proven with a killed writer. Expiry clock and identity issuance are controlled PoC inputs; production trust and clock behavior remain unverified.

## 4. Schema upgrade with switching

**[observed]** Command:

```bash
spikes/turso-replication/target/debug/single-writer-spike schema
```

**[observed]** Three independent project files test add-column, direct rename and a rebuild. No two machines migrate/write a project concurrently. Each begins at `schema_version(singleton=1, version=1)`.

**[observed]** ADD COLUMN: A commits `ALTER TABLE records ADD COLUMN label TEXT`, fills the label and advances schema version to 2 in one transaction. Push and B pull succeed. A's CDC is drained before releasing; after transfer the old Router supports version 1, reads version 2, refuses local admission and produces no pending CDC.

```text
SCHEMA add replicated version=2
LEASE drained release transfer A/1 -> B/2 persisted
old Router supported=1 admission=Err(schema version 2 incompatible ...)
```

**[observed]** Direct rename, on a fresh project:

```sql
BEGIN IMMEDIATE;
ALTER TABLE records RENAME COLUMN body TO content;
UPDATE records SET content='renamed';
UPDATE schema_version SET version=3 WHERE singleton=1;
COMMIT;
```

```text
rename push=Err(... no such column: records."content" ... BATCH_STEP_ERROR)
writer schema: (sequence, content)
peer schema:   (sequence, body)
peer version=3; no lease release after failed migration
```

**[observed]** Rename still fails under exactly one writer. The peer's version row advances despite the absent rename, so a version number alone is not proof of schema integrity. The failed migration is not declared drained and its lease is not released.

**[observed]** Rebuild alternative, on another fresh project: capture the three records in Rust, then in one local IMMEDIATE transaction drop `records`, create `records` with the new `content` column, reinsert bound values, and advance version to 3. This deliberately uses the same final table name and no ALTER RENAME. No fake table or column coercion is used.

```text
SCHEMA_REBUILD DROP/CREATE same-name push=Ok
reader pull=Ok(true)
writer_schema = reader_schema = (sequence INTEGER PRIMARY KEY, content TEXT NOT NULL)
peer content=[(1,A-1),(2,A-2),(3,A-3)]
LEASE drained release transfer A/1 -> B/2 persisted
old Router admission=Err(schema version 3 incompatible with Router version 1)
```

**[observed]** Schema definitions and all three data rows match after rebuild. The old Router's version gate refuses writes after transfer. This sample schema has no external foreign keys, triggers or additional indexes. It is not a proof for a full Router table rebuild.

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/sync/engine/src/database_sync_operations.rs#L3074-L3096, 2026-10-05]** Push has specific DDL extension/idempotency handling. Single-writer ownership does not create general DDL support or guarantee remote atomicity after a replay error.

**[inference]** A supported migration can precede a switch; readers may inspect a newer store, but their Router must check compatibility before *any* local write. Pair the version gate with exact schema verification and drained-success proof, because the failed rename demonstrates ledger/schema disagreement. The working rebuild is a candidate migration recipe with narrower evidence than arbitrary DDL.

**Verdict [inference]: `works with caveat`.** Add-column + version gate and same-name rebuild + gate work. Direct rename does not work; general migration fidelity remains a gate.

## 5. SQLx on the Turso engine: pinned out-of-tree driver

**[observed]** Evaluated the requested `avencera/sqlx-turso` at commit `b7e5fab909f5403f67f69d69d621f8f72e472a1f`. Source was inspected before implementing the probe. DeepWiki had no index for that repository; the named GitHub source was cloned under `tmp/tools/` as fallback. The driver itself was not patched.

**[observed]** Online describe/checked-query build commands:

```bash
cargo build --manifest-path spikes/turso-replication/Cargo.toml \
  --no-default-features --features driver --bin driver-seed
spikes/turso-replication/target/debug/driver-seed tmp/turso-driver/metadata.db
SQLX_OFFLINE=false DATABASE_URL="turso://$PWD/tmp/turso-driver/metadata.db" \
SQLX_OFFLINE_DIR="$PWD/spikes/turso-replication/.sqlx" \
cargo build --manifest-path spikes/turso-replication/Cargo.toml \
  --no-default-features --features driver --bin driver-probe
spikes/turso-replication/target/debug/driver-probe
SQLX_OFFLINE=true cargo check --locked --manifest-path spikes/turso-replication/Cargo.toml \
  --no-default-features --features offline-proof --bin driver-probe
```

**[observed]** `driver-probe` executes `sqlx_turso::query!` and `sqlx_turso::query_as!` against real Turso files. The generated metadata is committed under `.sqlx/`. Both online and offline checked builds succeeded; typed rows and parameterized INSERT/UPDATE queries executed.

```text
query! typed sequence=1 body=local
query_as! [RecordRow { sequence: 1, body: "local" }]
extra bind compiled runtime_result=Err(... bind index 2 is out of bounds)
read_only open=Err(... read-only Turso connections ... not supported ...)
```

**[observed]** A permanent negative binary queries `missing_column`. Online `cargo check ... --features negative-query --bin driver-negative-query` exits 101 with `Parse error: no such column: missing_column`. The reproduction script requires that diagnostic. It proves actual schema checking, not merely a SQLx-shaped runtime API. Extra binds deliberately compile, then fail at runtime; arity checking is not provided.

**[observed]** Native Sync driver: writer seeds a store and pushes. A second driver connection owns a separate synced replica. The *same persistent reader connection*, using the same checked query, first sees the old prefix, then sees each new insert after its own `sync_pull()`. It also sees an updated body through `query!` after pull. No reopen is used.

```text
initial reader=[(1,first)]
insert 2, push: before pull=[(1,first)]
same_connection pull=true -> [(1,first),(2,item-2)]
insert 3,4 with push/pull -> same reader sees exact prefix through 4
same_connection update query! body=updated
DRIVER PASS checked macros, native Sync persistent read connection
```

**[observed]** The driver feature lane has no `libsqlite3-sys`, `libsql-ffi` or `libsql-sys` dependency. Its build produces no duplicate-SQLite symbol warning. It uses the Rust Turso engine instead of opening the synced WAL file with a second SQLite engine. This avoids both earlier issues in the demonstrated native-driver path. Arbitrary separate opens of the same file were not retested or established safe.

**[docs — https://github.com/avencera/sqlx-turso/blob/b7e5fab909f5403f67f69d69d621f8f72e472a1f/README.md, 2026-10-05]** These are the driver's Turso-specific macros, not stock `sqlx::query!`. It documents weak/no bind-arity checking, rejected read-only opens, a prerelease engine pin and untested remote Sync at that revision. Stock `cargo sqlx prepare` does not recognize its Turso URL; the upstream wrapper or online metadata generation is required.

**[docs — https://github.com/avencera/sqlx-turso/blob/b7e5fab909f5403f67f69d69d621f8f72e472a1f/crates/sqlx-turso-core/src/driver.rs#L39-L93 and https://github.com/avencera/sqlx-turso/blob/b7e5fab909f5403f67f69d69d621f8f72e472a1f/crates/sqlx-turso-core/src/connection.rs#L136-L177, 2026-10-05]** The connection retains the native Sync database and calls its push/pull directly, instead of mixing SQLx SQLite with Turso's external file changes. Sync custom IO and read-only options are explicitly rejected.

**[observed]** Initial fresh dependency resolution failed to compile the driver's `turso=0.7.0-pre.3`: its semver-ranged internal SDKs resolved to `0.7.2`, producing a missing `logical_mvcc_pull` field error. The PoC pins all eight internal Turso packages to `0.7.0-pre.3`, matching the upstream driver's lockfile. That source-preserving pin fixes the build. Engine `0.8.1` is not substituted underneath the driver.

**[inference]** This is a promising SQLx storage-adapter route, with real checked-query and Sync visibility proof. It is not a drop-in migration of current `Sqlite` queries/descriptions: database type, macro namespace, preparation workflow, read-only policy and supported engine version change. The driver probe used SDK `0.7.0-pre.3` with the local CLI hub `0.8.1`; it is not unified-version production integration proof.

**Verdict [inference]: `works with caveat`.** Driver-native checked queries and persistent Sync reads work. Read-only file opening and bind-arity checking do not; pins and integration changes are required.

## 6. Runtime ownership

**[observed]** Command:

```bash
cargo build --locked --manifest-path spikes/turso-replication/Cargo.toml \
  --no-default-features --features runtime-probe --bin runtime-probe
spikes/turso-replication/target/debug/runtime-probe
```

**[observed]** The new binary uses the public `turso_sync_sdk_kit::rsapi::TursoDatabaseSync` IO queue directly, pinned at `0.8.1`. It drives `create`, `connect`, `push_changes`, `wait_changes` and `apply_changes`; HTTP requests are awaited through Reqwest on the current Tokio runtime, and file completions are supplied by the application. It does not construct the high-level `turso::sync::Builder`, invoke its private IO worker, or construct another runtime.

```text
RUNTIME lower SDK 0.8.1 application runtime only; current Tokio available=true
bootstrapped reader: [(1,first)]
writer updates to second and pushes
before pull: [(1,first)]
pull=true reader: [(1,second)]
RUNTIME PASS no ... Builder / IoWorker / runtime constructor invoked
```

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/sync/sdk-kit/src/rsapi.rs#L243-L286 and https://github.com/tursodatabase/turso/blob/v0.8.1/sync/sdk-kit/src/rsapi.rs#L477-L546, 2026-10-05]** The lower SDK publicly provides configuration, async operation handles, queued IO requests and completion stepping. [IO completion APIs](https://github.com/tursodatabase/turso/blob/v0.8.1/sync/sdk-kit/src/sync_engine_io.rs#L185-L243) let the caller report HTTP status/data, file data, errors and completion. This route needs no engine fork or invented private method.

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/bindings/rust/src/sync.rs#L348-L412 and https://github.com/tursodatabase/turso/blob/v0.8.1/bindings/rust/src/sync.rs#L690-L707, 2026-10-05]** The high-level `turso::sync::Builder` unconditionally starts its private `turso-sync-io` worker and a second Tokio runtime. No public executor/IO injection hook was found in that builder or its feature table. A local non-Sync builder's `with_io` hook does not replace the Sync worker.

**[observed]** The pinned driver's older `turso-0.7.0-pre.3/src/sync.rs:514–525` also builds a private current-thread runtime; its adapter offers no replacement hook. The driver and the own-runtime probe therefore demonstrate two separate paths. They are not yet a combined SQLx-plus-custom-Sync-IO solution.

**[inference]** Avoiding the extra runtime is possible through public lower SDK APIs, but we pay for maintaining the IO adapter, streaming/backpressure, durable atomic metadata writes, error/cancellation handling, operation serialization and SDK-version compatibility. The probe buffers small HTTP responses, uses syscall/synchronous native SQL stepping and performs a directory fsync synchronously; heavy disk work would need a proper executor-safe integration. It proves the replication path and ownership seam, not production performance or fault completeness.

**Verdict [inference]: `works with caveat`.** Public lower SDK push/pull can run on our runtime. The convenient high-level crate and current SQLx driver cannot be configured to do so; integration ownership is the cost.

## What the fabric must provide for single-writer project stores

- **[inference]** Centrally persisted lease identity, epoch, expiry and holder; authenticated capability issuance; current-epoch validation on every push, with transfer serialized against admitted in-flight pushes. Keep lease/control records outside project replication.
- **[inference]** Writer-side admission that stops on release/expiry, and a handover state requiring successful drain, caught-up hydration, compatible version and verified schema before new writes. Native file access alone does not enforce reader-only behavior.
- **[inference]** Explicit custody/watermarks and recovery-point semantics: locally committed work can still be absent from the hub when a machine fails. A new holder starts at the last replicated prefix.
- **[inference]** A durable rejected-generation/tail record, an operator-visible disposition and a fresh-generation reset that preserves rejected evidence. No silent pull/replay, automatic stale-tail resubmission or overwriting the old file.
- **[inference]** Proven migrations, exact schema verification plus version gating, and a failure rule that blocks release/admission when DDL push diverges. The working sample rebuild is narrower than full Router migrations.
- **[inference]** A native storage adapter (potentially the pinned SQLx driver), reliable checked-query preparation, a supported engine-version policy and reader access enforcement. If one-runtime ownership is mandatory, an owned lower-SDK IO adapter and a way to connect it to the SQLx adapter are additional work.
- **[inference]** A production sync-server distribution, backend isolation, auth/TLS, backup/restore and restart/upgrade/reconciliation proof. The local CLI hub remains a development/testing server under the current docs.

## Recommended topology and limits

**[inference]** Recommend the fixed Sync hub + lease gateway for machine-level project stores, with exactly one leased local writer and reader copies elsewhere. Fleet-level entities remain centrally admitted/written. The clean and killed-holder cases now demonstrate this owner model directly; the earlier multi-writer conflict findings are not a reason to replace it with central project admission.

**[inference]** The PoC makes this topology viable as a design input, with three material gates before production: supported authenticated server operation, a migration strategy verified on actual stores, and a supported native SQLx/runtime integration. Simple epoch-in-row triggers are unnecessary as the primary fence once all pushes pass the trusted gateway. The backend must be inaccessible through an unfenced route.

**[docs — https://docs.turso.tech/sync/local-sync-server, 2026-10-05]** The CLI server is documented for development/testing and requires no auth token. This run does not change that support statement. A gateway does not manufacture production support or erase engine migration limitations.

**[observed]** Proof completion: `bash run-single-writer.sh` exit 0; 4 project scenario groups passed, 1 native-driver journey passed, 1 lower-SDK journey passed, invalid-column compile check exited 101 with the expected diagnostic, online/offline metadata builds passed, formatting and 3 Clippy feature lanes passed. Negative engine outcomes are explicitly recorded and do not count as successful migrations. No repo-wide test/release claim is made.

**[observed]** Unverified: real multi-machine WAN behavior; concurrent gateway transfer during an in-flight push; real-time expiry/clock drift and lease renewal; cryptographic caller authentication or hostile SQL; backend network isolation; response loss after server commit; gateway/process crash recovery; full Router schemas/indexes/FKs; Cloud/MVCC; a unified driver+0.8.1/custom-IO path; production support/auth/backup. The gateway's token map, injected clock and one-project controller are named stand-ins for those production boundaries. The server, SDK, files, child SIGKILL, gate, driver, compile checks and IO-queue operations themselves are real.

**[observed]** Research routes: current packet/spike -> [Turso DeepWiki locator](https://deepwiki.com/search/exact-public-apis-in-rust-turs_db362bcc-c579-4691-a56d-2febac518101) -> pinned public SDK code -> requested driver source (DeepWiki unavailable) -> fresh loopback proof. No saved Reader/UI/provider-log material was needed. The supporting ledger and full command logs are under `tmp/practices-research/2026-10-05-turso-single-writer/`.

**[observed]** Trace home remains the read coordination root `01a0f466-4201-7321-822e-018056ef60d0`, service `0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89`, project `01a09c8d-bfa1-7eb2-8432-595da973724f`, board `01a0f465-9f83-7f70-903c-aeeebe3904f6`, topic `01a0f465-b616-73e2-9707-d98c408e11de`. No execution root/Lead trace-file grant was supplied; contributor checkpoints were returned by authorized DMs and this assigned report, without posting to or resolving the coordination root.

**[inference] Return token:** `program-design-gap` — Lead disposition of production-server support, migration policy and native driver/runtime ownership; the assigned PoC evidence is complete, not a production implementation.
