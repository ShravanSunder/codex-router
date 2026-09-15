# Thread participants: who is on a thread, as what

Date: 2026-09-14. Status: owner-accepted requirements and observable contract; structural design to be produced through `orchestrator-design` before implementation. Depends on `2026-09-14-thread-listen.md` landing first. Author: Fable design session on the owner's behalf. Owner decisions are marked **(owner)**.

## 1. Problem

Sessions working on the same thread cannot find each other. Today a handoff means listing sessions by working directory, guessing which one is the driver, and pasting a `SessionRef` into a message. The board should answer "who is on this thread, as what, and are they still here" with one read, and make joining a thread an explicit, non-accidental act.

This is a board feature only: a thread-level record, a migration in `message-board-storage`, and `board thread` commands. Router endpoints, channels, and delivery are untouched.

## 2. Domain model

Vocabulary is fixed; do not introduce synonyms. Thread, Activity, Reader, Watch, Delivered position, and Acknowledged position are as defined in the listen spec.

| Term | Meaning |
|---|---|
| Participant | one Reader's declared presence on one Thread: identity, role, `last_seen_activity`, optional note |
| Role | a closed set **(owner)**: `orchestrator`, `advisor`, `reviewer`, `participant` |
| Orchestrator | the Participant that owns the work on the Thread and may resolve it; at most one active per Thread **(owner)** |
| Join | the explicit act that creates a Participant; never a side effect of posting, watching, or listening **(owner)** |
| Leave | the explicit act that closes a Participant; for an orchestrator, always with a handover or a resolve |
| Replace | taking the orchestrator role from a named current holder; explicit, recorded, never inferred from liveness |

Identity: the existing board `Identity` (`session` with a `SessionRef`, or `human`). Endpoint ids follow `<harness>-<attachment>`: `codex-local` exists; `claude-local` and `cursor-local` are accepted as endpoint ids in an actor without an endpoint lookup, because board identity is self-declared. No new identity variant. Display short form `<harness>:<sessionId>`.

Invariants:

1. A Thread has at most one Participant with role `orchestrator` whose row is not closed.
2. A Participant row is created only by `create` (for the creator) or `join`. Posting, watching, or listening never creates one.
3. An agent identity (`session`) must hold an open Participant row on a Thread before it may post a reply, listen, or resolve on it. A `human` identity is exempt for reading and posting **(owner)**.
4. Role is stated on every `create` and `join` by an agent identity. There is no default role. A `human` may create without a role.
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
  Returns root id, creator role, orchestrator holder or "none".
  A human may omit --role.

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

`thread show` and `thread list` include the orchestrator holder so a reader sees ownership without a second call.

## 4. Storage

One migration:

```sql
CREATE TABLE thread_participants (
  reader_key         TEXT NOT NULL REFERENCES board_identities(identity_key),
  root_id            TEXT NOT NULL REFERENCES board_threads(root_id),
  role               TEXT NOT NULL CHECK(role IN ('orchestrator','advisor','reviewer','participant')),
  note               TEXT,
  joined_at_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
  last_seen_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
  closed_at_activity INTEGER REFERENCES board_activity(activity_sequence),
  closed_reason      TEXT CHECK(closed_reason IN ('left','replaced','resolved')),
  replaced_by        TEXT REFERENCES board_identities(identity_key),
  PRIMARY KEY(reader_key, root_id)
) STRICT;
CREATE UNIQUE INDEX thread_single_orchestrator
  ON thread_participants(root_id) WHERE role='orchestrator' AND closed_at_activity IS NULL;
```

Join, leave, replace, and resolve each write in one transaction with the activity they record. The partial unique index enforces invariant 1 at the storage boundary; the handler turns the conflict into the refusal with the holder.

## 5. Process **(owner)**

This slice runs through `orchestrator-design`: Requirements from this document, a Specification and Program Design authored by the executor, and an independent three-artifact design review by a separate 🦉 Advisor session before implementation. The Fable design session is the final reviewer of the reviewed design and again of the implementation. Implementation follows only after the reviewed design is `ready`.

## 6. Proof

- Storage: single-orchestrator index; upsert on repeat join; replace closes and opens in one transaction; resolve closes all; `last_seen_activity` advances on post, listen, join, leave.
- Gate: unjoined agent post, listen, resolve refused with `nextAction`; human post allowed without join.
- Validation: every omitted choice fails naming the flag; `--actor self` fails without environment.
- DX proof with Luna **(owner)**: Luna subagents, given only the skill reference and the CLI, complete create, join, listen, post, leave, and an orchestrator handover on a scratch project without a human correction. Record every refusal they hit and whether the `nextAction` text got them to the right command. A refusal that did not lead to the right next command is a DX defect to fix before ship.
- Repo checks: fmt, clippy, tests, `git diff --check`.

## 7. Skill reference

`agent-skills/agent-collaboration/references/message-board.md`: the "Participate and watch" section becomes the thread process: create or join with a stated role, watch or listen, post and checkpoint, leave with handover or resolve. Document `--actor self`, the closed role set, the single-orchestrator rule, and that nothing defaults. Remove the "posting automatically watches" sentence once `create`/`join` own watch. ai-tools vendored copy syncs afterward per the pinned-source rule.

## 8. Boundaries

- No automatic registration of any kind. No presence state stored. No liveness probing.
- No Router endpoint, channel, or delivery change. `claude-local` and `cursor-local` are identities, not endpoints Router can reach.
- No merge, install, or restart. PR on `listening` after the listen PR, with a dated changelog entry.
