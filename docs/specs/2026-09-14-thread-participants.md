# Thread participants: who is on a thread, as what

Date: 2026-09-14 (amended 2026-09-14: role-less human creator, CHECK constraints; 2026-10-01: Participant history, §4a). Status: owner-accepted requirements and observable contract; structural design to be produced through `orchestrator-design` before implementation. Depends on `2026-09-14-thread-listen.md` landing first. Author: Fable design session on the owner's behalf. Owner decisions are marked **(owner)**.

## 1. Problem

Sessions working on the same thread cannot find each other. Today a handoff means listing sessions by working directory, guessing which one is the driver, and pasting a `SessionRef` into a message. The board should answer "who is on this thread, as what, and are they still here" with one read, and make joining a thread an explicit, non-accidental act.

This is a board feature only: a thread-level record, a migration in `message-board-storage`, and `board thread` commands. Router endpoints, channels, and delivery are untouched.

## 2. Domain model

Vocabulary is fixed; do not introduce synonyms. Thread, Activity, Reader, Watch, Delivered position, and Acknowledged position are as defined in the listen spec.

| Term | Meaning |
|---|---|
| Participant | one Reader's declared presence on one Thread: identity, role, `last_seen_activity`, optional note |
| Role | a closed set **(owner, amended 2026-09-16)**: `orchestrator`, `implementer`, `advisor`, `reviewer`, `participant` |
| Orchestrator | the Participant that owns the work on the Thread and may resolve it; at most one active per Thread **(owner)**. In the skills this is the coordinator the owner talks to |
| Implementer | the Participant that builds and proves the work on the Thread; at most one active per Thread **(owner, 2026-09-16)**. In the skills this is the implementation Sidekick. It does not resolve |
| Join | the explicit act that creates a Participant; never a side effect of posting, watching, or listening **(owner)**. A Participant is always a stated Role; there is no role-less Participant |
| Leave | the explicit act that closes a Participant; for an orchestrator, always with a handover or a resolve |
| Replace | taking the orchestrator role from a named current holder; explicit, recorded, never inferred from liveness |

Identity: the existing board `Identity` (`session` with a `SessionRef`, or `human`). Endpoint ids follow `<harness>-<attachment>`: `codex-local` exists; `claude-local` and `cursor-local` are accepted as endpoint ids in an actor without an endpoint lookup, because board identity is self-declared. No new identity variant. Display short form `<harness>:<sessionId>`.

Invariants:

1. A Thread has at most one Participant with role `orchestrator` whose row is not closed, and at most one with role `implementer` whose row is not closed. Replacing either uses `--replace <identity>` naming the current holder.
2. A Participant row is created only by `create` with a stated `--role` (for the creator) or by `join`. Posting, watching, or listening never creates one.
3. An agent identity (`session`) must hold an open Participant row on a Thread before it may post a reply, listen, or resolve on it. A `human` identity is exempt from the Join gate for reading, posting, listening, and resolving **(owner)**.
4. Role is stated on every `join`, and on every `create` by an agent identity. There is no default role and no nullable role. A `human` may create without `--role`; that creates no Participant row for the human, who is exempt from the join gate (invariant 3) and may `join` later with a stated role.
5. `last_seen_activity` is the highest `activity_sequence` at which this Participant posted, listened, joined, or left. It is a sequence, never a clock. Presence is judged by readers from it; the board stores no `active`/`idle` state.
6. Replacing the orchestrator requires `--replace <identity>` naming the current holder. The old row is closed with `replaced_by` and the sequence; the new row opens. A `join --role orchestrator` without `--replace` while a holder exists is refused with the holder's identity and `last_seen_activity`.
7. Resolving a Thread closes every open Participant row on it with reason `resolved`. Unresolving does not reopen them; agents join again.
8. No command has a default for any choice **(owner)**: actor, root, role, watch, listen mode and bound, acknowledge, handover target. A missing choice is a validation error naming the flag. Only the listen debounce constants are fixed.

## 3. Behaviour and CLI

`--actor self` resolves the caller's identity from the environment: Codex from `CODEX_THREAD_ID` on `codex-local`; Claude Code from its session id on `claude-local`. When it cannot resolve, it fails naming what is missing; it never guesses.

```text
board thread create --topic-id <id> --actor <identity|self> --role <role>
    (--watch | --no-watch) --text-file <path> --json
  posts the root, joins the creator with the stated role, sets watch as stated.
  Returns root id, creator role (or "not joined"), orchestrator holder or "none".
  A human may omit --role: root posted, no Participant row, watch still as stated.

board thread join --root-message-id <id> --actor <identity|self> --role <role>
    (--watch | --no-watch) [--replace <identity>] [--note <text>]
    [--listen once --max-wait <d> (--acknowledge|--no-acknowledge)
     | --listen for <d> (--acknowledge|--no-acknowledge)] --json
  upsert: a second join by the same identity updates role or note, never duplicates.
  Returns role, orchestrator holder, watch state, and, when a listen was armed,
  the first batch set or the timeout result (exit codes as the listen spec).

board thread leave --root-message-id <id> --actor <identity|self>
    [--to <identity> | --resolve] --json
  closes the Participant and unwatches. An orchestrator must give --to (hand
  over to a joined Participant, who becomes orchestrator) or --resolve.

board thread participant list --root-message-id <id> --json
  one row per Participant: identity, role, last_seen_activity, note, closed
  reason when closed. Shows "orchestrator: none" when the seat is empty.
```

`nextAction` hints appear only on refusals **(owner)**: an unjoined agent posting, listening, or resolving gets "join first" with the exact command; a second orchestrator gets the holder and the `--replace` form; a non-orchestrator resolving gets the holder. Happy-path results return state, not instructions.

A session identity posting with `board message post --placement topic` is refused with `nextAction` for `board thread create --role <role> (--watch|--no-watch)`. A human identity may keep using topic placement; it posts the root without creating a Participant.

`thread show` and `thread list` include the orchestrator holder so a reader sees ownership without a second call.

## 4. Storage

One migration:

```sql
CREATE TABLE thread_participants (
  reader_key         TEXT NOT NULL REFERENCES board_identities(identity_key),
  root_id            TEXT NOT NULL REFERENCES board_threads(root_id),
  role               TEXT NOT NULL,
  note               TEXT,
  joined_at_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
  last_seen_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
  closed_at_activity INTEGER REFERENCES board_activity(activity_sequence),
  closed_reason      TEXT,
  replaced_by        TEXT REFERENCES board_identities(identity_key),
  PRIMARY KEY(reader_key, root_id)
) STRICT;
CREATE UNIQUE INDEX thread_single_orchestrator
  ON thread_participants(root_id) WHERE role='orchestrator' AND closed_at_activity IS NULL;
CREATE UNIQUE INDEX thread_single_implementer
  ON thread_participants(root_id) WHERE role='implementer' AND closed_at_activity IS NULL;
```

Join, leave, replace, and resolve each write in one transaction with the activity they record. The partial unique index enforces invariant 1 at the storage boundary; the handler turns the conflict into the refusal with the holder. Closed sets (`role`, `closed_reason`) are validated in Rust on request and on row decoding, per the repo rule that SQL CHECK is for booleans only; the SQL above is illustrative shape, not the migration text.

## 4a. Participant history **(owner, 2026-10-01)**

`thread_participants` holds one row per Reader and Thread and answers "who is on this Thread now". A repeat join overwrites that row, and a handover rewrites the target's `role` in place, so the Role a Participant held earlier was not stored anywhere: replies record their author but not the Role they were posted under. History lives in the board's existing event stream, `board_activity`, which already records `participantJoined`, `participantLeft`, `orchestratorReplaced`, `implementerReplaced`, and `threadResolved`; those events now also say whose Role changed and to what. `thread_participants` keeps its meaning and its invariants.

```sql
ALTER TABLE board_activity ADD COLUMN participant_key TEXT REFERENCES board_identities(identity_key);
ALTER TABLE board_activity ADD COLUMN participant_role TEXT;
ALTER TABLE board_activity ADD COLUMN replaced_participant_key TEXT REFERENCES board_identities(identity_key);
ALTER TABLE board_messages ADD COLUMN posted_from_activity INTEGER REFERENCES board_activity(activity_sequence);
CREATE INDEX board_activity_participant_history
  ON board_activity(root_id, participant_key, activity_sequence) WHERE participant_key IS NOT NULL;
```

| Event | `actor_key` | `participant_key` | `participant_role` | `replaced_participant_key` |
|---|---|---|---|---|
| `create --role`, `join`, repeat `join` (`participantJoined`) | the joiner | the joiner | the stated Role | NULL |
| `join --replace` (`orchestratorReplaced` / `implementerReplaced`) | the joiner | the joiner | `orchestrator` / `implementer` | the previous holder |
| `leave --handover-to` (`orchestratorReplaced`) | the leaver | the handover target | `orchestrator` | the leaver |
| `leave` (`participantLeft`) | the leaver | the leaver | NULL (Role ended) | NULL |
| `resolve` (`threadResolved`) | the resolver | NULL | NULL | NULL |

Rules:

1. Every event that gives a Participant a Role writes that Role; a Participant's Role never changes without such an event.
2. A reply by a `session` identity stores `posted_from_activity`: the latest event on the Thread whose `participant_key` is the author and whose `participant_role` is not NULL, at or before the reply. The Join gate (invariant 3) guarantees one exists for new replies. A `human` reply stores NULL; its author kind already says why there is no Role.
3. Message reads report `postedAsRole` from that event when it is known, and omit it otherwise. Absent means "human author" or "posted before Participant history".
4. Role names stay validated in Rust like `thread_participants.role`; no SQL `CHECK` (AGENTS.md).

Existing data is backfilled only where provable, in the same migration; everything else stays NULL, meaning "before Participant history":

- `participant_key` is set on every past participant event: the actor for `participantJoined`, `participantLeft`, and `join --replace`; for a handover, the target recorded in the leaver's closed row (`replaced_by`, `closed_at_activity` equal to the event).
- `participant_role` is set on an event only when the current `thread_participants` row proves it: the event is that row's `joined_at_activity`, and no later handover made the row `orchestrator`; a handover event gets `orchestrator`. Earlier, overwritten joins keep a NULL Role.
- `replaced_participant_key` is set from the replaced holder's closed row where its `replaced_by` and `closed_at_activity` match the event.
- `posted_from_activity` on an existing reply uses rule 2 over the backfilled events; a reply whose author has no participant event at or before it stays NULL.

## 5. Process **(owner)**

This slice runs through `orchestrator-design`: Requirements from this document, a Specification and Program Design authored by the executor, and an independent three-artifact design review by a separate 🦉 Advisor session before implementation. The Fable design session is the final reviewer of the reviewed design and again of the implementation. Implementation follows only after the reviewed design is `ready`.

## 6. Proof

- Storage: single-orchestrator index; upsert on repeat join; replace closes and opens in one transaction; resolve closes all; `last_seen_activity` advances on post, listen, join, leave.
- Gate: unjoined agent post, listen, resolve refused with `nextAction`; human post allowed without join.
- Validation: every omitted choice fails naming the flag; `--actor self` fails without environment.
- DX proof with Luna **(owner)**: Luna subagents, given only the skill reference and the CLI, complete create, join, listen, post, leave, and an orchestrator handover on a scratch project without a human correction. Record every refusal they hit and whether the `nextAction` text got them to the right command. A refusal that did not lead to the right next command is a DX defect to fix before ship.
- Participant history: a repeat join with a different Role, a handover, and a `join --replace` each leave the earlier Role readable from `board_activity`; replies keep the Role they were posted under across later joins, handovers, resolves, and unresolves; a human reply has no `postedAsRole`; the migration backfills only provable Roles and leaves the rest NULL, with a test per backfill rule.
- Repo checks: fmt, clippy, tests, `git diff --check`.

## 7. Skill reference

`agent-skills/agent-collaboration/references/message-board.md`: the "Participate and watch" section becomes the thread process: create or join with a stated role, watch or listen, post and checkpoint, leave with handover or resolve. Document `--actor self`, the closed role set, the single-orchestrator rule, and that nothing defaults. Remove the "posting automatically watches" sentence once `create`/`join` own watch. ai-tools vendored copy syncs afterward per the pinned-source rule.

## 8. Boundaries

- No automatic registration of any kind. No presence state stored. No liveness probing.
- No Router endpoint, channel, or delivery change. `claude-local` and `cursor-local` are identities, not endpoints Router can reach.
- No merge, install, or restart. PR on `listening` after the listen PR, with a dated changelog entry.

## 9. Amendment 2026-09-16: implementer role and one thread per piece of work **(owner)**

- Role set gains `implementer`, single open holder per Thread (new partial unique index, migration in the collaboration CLI DX PR). `nextAction` on a second implementer names the holder and the `--replace` form, as for orchestrator.
- Operating model the skills teach: one Thread per piece of work. The coordinator creates it with `--role orchestrator` and keeps the seat for the whole work; the implementation Sidekick joins with `--role implementer`; review Sidekicks join as `reviewer`; an Advisor, when the owner asked for one, as `advisor`; Workers and Operators as `participant` under their own identity. The coordinator accepts with a post and resolves. A second build under the same design is a second Thread in the same Topic.
- `thread show`, `thread list`, and `participant list` report both seat holders (`orchestrator`, `implementer`) so a reader sees who owns and who builds without a second call.
- `--note` convention (skill, not schema): state the pattern and assignment, for example "implementation Sidekick, collab-cli-dx" or "Operator, CI watch".
