# Participant history: Program Design

Date: 2026-10-02. Requirements and Specification: `2026-09-14-thread-participants.md` §4a (owner, 2026-10-01). This document says which code owns each part of §4a, how the migration and backfill run, and what the wire gains. Owner decisions stay marked in §4a; choices made here are marked **(design)** with their reason.

## 1. Ownership

| Concern | Owner | Change |
| --- | --- | --- |
| Schema and backfill | `crates/message-board-storage/migrations/202610020001_participant_history.sql` | the §4a `ALTER`s, index, and backfill `UPDATE`s in one migration |
| Expected schema | `crates/message-board-storage/src/board_schema_migrations.rs` | register the migration in the `include_str!` list and the joined expected schema, or `validate_schema` rejects every open |
| Role-changing events | `crates/message-board-storage/src/participant_records.rs` (`join_thread`, `leave_thread`, `resolve_in_transaction`, `insert_lifecycle_activity`) and `message_write_operations.rs` (`create_thread`) | every lifecycle insert writes the three participant columns from one value, `ParticipantChange` |
| Reply attribution | `message_write_operations.rs` `post_message` | resolve `posted_from_activity` before the `board_messages` insert |
| Read attribution | `message_row_decoding.rs` `load_message_unattributed` (the only `Message` constructor) and `message_history_reads.rs` | join the attributing event and decode its Role |
| Wire | `crates/message-board/src/board_messages.rs` `Message` | `postedAsRole: Option<ParticipantRole>`, omitted when unknown |

Role names keep their single Rust mapping (`role_name` / `decode_role` in `participant_row_decoding.rs`); a stored Role that does not decode fails the read with the existing corrupt-row error rather than being dropped (AGENTS.md: reject invalid stored values).

## 2. Writes

`ParticipantChange` **(design)** replaces the bare column list at every participant lifecycle insert, so no call site can forget a column:

```rust
/// Whose Role an activity changes, and to what (§4a). `None` role means the Role ended.
struct ParticipantChange<'key> {
    participant_key: &'key str,
    participant_role: Option<ParticipantRole>,
    replaced_participant_key: Option<&'key str>,
}
```

`insert_lifecycle_activity` takes `Option<ParticipantChange>`; `threadResolved` and `threadUnresolved` pass `None`. The `create_thread` literal `participantJoined` insert writes the same columns.

| Call site | `ParticipantChange` |
| --- | --- |
| `create_thread` with `--role` | joiner, stated Role, none |
| `join_thread`, first or repeat join | joiner, requested Role, none |
| `join_thread --replace` | joiner, `orchestrator` / `implementer`, the holder closed at `:279-288` |
| `leave_thread --to` (handover) | the target, `orchestrator`, the leaver |
| `leave_thread` | the leaver, no Role, none |
| `resolve_in_transaction` (also `leave --resolve`) | none |

**Main message attribution (design).** §4a rule 2 names replies. A new Thread's main message is posted by a Participant whose Role is stated in the same request, so `create_thread` sets the main message's `posted_from_activity` to its `participantJoined` event with an `UPDATE` after that event is inserted (the foreign key is immediate, so the message cannot reference the event before it exists). A `create` by a `human`, even with a Role, stores NULL like every human post; a session cannot `create` without a Role. Existing main messages are not backfilled (§3). Reason: the owner's requirement is that a post never loses the Role it was written under, and a new main message's Role is known exactly.

**Attribution rule (design, amends §4a rule 2).** "The latest event that gave the author a Role" is not provable when an event of unknown meaning lies between that grant and the reply: a surviving handover followed by an overwritten rejoin would label a later reply with the stale handover Role. Attribution therefore looks at the latest event that *could have changed* the author's Role, its **Role barrier**, and accepts it only when it is a known grant to the author. For author `X` on root `R`, the barriers are the events on `R` that are:

- `participantJoined` or `participantLeft` with `actor_key = X`;
- `orchestratorReplaced` / `implementerReplaced` with `participant_key = X` or `replaced_participant_key = X`;
- `orchestratorReplaced` / `implementerReplaced` with `participant_key IS NULL` (unattributed history: it may have been `X`'s);
- `threadResolved` (it ends every Role on the Thread).

`posted_from_activity` is the latest barrier before the reply when that barrier has `participant_key = X` and a non-NULL `participant_role`; otherwise NULL, meaning the Role is unknown. Every event written after this change is fully attributed, so for new history the rule returns the author's current grant. One SQL expression, `latest_role_barrier`, is shared by the live write and the backfill so they cannot diverge; a test runs both over the same sequence.

**Replies.** `post_message`, for a `session` author with a `root_id`, evaluates `latest_role_barrier` after `require_open_participant` and before the `board_messages` insert, and stores its result, NULL included. NULL on a live write happens only for a Participant whose current Role predates Participant history and cannot be proven, for example the target of a handover whose leaver later rejoined. **(owner-deferred, 2026-10-02: option B.)** Such replies omit `postedAsRole` until the author's next join or handover gives them a recorded grant. The alternative, a baseline event per open Participant written by the migration, was deferred: it adds an activity kind that subscribers and push delivery would see. It remains possible later as its own migration. Human authors and top-level posts without a `root_id` store NULL.

## 3. Migration and backfill

One migration, in the order §4a gives, inside sqlx's migration transaction. The backfill is plain SQL because every rule is a join over stored rows with nothing to validate in Rust; `foreign_key_check` after migrating covers every key it writes. Statements run in this order, each reading the results of the earlier ones.

1. **Join and leave.** `participantJoined` and `participantLeft`: `participant_key = actor_key`.
2. **Replaced events, split by proof (design).** An `orchestratorReplaced` / `implementerReplaced` event is one of two writes that share a kind, and only stored rows tell them apart:
   - *Handover* (only ever `orchestratorReplaced`): the actor's `thread_participants` row on that root has `closed_at_activity = event` and `replaced_by` set. Then `participant_key = replaced_by`, `replaced_participant_key = actor_key`, `participant_role = 'orchestrator'`.
   - *Join --replace*: the actor's row has `joined_at_activity = event`, or another row on that root has `closed_at_activity = event` and `replaced_by = actor_key`. Then `participant_key = actor_key`, `participant_role` from the kind, and `replaced_participant_key` = that other row's `reader_key` when present. **(design, amends §4a)** The kind proves the Role: `join_thread` writes `orchestratorReplaced` only for an orchestrator replacement and `implementerReplaced` only for an implementer replacement, so the Role is provable even when the joiner's row was later overwritten.
   - Neither proven (a later rejoin overwrote the rows): all three stay NULL. Guessing the actor would attribute a handover to the leaver.
3. **Roles on joins.** A `participantJoined` event gets the Role of the row whose `joined_at_activity` equals it, when that Role is not `orchestrator`, or when it is `orchestrator` and no `orchestratorReplaced` event on that root after the join is either attributed to this Participant or unattributed. A later handover rewrites the row's Role in place without moving `joined_at_activity`, so an `orchestrator` row only proves its join when no handover could have produced it. Earlier, overwritten joins stay NULL.
4. **Replies.** For a `session` author's reply (`root_id` set), `posted_from_activity = latest_role_barrier` evaluated before the reply's own `threadMessageCreated` sequence.
5. **Main messages** are not backfilled. A legacy session root followed by its author's first join is indistinguishable from `create --role`, because sessions could post roots before Participants existed.

`board_migration_tests.rs`' latest-version assertion moves to `202610020001`.

## 4. Reads and wire

`StoredMessageRow` gains the attributing event's `root_id`, `kind`, `participant_key`, `participant_role`, and `activity_sequence` through `LEFT JOIN board_activity pa ON pa.activity_sequence = m.posted_from_activity`. Decoding validates the relationship in Rust, alongside the existing cross-row checks, and fails the read with the corrupt-row error when it does not hold:

- the event's `participant_key` is the message author;
- its `root_id` is the message's root (for a main message, the message itself);
- its Role is present and decodes as a `ParticipantRole` (`decode_role`, made crate-visible);
- for a reply, its sequence precedes the message's activity; for a main message, it is a `participantJoined` on that root.

`Message` gains:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub posted_as_role: Option<ParticipantRole>,
```

Absent means "human author", "posted before Participant history", or "Role unknown" (§4a rule 3, with the attribution rule above). The field reaches every surface that carries `Message`: post, show, list, create results, search, inbox, and push records.

**Version skew.** `Message` has `deny_unknown_fields`, and nothing refuses an older client: service discovery compares the Host's digest with its own manifest, and initialization negotiates Control `1.0` with only a warning on version mismatch. A client built before this change, typically a long-running agent-router MCP server started on the old binary, rejects any message carrying `postedAsRole`; for a post it has already committed, `board_mutation_call` reports an unknown outcome. Newer clients reading an older Host see the field absent, which `default` handles. The cutover is therefore: upgrade, restart the Host, then restart agent sessions whose MCP servers predate the upgrade. The release note says so. No schema files are committed, so nothing is regenerated beyond `.sqlx/` metadata (`python3 scripts/tooling/prepare-sqlx.py`, both `message-board-storage` and `collaboration-service`).

## 5. Proof

| Obligation | Proof |
| --- | --- |
| Each §4a event row writes the table's columns | storage tests per call site: create with Role, join, repeat join, join --replace (both kinds), handover, leave, leave --resolve, resolve, unresolve; assert the three columns on the stored activity |
| Rule 1: Role never changes without an event | the handover test asserts the target's latest barrier is the handover |
| Attribution, live | through the real storage path: a session reply after a handover reports `orchestrator`; a reply before it the earlier Role; a reply after a different-Role repeat join the new Role; after resolve, unresolve and rejoin the rejoined Role; a human reply none; a new main message the creator's Role; a human creator with a Role none |
| Lifetime stability (§6) | after each of those later events, every earlier reply still reads back its original Role |
| Attribution, upgrade gap (option B) | a migrated database where an open orchestrator's handover is unprovable: their next reply stores NULL and reads without `postedAsRole`; after they rejoin, replies report the rejoined Role |
| Review counterexamples | migrated databases reproducing each: surviving handover, then resolve, unresolve, reviewer rejoin, reply, advisor repeat join (reply stays unknown, not `orchestrator`); surviving `implementerReplaced`, then overwritten joins (no stale `implementer`); older proven replacement followed by an unprovable handover (new reply unknown, not `implementer`); legacy session root followed by its author's first join (root stays NULL) |
| Live and backfill agree | one sequence evaluated by the live write and by the migration's `UPDATE` gives the same `posted_from_activity` |
| Backfill is provable-only | migration test on a copied pre-migration database: a join overwritten by a repeat join, a join --replace whose joiner later rejoined, a surviving handover, an unprovable handover, a leave, a resolve, one identity on two roots, a participating human, session and human replies, a reply with no earlier event; assert every column and that `foreign_key_check` is clean |
| Reads fail closed | rows whose attributing event has an invalid Role, another author, another root, a later sequence, or no Role each return the corrupt-row error |
| Schema registration | opening a migrated store passes `validate_schema` |
| Gates | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the affected crates' tests, `python3 scripts/tooling/prepare-sqlx.py --check`, `git diff --check` |

## 6. Risks

- The backfill's handover/replace split depends on `thread_participants` closure columns surviving; where a rejoin cleared them the event stays unattributed, which §4a accepts as "before Participant history".
- A client older than the Host rejects messages carrying `postedAsRole` and reports a committed post as an unknown outcome (§4); the cutover restarts agent sessions after the Host.
- Threads active before the upgrade can show replies without a Role until their authors rejoin (option B).
