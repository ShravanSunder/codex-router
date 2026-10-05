# M1.7 — Turso replication and sync spike

**[observed]** Executed 2026-10-04 America/Toronto (2026-10-05 UTC), on Apple Silicon macOS, branch `spike/turso-replication`, starting HEAD `21a9b029f9e5b4d3deea5dccf661abf7d4029861`. This standalone project changes no Router crate. All servers bind to `127.0.0.1`; all database files and tools are in this worktree's `tmp/`. No cloud account, production process, account store, or credential was used.

**[observed]** Pins: `turso = 0.8.1`, `libsql = 0.9.30`, `sqlx = 0.9.0`, `tokio = 1.48.0`; exact direct dependency pins and committed `Cargo.lock`. Server releases: `tursodb 0.8.1`, `sqld 0.24.32` (the latter prints build `40c272de 2025-02-14`). Both downloaded archives matched their published SHA256 files. These are two distinct engines/protocols, not interchangeable servers.

**[inference]** Recommendation: adopt the Turso family on the fabric first, using central Rust admission and the demonstrated libSQL/sqld embedded-replica topology. Do not cut existing SQLx/DELETE stores over to raw Turso Sync yet. D1=A has materially fewer demonstrated correctness hazards than D1=B. The owner's stated B preference remains an owner decision; this spike does not settle it.

## Reproduce and inspect

**[observed]** Commands run from the repository root:

```bash
bash spikes/turso-replication/setup-tools.sh
bash spikes/turso-replication/run.sh
```

**[observed]** The scripts download only to `tmp/tools/`, build two separate feature/link lanes, run the real embedded and Sync scenarios, probe a replica through stock SQLx SQLite, and run formatting and Clippy. The first two commands exited 0. The final suite's output ends with `HARNESS failures=0`. Negative database outcomes below are experiment results, not a claim that the desired fabric guarantees passed.

**[observed]** Committed receipts: [embedded](evidence/embedded.txt), [SQLx embedded](evidence/sqlx-embedded.txt), [Sync and compatibility](evidence/sync-and-compatibility.txt), [installed update trigger](evidence/fence-update.txt), [body-only update](evidence/fence-update-body.txt), [delete trigger](evidence/fence-delete.txt), [tool versions/digests](evidence/setup-tools.txt). Raw server logs and fresh run directories remain under `tmp/turso-runs/`. No test reuses a database from another conflict/DDL case.

**[observed]** Individual scenario commands, after building:

```bash
spikes/turso-replication/target/debug/embedded-replica-spike
spikes/turso-replication/target/debug/turso-replication-spike last-push
spikes/turso-replication/target/debug/turso-replication-spike uniqueness
spikes/turso-replication/target/debug/turso-replication-spike edit-last
spikes/turso-replication/target/debug/turso-replication-spike delete-last
spikes/turso-replication/target/debug/turso-replication-spike fencing
spikes/turso-replication/target/debug/turso-replication-spike fence-update
spikes/turso-replication/target/debug/turso-replication-spike fence-update-body
spikes/turso-replication/target/debug/turso-replication-spike fence-delete
spikes/turso-replication/target/debug/turso-replication-spike fence-abort
spikes/turso-replication/target/debug/turso-replication-spike fence-rollback
spikes/turso-replication/target/debug/turso-replication-spike schema
spikes/turso-replication/target/debug/turso-replication-spike schema-swap
spikes/turso-replication/target/debug/turso-replication-spike compatibility
```

**[observed]** The main executable supports those scenario selectors; `all` executes every Sync scenario plus compatibility. It forwards actual HTTP queries to the primary's `/v2/pipeline` and reports primary rows separately from the two local copies. Owned server children are killed and waited on after each case. Startup uses a bounded TCP readiness check; replication conclusions use explicit protocol completion and row reads, not sleeps.

## 1. Embedded replica with central primary — option A

**[observed]** `embedded-replica-spike` starts one primary and builds two `libsql::Builder::new_remote_replica(...)` databases with default read-your-writes and no periodic sync. Actual server command in the final receipt:

```bash
tmp/tools/libsql-server-aarch64-apple-darwin/sqld \
  --db-path tmp/turso-runs/1791167087536-9755/embedded/primary.sqld \
  --http-listen-addr 127.0.0.1:49842 --no-welcome --disable-metrics
```

**[observed]** Trimmed output:

```text
first write elapsed_us=1794 first=from-first second=initial primary=from-first
second.sync=Replicated { frame_no: Some(3), frames_synced: 1 }
pull elapsed_us=1019 second=from-first
down reads first=from-first second=from-first
down write=Err(WriteDelegation(... Connection refused (os error 61)))
down sync=Err(Replication(PrimaryHandshakeTimeout))
reconnect first=from-first second=from-first
old remote connection after restart=Err(... status=400 ... invalid baton)
ADMISSION accepted=admitted stale=Err(stale expected body)
```

**[observed]** A replica write reached the primary and was readable on its originating replica when the write returned. The other replica retained its old value until explicit `sync()`. The measured durations are one loopback sample each, not a latency distribution or WAN benchmark. The lag here is deliberately application-controlled and can persist without a sync.

**[observed]** With the primary killed, both replicas still read committed local data; a delegated write failed and was not queued or applied locally. After restarting the same primary directory, both replicas synced successfully and no `outage-write` appeared. A remote primary connection created before restart retained an invalid Hrana baton; creating a new remote connection restored operation. Admission and subsequent replica sync then succeeded.

**[inference]** Fabric outage requests need an application-owned pending/outbox state; the default replica does not provide an accepted-write buffer. Reconnect logic needs to distinguish a failed admission from an unknown result and recreate stale remote connections. This experiment did not inject a network failure after server commit and before client acknowledgment.

**Verdict [inference]: `works with caveat`.** Central-primary/local-read topology works, with explicit freshness, unavailable writes during outage, and remote-connection recovery obligations.

## 2. Turso Sync offline writes, push/pull, conflicts — option B

**[observed]** The `last-push`, `uniqueness`, `edit-last`, and `delete-last` selectors each start their own `tursodb DATABASE --sync-server 127.0.0.1:PORT`, seed A, push, bootstrap B, kill the server, write on both local databases, then restart and push/pull. A representative final server command:

```bash
tmp/tools/turso_cli-aarch64-apple-darwin/tursodb \
  tmp/turso-runs/1791167091638-9807/last-push/primary.db \
  --sync-server 127.0.0.1:49906
```

**[observed]** Same-row result, normalized from the receipt's typed values:

```text
offline: A=(1,offline-A), B=(1,offline-B)
push while server down: Err(... http request failed ... Connect)
after A push: primary=(1,offline-A), B=(1,offline-B)
B pull with pending edit: Ok(true); B remains (1,offline-B)
after B push, A pull, B pull: A=B=primary=(1,offline-B)
```

**[docs — https://docs.turso.tech/sync/conflict-resolution, 2026-10-04]** Documented conflict policy is last push wins. Pull rolls back to the last synced state, applies remote changes, and replays pending local changes. The document says failed replay leaves the previous state intact.

**[observed]** Different primary keys with the same unique `handle`:

```text
A insert=(10,same,A), B insert=(20,same,B), both accepted offline
A push=Ok
B push=Err(... UNIQUE constraint failed: accounts.handle ... BATCH_STEP_ERROR)
B pending CDC: 1 before push, 1 after failed push
before pull: A/primary=(10,same,A), B=(20,same,B)
B pull=Ok(true)
after pull: A=B=primary=(10,same,A); B pending CDC=0
```

**[observed]** B's locally accepted row disappeared on successful pull after the failed push. The initial push error was visible, but the later pull did not return a rejection or preserve the row in pending CDC. No application conflict archive exists in this test.

**[observed]** Delete-versus-edit used a seeded row. A deleted it offline; B edited its body offline. Pushing delete then edit, or edit then delete, both completed without a reported push failure. After pulls, A, B and the primary had no row in both cases. The edit did not resurrect the deleted row.

**[inference]** “Last push wins” alone does not describe every semantic outcome: update-after-delete can successfully affect no row. Locally accepted work can disappear or become a no-op. Required rejection/reconciliation and retention of meaningful actions belong outside raw database sync.

**Verdict [inference]: `works with caveat`.** Offline storage and transport work; raw reconciliation fails our no-silent-loss/admission expectations for uniqueness and delete/edit.

## 3. Single-writer fencing and trigger failure state

**[observed]** Commands: `fencing`, `fence-update`, `fence-update-body`, `fence-delete`, `fence-abort`, `fence-rollback`. Each uses a separate primary and two clients. Lease epoch starts at 1; B promotes the authoritative lease to 2 and pushes successor work while A still holds local epoch 1.

**[observed]** The first workaround created INSERT/UPDATE triggers on A and called `push()`. Both calls returned success, but `sqlite_schema` showed the triggers only on A: B and the primary had none. B's pull returned `false`. A subsequently overwrote the successor body, and after pulling could push the lease table back to epoch 1. This is a failure to replicate that trigger DDL and to protect the control table, not evidence that an installed trigger cannot execute.

**[observed]** Further cases installed triggers directly on the primary through `/v2/pipeline`, pulled them onto both clients, and queried `sqlite_schema` to prove their presence. The UPDATE trigger compared `NEW.epoch` with the server's lease table and used `RAISE(ROLLBACK,'stale epoch')`:

```text
stale UPDATE SET epoch=1,body='stale-holder': push Err(stale epoch)
primary remains (1,2,successor)
stale UPDATE SET body='stale-holder': push Ok
primary becomes (1,2,stale-holder)
```

**[observed]** A server-installed DELETE trigger comparing `OLD.epoch` with the lease table accepted A's stale deletion. The primary row disappeared even though B's successor row had epoch 2 and A's local row had epoch 1.

**[inference]** Epoch carried explicitly in an UPDATE can be checked. A stored row epoch is insufficient to authenticate the producer of a body-only UPDATE or DELETE: the trigger sees server row values. Every mutation needs producer/lease provenance at admission, including deletion. A lease table clients can also overwrite is not an authority boundary.

**[observed]** For appends, the full inserted row carries epoch, so an installed `BEFORE INSERT` epoch trigger can reject stale replay. Its failure action changes sync behavior:

```text
RAISE(ABORT):
  stale push Err(stale epoch); pending CDC remains 1
  retry push Ok
  pull Ok(true); stale row disappears; pending CDC becomes 0
  primary contains only successor; current-holder next push Ok

RAISE(ROLLBACK):
  stale push Err(stale epoch); retry Err(stale epoch)
  pull Err(failed to replay local change after remote apply ... stale epoch)
  local stale row and pending CDC=1 remain; primary remains successor-only
  current-holder next push Ok
```

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/sync/engine/src/database_sync_operations.rs#L3008-L3242, 2026-10-04]** Push builds a `BEGIN IMMEDIATE` batch. Later statements, including the acknowledgment table update and `COMMIT`, are conditioned on the transaction remaining open, rather than on every preceding statement succeeding.

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/cli/sync_server.rs#L454-L527, 2026-10-04]** The server records statement errors and continues evaluating later batch steps. `RAISE(ABORT)` aborts a statement; `RAISE(ROLLBACK)` ends the transaction.

**[inference]** These two source paths explain why an ABORT failure can leave acknowledgment progress committed, whereas ROLLBACK preserves the pending rejected operation. The latter then obstructs pull because replay is still invalid under the newer lease. It is a useful narrow rejection seam, but requires an explicit reconciliation policy.

**[docs — https://docs.turso.tech/sync/local-sync-server and https://github.com/tursodatabase/turso/blob/v0.8.1/cli/sync_server.rs#L222-L343, 2026-10-04]** Local server needs no token. The inspected routing/execution code provides no server authorization callback or dynamic lease-epoch scope. SQL triggers are executable on it, as proven above. This is a bounded inspection of this server, not a claim about proprietary Cloud internals.

**[docs — https://docs.turso.tech/sdk/authorization/fine-grained-permissions, 2026-10-04]** Cloud tokens support table/action permissions (`data_read`, `data_add`, `data_update`, `data_delete`, schema actions). No lease-registry predicate is described on that page. Cloud auth was not exercised.

**Verdict [inference]: `doesn't work` as complete single-writer fencing.** Installed epoch+ROLLBACK triggers reject epoch-carrying appends and explicit-epoch updates, but ordinary updates/deletes and mutable lease state defeat the simple workaround. Failed replay can leave a client unable to pull.

## 4. Schema change under sync

**[observed]** Commands: `schema` and fresh-database `schema-swap`. The add-column and rename experiments run explicit local `BEGIN IMMEDIATE ... COMMIT` migrations, with a migrations-version table, then push and pull.

```sql
-- Version 2 on A
ALTER TABLE notes ADD COLUMN label TEXT;
UPDATE notes SET label='added';
INSERT INTO migrations VALUES(2);

-- Version 3 on A
ALTER TABLE notes RENAME COLUMN body TO content;
UPDATE notes SET content='renamed';
INSERT INTO migrations VALUES(3);
```

**[observed]** Trimmed output:

```text
add push=Ok; B pull=Ok(true)
A=B=primary: (1,seed,added)
rename push=Err(... no such column: notes."content" ... BATCH_STEP_ERROR)
B pull=Ok(true)
A: schema=(id,content,label), data=(1,renamed,added)
B/primary: schema=(id,body,label), data=(1,seed,added)
migrations A=B=primary: 1,2,3
```

**[observed]** The failed rename push nevertheless advanced the remote migration ledger. A local transaction did not ensure remote migration atomicity across this unsupported DDL replay and subsequent failed DML. The remote schema and migration ledger disagree.

**[observed]** Independent name-swap case, no intervening data UPDATE:

```sql
BEGIN IMMEDIATE;
ALTER TABLE notes RENAME COLUMN a TO temporary;
ALTER TABLE notes RENAME COLUMN b TO a;
ALTER TABLE notes RENAME COLUMN temporary TO b;
COMMIT;
```

```text
push=Ok; B pull=Ok(false)
A SELECT a,b: B1,A1
B/primary SELECT a,b: A1,B1
A schema column order: id,b,a
B/primary schema column order: id,a,b
```

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/sync/engine/src/database_sync_operations.rs#L3074-L3096, 2026-10-04]** Push comments explicitly scope supported DDL to schema extensions such as CREATE TABLE/INDEX and ADD COLUMN. They describe special handling for ADD COLUMN errors and idempotent CREATE rewriting.

**[docs — https://github.com/tursodatabase/turso/issues/9415 and https://github.com/tursodatabase/turso/issues/9430, 2026-10-04]** #9415 is closed and concerns MVCC replay after drop/rename; #9430 remains open and concerns a remote MVCC column swap matched by name. #9430's author explicitly says the dev server differs from Cloud and their Cloud reproduction did not reproduce locally.

**[inference]** These results demonstrate local-writer push DDL failures on pinned 0.8.1. They are not exact reproductions of those remote MVCC pull issues, and cannot clear them. Cloud, a post-fix SDK, MVCC sync, production migrations and mixed-version clients remain unverified.

**Verdict [inference]: `doesn't work` for general migrating project stores.** ADD COLUMN worked; rename, name swap and ledger integrity did not.

## 5. Rust typed admission and read replicas

**[observed]** Commands: `embedded-replica-spike` and `compatibility`. [embedded_replica.rs](src/embedded_replica.rs) implements `PostRequest`, validates a nonempty body, opens `transaction_with_behavior(Immediate)` on the primary connection, reads a state precondition, writes bound parameters, and commits; stale preconditions explicitly roll back. Its read peer only calls sync and SELECT in this admission portion.

```text
ADMISSION accepted=admitted stale=Err(stale expected body)
ADMISSION invalid=Err(body must be nonempty)
second replica after sync: admitted (asserted)
```

**[observed]** [compatibility.rs](src/compatibility.rs) also demonstrates the new `turso` API with a validated `BoardPost`, `Connection::transaction_with_behavior(Immediate)`, state read, parameterized write, commit, push, and peer read:

```text
TURSO admission local=(1,admitted)
TURSO admission invalid=Err(body must be nonempty)
TURSO admission push=Ok
TURSO admission peer read=admitted
```

**[observed]** The central-primary transaction is actual libSQL/sqld admission. The new-engine transaction is on a local synced writer, subsequently pushed, not Rust code embedded into the CLI server. No server-side Rust admission hook was invented or tested.

**[inference]** Both APIs permit a clean Rust transaction boundary. Use that boundary where authority lives, with authenticated typed actions. In option B, producer-side Rust validation is not receiver-side admission, and logical replay does not call that Rust validation again. In option A, application-controlled replicas can read only, but the embedded-replica API itself can forward writes (§1); credential/gateway enforcement must make bypass impossible. This test did not enforce read-only credentials.

**Verdict [inference]: `works with caveat`.** Transactional admission is clean; transport access policy and placement of validation remain fabric responsibilities.

## 6. SQLx, journaling and Tokio compatibility

**[observed]** Commands: `compatibility`, and `sqlx-probe FILE` on the emitted embedded-replica path. SQLx 0.9.0 uses its stock bundled SQLite (version printed as 3.51.3), read-only connection options, and ordinary runtime SQL queries.

```text
SQLX embedded closed file: mode=wal, rows=[(1,admitted)]
TURSO journal_mode=wal
TURSO PRAGMA journal_mode=DELETE returns wal; subsequent mode remains wal
EMBEDDED set DELETE=Err(Sqlite3UnsupportedStatement)
```

**[observed]** The live new-engine synced file was readable initially. The same SQLx connection did not track an incoming sync update correctly:

```text
initial SQLX live read_only=[(1,admitted)]
peer writes remote-update and pushes; primary reads remote-update
local pull=true; Turso connection reads (1,remote-update)
same SQLX live connection reads (1,admitted)
close/reopen SQLX while Turso is still open: (1,remote-update)
after explicit checkpoint: (1,remote-update)
closed/checkpointed file reopened through SQLX: (1,remote-update)
```

**[inference]** SQLx can open these SQLite-compatible WAL files read-only, but this does not establish safe continuous read access to a file another engine is syncing. The stale connection is a concrete counterexample. The cause inside SQLite/WAL coordination was not established; reopening is an observed visibility boundary, not a production integration prescription.

**[observed]** The initial combined executable linked `libsql` core/replication and SQLx SQLite and emitted duplicate `sqlite3_*` symbols. Final proof uses separate feature/link lanes: embedded executable has the libSQL C engine, while the Sync/SQLx executable uses remote-only libSQL and stock SQLite. All final builds and Clippy checks emitted no warnings. The initial contaminated executable's output is excluded from committed proof.

**[observed]** New-engine Sync executes successfully when awaited from our Tokio runtime. No second runtime is created by this spike's application code.

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/bindings/rust/src/sync.rs#L690-L707, 2026-10-04]** The crate itself spawns a `turso-sync-io` thread, constructs `tokio::runtime::Builder::new_current_thread().enable_all().build()`, and calls `rt.block_on(...)`. The actual compiled registry source `turso-0.8.1/src/sync.rs:695–703` was also inspected.

**[inference]** “Callable from Tokio” is true; “Sync uses only our existing Tokio runtime” is false for this pin. Accepting the crate's internal runtime, replacing its IO integration, or waiting for a different SDK is a design choice, not solved here.

**[observed]** Current Router `crates/message-board-storage/src/board_connection.rs:29` selects DELETE journaling. This spike tested no existing Router schema, SQLx compile-time query descriptions, migration verifier, or MVCC-file interoperability. Those cannot be inferred from a successful generic SELECT.

**Verdict [inference]: `works with caveat` for file opening and calling async APIs; `doesn't work` as a drop-in SQLx/DELETE/one-runtime replacement.**

## 7. Production self-hosting and auth

**[observed]** Both worktree-local servers booted and served their respective protocols without a cloud account. `sqld --help` exposes Ed25519 JWT key configuration, legacy HTTP Basic auth, an admin auth key, gRPC listeners and TLS certificate options. `tursodb --help` exposes `--sync-server`, `--sync-dir`, and experimental options, with no sync-server auth option. All requests in this spike were unauthenticated loopback requests.

**[docs — https://github.com/tursodatabase/libsql/blob/libsql-server-v0.24.32/docs/BUILD-RUN.md and https://github.com/tursodatabase/libsql/blob/libsql-server-v0.24.32/libsql-server/README.md, 2026-10-04]** sqld is an official self-hostable server with documented prebuilt binaries, Docker/source deployment and read-replica support. Its default binding is loopback. Those docs do not provide a production SLA or maintenance/support contract.

**[docs — https://docs.turso.tech/libsql and https://docs.turso.tech/features/embedded-replicas/introduction, 2026-10-04]** libSQL is described as production-ready and suitable for mission-critical workloads; embedded replicas are “fully supported in production.” The embedded-replica support statement describes Turso Cloud. It must not be recast as a self-hosting support SLA.

**[docs — https://github.com/tursodatabase/libsql/blob/libsql-server-v0.24.32/libsql-server/src/auth/user_auth_strategies/jwt.rs#L22-L136, 2026-10-04]** sqld validates EdDSA bearer JWTs and supports namespace, permissions/authorization and expiration claims. This is real authentication code, not a reverse-proxy suggestion. Credential configuration and token enforcement were not executed here.

**[docs — https://docs.turso.tech/sync/local-sync-server, 2026-10-04]** The new-engine local sync server is documented for development/testing, with no auth token needed. No production-support commitment for this server was found in that page, its CLI help, or its inspected source.

**[docs — https://github.com/tursodatabase/turso/blob/v0.8.1/README.md#L432-L454 and https://docs.turso.tech/libsql, 2026-10-04]** The new Turso engine is currently described as production-ready and running at multiple organizations, despite a pre-1.0 version. That claim concerns the database engine; it does not turn the local dev sync server into a documented production distribution. A blanket “Turso is not production-ready because pre-1.0” is unsupported.

**[inference]** A self-hosted libSQL replica server is a documented implementation path with an established engine foundation. Production operation, authentication policy, backup/restore, upgrade proof and monitoring belong to the fabric operator. For the new engine's self-hosted Sync server, production support and appropriate authenticated admission remain gaps. Cloud support cannot be proven under this packet's no-Cloud constraint.

**Verdict [inference]: `works with caveat` for self-hosted sqld/embedded replicas; `couldn't test (why)` for a production-supported new-engine Sync server because no such supported distribution/commitment was established in the named sources, and Cloud was explicitly excluded.**

## What the fabric must add around Turso

- **[inference]** Authenticated typed admission before authoritative writes, transactional state/revision checks, and write credentials inaccessible to read replicas (§1, §5, §7).
- **[inference]** A pending-request/outbox contract, deduplication and unknown-result reconciliation; default embedded writes fail during outage (§1).
- **[inference]** Enforced lease provenance on every mutation, including deletes, and separately protected lease control state. Row epochs and last-push-wins are insufficient (§3).
- **[inference]** A conflict/rejection record retaining locally accepted work, plus a deliberate recovery path for a rejected CDC tail that blocks pull. Do not treat successful retry/pull as proof the intended write was accepted (§2–§3).
- **[inference]** Migration/version gates that verify real schema and data, not just the migration ledger, and a supported DDL/upgrade strategy before project-store sync (§4).
- **[inference]** Explicit replica freshness/watermarks, reconnect handling, a supported read API/storage adapter, and a decision on the SDK's internal runtime (§1, §6).
- **[inference]** Production deployment, auth enforcement, backup/restore and operator proof appropriate to the selected server (§7).

## D7 and D1 recommendation

**[inference] D7:** Fabric only first, within the Turso family. The demonstrated self-hosted libSQL/sqld route supports central transactional admission and local read replicas. Keep the present Router SQLx/DELETE stores intact until their own migration and interoperability proof exists. Defer project/every-store adoption of new-engine raw Sync: observed silent loss/no-op conflicts, failed rename/trigger replication, partial migration-ledger advancement, stale SQLx reads and second-runtime ownership all add substantive work (§2–§6).

**[inference] Scope gap:** “Turso” could mean the family including libSQL or exclusively the new Rust engine. If R14 mandates the latter, the production self-hosted Sync/admission distribution is unresolved; the sqld proof is evidence for option A, not permission to substitute engines. The Lead/owner must settle that before an implementation commitment.

**[inference] D1:** Recommend A for authoritative board/project decisions now. B permits low-latency local acceptance while fabric is unreachable, but those accepted writes may later disappear or become unreplayable; it also requires full mutation fencing and tail/handover/reconciliation semantics not provided by the tested Sync path. R5's local facts/outbox requirements remain obligations independent of D1; recommending A for decisions does not move facts to server-first.

**[inference]** A is not a free operational choice: it pays one authority round trip and unavailable admission during a fabric outage. The sample loopback durations are not proof of acceptable real-world latency. The owner's B preference is recorded in the controlling packet, and choosing A is still an owner decision informed jointly by M1.7 and M1.0.

## Verification, coverage and handback

**[observed]** Final `run.sh`: exit 0; embedded real-server journey and separate SQLx probe passed; nine independent Sync scenario receipts and the targeted compatibility case completed, with `HARNESS failures=0`. Three additional installed-trigger probes (`fence-update`, `fence-update-body`, `fence-delete`) exited 0 with their negative/positive outcomes above. After adding those probes, targeted build, fmt and Clippy were checked again. This is standalone executable real-path proof, not a workspace unit-test or release-health claim.

**[observed]** Required scoped quality commands, all exit 0 and zero errors/warnings:

```bash
cargo fmt --manifest-path spikes/turso-replication/Cargo.toml -- --check
cargo clippy --locked --manifest-path spikes/turso-replication/Cargo.toml --all-targets -- -D warnings
cargo clippy --locked --manifest-path spikes/turso-replication/Cargo.toml \
  --no-default-features --features embedded --bin embedded-replica-spike -- -D warnings
bash -n spikes/turso-replication/setup-tools.sh spikes/turso-replication/run.sh
```

**[observed]** Coverage route: local controlling design §2–§5, architecture-options §3, work-tree M1.7 and journal owner; [DeepWiki repository/API locator](https://deepwiki.com/search/handson-macos-rust-tokio-offli_f57c691c-b202-49a3-a2bf-2240d47fcc4b); then pinned upstream code and current primary docs; then the real local experiments. Neither OSS checkout existed under the designated local OSS home, so source was fetched under this worktree's `tmp/tools/turso-source`. crates.io REST metadata returned HTTP 403; sparse registry/Cargo and release downloads worked. Two guessed self-host/auth document paths returned 404; the docs index and pinned sqld auth code supplied the sources above. No Reader, UI or provider-transcript source was relevant. No independent reviewer was commissioned; the Lead is the accepting reader.

**[observed]** Unverified: Turso Cloud and its conflict/DDL/auth behavior; MVCC sync; newer prereleases; mixed SDK/schema versions; full Router SQLx/migration integration; real WAN lag or throughput; production support/SLA; authenticated server deployment; backup/recovery; response-loss after committed writes; multi-machine lease transfer. Where looked: cited docs/index, pinned server/SDK code, the two named issues, and bounded loopback runs. Source and observations are separated throughout.

**[observed]** Work home read: service `0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89`, project `01a09c8d-bfa1-7eb2-8432-595da973724f`, board `01a0f465-9f83-7f70-903c-aeeebe3904f6`, topic `01a0f465-b616-73e2-9707-d98c408e11de`, coordination root `01a0f466-4201-7321-822e-018056ef60d0`. No execution root or Lead trace-file grant was supplied. That trace gap was returned through authorized direct messages; no coordination root was created, joined or resolved. Supporting ledger: `tmp/practices-research/2026-10-04-turso-replication/research-ledger.md`; assigned report and direct-message checkpoints hold the contribution.

**[inference] Return token:** `program-design-gap` — D1/D7, full push fencing, rejected-tail recovery, supported production server/admission and runtime/storage integration need disposition by the Lead/owner. Spike evidence is complete; those product decisions are not made here.
