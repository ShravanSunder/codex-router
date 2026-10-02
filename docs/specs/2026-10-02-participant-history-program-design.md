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

**Main message attribution (design).** §4a rule 2 names replies. A Thread's main message is posted by a Participant whose Role is stated in the same request, so `create_thread` sets the main message's `posted_from_activity` to its `participantJoined` event with an `UPDATE` after that event is inserted (the foreign key is immediate, so the message cannot reference the event before it exists). A `create` without `--role` is human-only (`create_thread` refuses a session without one) and stays NULL. Reason: the owner's requirement is that a post never loses the Role it was written under, and the main message is a post.

**Replies.** `post_message`, for a `session` author with a `root_id`, runs after `require_open_participant` and before the `board_messages` insert:

```sql
SELECT activity_sequence FROM board_activity
WHERE root_id = ? AND participant_key = ? AND participant_role IS NOT NULL
ORDER BY activity_sequence DESC LIMIT 1
```

The Join gate guarantees a row for any new reply; if none exists the write fails as a storage invariant violation rather than storing NULL, because NULL means "before Participant history". Human authors and top-level posts without a `root_id` store NULL.

## 3. Migration and backfill

One migration, in the order §4a gives, inside sqlx's migration transaction. The backfill is plain SQL because every rule is a join over stored rows with nothing to validate in Rust; `foreign_key_check` after migrating covers every key it writes. Statements run in this order, each reading the results of the earlier ones.

1. **Join and leave.** `participantJoined` and `participantLeft`: `participant_key = actor_key`.
2. **Replaced events, split by proof (design).** An `orchestratorReplaced` / `implementerReplaced` event is one of two writes that share a kind, and only stored rows tell them apart:
   - *Handover*: the actor's `thread_participants` row on that root has `closed_at_activity = event` and `replaced_by` set. Then `participant_key = replaced_by`, `replaced_participant_key = actor_key`, `participant_role = 'orchestrator'`.
   - *Join --replace*: the actor's row has `joined_at_activity = event`, or another row on that root has `closed_at_activity = event` and `replaced_by = actor_key`. Then `participant_key = actor_key`, `participant_role` from the kind (`orchestrator` / `implementer`; the kind proves it), and `replaced_participant_key` = that other row's `reader_key` when present.
   - Neither proven (a later rejoin overwrote the rows): all three stay NULL. Guessing the actor would attribute a handover to the leaver.
3. **Roles on joins.** A `participantJoined` event gets the Role of the row whose `joined_at_activity` equals it, when that Role is not `orchestrator`, or when it is `orchestrator` and no `orchestratorReplaced` event on that root after the join is either attributed to this Participant or unattributed. A later handover rewrites the row's Role in place without moving `joined_at_activity`, so an `orchestrator` row only proves its join when no handover could have produced it. Earlier, overwritten joins stay NULL.
4. **Replies.** For a `session` author's reply (`root_id` set), `posted_from_activity` = the latest event on its root with `participant_key = actor_key`, `participant_role IS NOT NULL`, and `activity_sequence <=` the reply's own `threadMessageCreated` sequence. None found: NULL.
5. **Main messages.** For a `session` author's main message, `posted_from_activity` = the `participantJoined` event on that root whose actor is the author and whose sequence is the root's earliest participant event, when step 3 gave it a Role. `create --role` writes that event immediately after `mainMessageCreated`. Otherwise NULL.

`board_migration_tests.rs`' latest-version assertion moves to `202610020001`.

## 4. Reads and wire

`StoredMessageRow` gains `posted_as_role: Option<String>` from `LEFT JOIN board_activity pa ON pa.activity_sequence = m.posted_from_activity` (`pa.participant_role`). `Message` gains:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub posted_as_role: Option<ParticipantRole>,
```

Absent means "human author" or "before Participant history" (§4a rule 3). The field reaches every surface that carries `Message`: post, show, list, create results, search, and push records. `Message` has `deny_unknown_fields`, so an older client reading a newer Host's message rejects it; the control schema digest already changes with the type and turns that skew into the existing digest mismatch instead of a decode failure. No schema files are committed, so nothing is regenerated beyond `.sqlx/` metadata (`python3 scripts/tooling/prepare-sqlx.py`, both `message-board-storage` and `collaboration-service`).

## 5. Proof

| Obligation | Proof |
| --- | --- |
| Each §4a event row writes the table's columns | storage tests per call site: create with Role, join, repeat join, join --replace (both kinds), handover, leave, leave --resolve, resolve; assert the three columns on the stored activity |
| Rule 1: Role never changes without an event | the handover test asserts the target's latest Role-bearing event is the handover |
| Rule 2: replies and main messages store their attributing event | a session reply after a handover reports `orchestrator`; a reply before it reports the earlier Role; a human reply reports none; the main message reports the creator's Role |
| Rule 3: reads omit unknown Roles | message JSON for a human reply and a pre-history reply has no `postedAsRole` key |
| Invalid stored Role fails closed | a read over a row whose `participant_role` is not a Role returns the corrupt-row error |
| Backfill is provable-only | migration test on a copied pre-migration database seeded with: a join later overwritten by a repeat join, a join --replace, a handover whose rows survive, a handover whose leaver later rejoined (unprovable), a leave, a resolve, session and human replies, a reply with no earlier event; assert every column and that `foreign_key_check` is clean |
| Schema registration | opening a migrated store passes `validate_schema` |
| sqlx metadata | `python3 scripts/tooling/prepare-sqlx.py --check` |

## 6. Risks

- The backfill's handover/replace split depends on `thread_participants` closure columns surviving; where a rejoin cleared them the event stays unattributed, which §4a accepts as "before Participant history".
- A Host and client on different versions disagree on `Message`; the control schema digest refuses the pairing, as for every protocol change.
