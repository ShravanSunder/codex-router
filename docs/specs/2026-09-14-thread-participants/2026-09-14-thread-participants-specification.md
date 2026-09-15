# Thread Participants — Specification

## 1. Scope and authority

This Specification defines the observable Thread Participants contract. Its Requirements identity is [`2026-09-14-thread-participants-requirements.md`](./2026-09-14-thread-participants-requirements.md). Its governing product authority is [`../2026-09-14-thread-participants.md`](../2026-09-14-thread-participants.md), with the Thread Listen contract in [`../2026-09-14-thread-listen.md`](../2026-09-14-thread-listen.md).

The feature adds explicit Participants and Roles to the existing project board. It changes board domain, storage, Control/client, CLI, and board skill guidance. It does not add Router endpoints, channels, delivery targets, liveness checks, automatic registration, daemons, queues, or multi-project operations.

## 2. Observable domain contract

The vocabulary in the governing documents is normative: Thread, Activity, Reader, Watch, Delivered position, Acknowledged position, Listen, Batch, Participant, Role, Orchestrator, Join, Leave, and Replace.

### 2.1 Participant and Role

A Participant MUST represent one Reader's declared presence on one Thread. It MUST expose:

- `identity`: the existing board `Identity`;
- `role`: exactly one of `orchestrator`, `advisor`, `reviewer`, or `participant`;
- `lastSeenActivity`: an Activity sequence;
- optional `note`;
- its Join Activity sequence;
- either an open state or a closed state with closing Activity sequence and reason;
- `replacedBy` only when an Orchestrator was replaced.

A Participant MUST always have a Role. A present note MUST be trimmed, non-empty, contain no NUL, and contain at most 16,384 UTF-8 bytes. Omitting `--note` on a repeat Join MUST preserve the existing note. This slice provides no note-clearing operation.

The human-readable short form for a session identity MUST be `<harness>:<sessionId>`, derived from endpoint ids such as `codex-local` and `claude-local`. Machine output MUST preserve the complete `Identity`.

### 2.2 Orchestrator

A Thread MUST have at most one open Participant whose Role is `orchestrator`. A Thread MAY have no Orchestrator. Thread show, Thread list, Thread create, Thread join, and Participant list results MUST expose the current Orchestrator holder or explicit absence.

An open Orchestrator MUST NOT change to a non-Orchestrator Role through a repeat Join. The holder MUST first Leave with `--to`, Leave with `--resolve`, or be explicitly Replaced. This prevents a Role update from bypassing handover or resolution.

### 2.3 Activity sequence

`lastSeenActivity` MUST be the highest Activity sequence at which the Participant joined, posted, completed a Listen observation, left, was replaced, or was closed by resolution. Sequence alone decides recency. No wall-clock field or stored active/idle state may affect presence.

Join, Leave, Replace, and resolution MUST use Thread Activity sequence. Participant lifecycle Activity MUST be distinguishable from a message and a Thread-state change. It advances Participant state and notification observation, but MUST NOT become an ordinary unread Thread message or a Listen Batch message. The existing inbox remains limited to message and Thread-state Activity in this slice.

## 3. Journeys and commands

Every command in this section requires `--json`. Missing required choices MUST fail validation before a request is sent and name the omitted flag or exclusive flag group.

### 3.1 Create a Thread

```text
board thread create --topic-id <id> --actor <identity|self> --role <role>
  (--watch | --no-watch) --text-file <path> --json
```

For a session identity, `--role` MUST be present. One successful operation MUST post the root message, create or reopen the session's Participant with the stated Role, apply the explicit Watch choice, record the Join Activity, and return:

- the root message and its Activity sequence;
- the creator Participant Role;
- the current Orchestrator holder or `null`;
- the resulting Watch state.

For a human identity, `--role` MAY be omitted. In that form, the operation MUST post the root and apply the explicit Watch choice, MUST NOT create a Participant, and MUST return creator participation as `notJoined` and Orchestrator as `null`. A human who supplies `--role` receives the same Participant behavior as a session identity.

The CLI MAY generate the root message identifier before submission, following existing create-command identity behavior. That generated resource identity is not a domain choice.

Create MUST be atomic at the board storage boundary: failure before commit creates neither root, Participant, Watch state, nor Activity. An uncertain transport outcome MUST identify the generated root message so the caller can inspect it without repeating the create blindly.

### 3.2 Join

```text
board thread join --root-message-id <id> --actor <identity|self> --role <role>
  (--watch | --no-watch) [--replace <identity>] [--note <text>]
  [--listen once --max-wait <duration> (--acknowledge|--no-acknowledge)
   | --listen for <duration> (--acknowledge|--no-acknowledge)] --json
```

Join MUST require Role and Watch choice for both session and human identities. It MUST create or reopen at most one Participant for the `(Reader, Thread)` pair. A repeat Join MUST update the stated Role and a supplied note, preserve an omitted note, set `joinedAtActivity` and `lastSeenActivity` to the new Join Activity, clear prior closed fields, and never create a duplicate Participant row.

`--replace` MUST be accepted only with `--role orchestrator`. It MUST name the exact current Orchestrator identity. A matching Replace MUST close the current holder with reason `replaced`, set its `replacedBy` to the joining identity, and open or update the new Orchestrator in the same committed operation and Activity sequence. A stale or wrong holder MUST be refused without changing either Participant.

The joining identity MUST differ from the identity named by `--replace`. Self-Replace MUST be a mutation-free validation refusal. A current Orchestrator who wants to refresh its existing Participant uses ordinary repeat Join without `--replace`.

When `--listen` is present, all Listen mode, bound, and acknowledgement choices from the Listen Specification remain required. Join and Watch state MUST commit before the Listen waits. The command MUST first write and flush one successful Join result record, then use the existing Listen stdout stream for the joined Thread. That stream MUST produce the first Batch set or Timeout and, in Repeating mode, continue producing Batch sets for its lifetime. A setup failure after the flushed Join result is a later Listen error and MUST NOT obscure the committed Join. Batch flush, acknowledgement ordering, and exit codes remain the Listen contract.

### 3.3 Post

An existing `board message post --placement thread` by a session identity MUST be refused unless that identity has an open Participant on the selected Thread. A human identity remains exempt. Posting MUST NOT create, reopen, or change a Participant Role or Watch.

An existing `board message post --placement topic` by a session identity MUST be refused before mutation. Its `nextAction` MUST render `board thread create --topic-id <id> --actor <identity|self> --role <role> (--watch|--no-watch) --text-file <path> --json`, preserving the supplied topic and text-file path where available. A human identity MAY continue using topic placement; it creates the root without creating a Participant.

A successful Participant post MUST update that Participant's `lastSeenActivity` to the message Activity sequence in the same transaction as the message.

### 3.4 Listen

At entry to `board/threadListen`, every selected Thread MUST have an open Participant for a session Reader. A human Reader remains exempt. A refusal MUST occur before a Listen registration, Watch creation/reactivation, Delivered-position change, or wait begins.

A successful Listen observation MUST update each selected open Participant's `lastSeenActivity` to the highest Activity sequence the Listen observed for that Thread. Timeout and cancellation MUST NOT invent an Activity sequence; when the Thread's current sequence has not advanced, `lastSeenActivity` remains unchanged. Delivered-position and Acknowledged-position rules remain exactly as specified by Thread Listen.

Listening MUST NOT create or reopen a Participant.

### 3.5 Leave, handover, and resolve

```text
board thread leave --root-message-id <id> --actor <identity|self>
  [--to <identity> | --resolve] --json
```

A non-Orchestrator Participant MUST omit both `--to` and `--resolve`. A successful Leave MUST close that Participant with reason `left`, update its `lastSeenActivity` and `closedAtActivity` to the Leave Activity sequence, and deactivate its Watch in one committed operation.

An Orchestrator MUST state exactly one of `--to` or `--resolve`:

- `--to` MUST name a different open Participant on the same Thread. The operation MUST close the leaving Orchestrator with reason `replaced`, set `replacedBy` to the named Participant, promote that Participant to Orchestrator, advance both affected Participants to the Replace Activity sequence, and deactivate the leaving Participant's Watch in one commit.
- `--resolve` MUST resolve the Thread and close every open Participant with reason `resolved` at the resolution Activity sequence in one commit. It MUST deactivate the leaving Orchestrator's Watch; other Watch choices remain personal state and are not implicitly changed by resolution.

The existing Thread resolve operation MUST require a session identity to be the open Orchestrator. A human identity MAY resolve without a Participant and regardless of the current Orchestrator holder, preserving the human's control of the board. Either successful resolution MUST close every open Participant atomically.

Unresolving MUST NOT reopen any Participant or restore any Role.

### 3.6 List Participants

```text
board thread participant list --root-message-id <id>
  [--limit <1..100>] [--cursor <opaque>] --json
```

The result MUST use the established bounded `Page<Participant>` contract. Ordering MUST be deterministic by Reader identity key. A cursor MUST remain bound to the Thread and operation. Omitted page controls use the existing board transport page size; pagination does not choose Participant, Role, Watch, or lifecycle behavior. A page MAY contain fewer records than the requested limit when required to keep the complete Control response within its frame budget, and MUST return a cursor for the first record not emitted.

The result MUST include open and closed Participant records and the current Orchestrator holder or `null`. Rejoin reopens the same record, so lifecycle history beyond the current row is observed through Activity history.

## 4. Actor self-resolution

`--actor self` MUST resolve after the CLI has verified the selected collaboration service, because the resulting session `Identity` includes that service id.

- `CODEX_THREAD_ID` alone resolves to endpoint `codex-local`.
- `CLAUDE_CODE_SESSION_ID` alone resolves to endpoint `claude-local`.
- neither variable fails naming both supported inputs;
- both variables fail as ambiguous and name both inputs;
- empty or invalid values fail validation;
- no endpoint inventory lookup is performed.

The explicit JSON `Identity` form remains accepted. Cursor identities are accepted in explicit form with endpoint `cursor-local`; this slice defines no Cursor `self` environment variable.

## 5. Refusals and `nextAction`

Participant refusals MUST have stable machine-readable failure kinds, structured details, and a refusal-specific `nextAction`. The `nextAction` MUST carry enough structured information for the CLI to render the exact corrective command. Successful results MUST omit `nextAction`.

| Refusal | Required details | Corrective command intent |
| --- | --- | --- |
| session is not joined for post, Listen, or resolve | Thread, actor, allowed Roles | `board thread join ... --role <role> (--watch|--no-watch)` |
| session uses topic-placement post | topic, actor, text source | `board thread create ... --role <role> (--watch|--no-watch)` |
| Orchestrator already exists | Thread, holder identity, holder `lastSeenActivity` | repeat Join with `--role orchestrator --replace <holder>` |
| non-Orchestrator attempts resolve | Thread, actor, holder identity or absence | identify the current holder; no success-path hint |
| Orchestrator Leave omits handover/resolve | Thread, actor | repeat Leave with exactly one of `--to` or `--resolve` |
| handover target is not an open Participant | Thread, target | Join the target first, then retry handover |
| current Orchestrator attempts Role downgrade by Join | Thread, holder | Leave with handover/resolve or use explicit Replace from the successor |
| replacement names a stale holder | Thread, named holder, current holder or absence | refresh Participant list and retry with current state |
| replacement names the joining identity | Thread, actor | repeat Join without `--replace` |

Validation errors for omitted choices MUST name the CLI flag. Domain refusals MUST not mutate Participant, Watch, Thread, Delivered position, Acknowledged position, or Activity state.

## 6. Compatibility and cutover

This is a hard CLI and behavior cutover:

- Thread creation moves from generic top-level message posting to `board thread create` when Participant semantics are required.
- A session topic-placement post is refused and directed to `board thread create`; a human topic-placement post remains allowed and creates no Participant.
- Posting a Thread reply no longer changes Watch state.
- Creating or joining owns the explicit Watch choice.
- Existing Threads begin with no Participant records and no Orchestrator. Session identities must Join before their next post, Listen, or resolve after the feature lands.
- Existing Watches, Delivered positions, Acknowledged positions, messages, and Thread states remain valid.
- Participant rows are never backfilled from historical posts, Watches, Listens, or sessions.

## 7. Reliability and concurrency obligations

- Concurrent attempts to become Orchestrator MUST yield exactly one committed holder. Every loser MUST receive the current holder rather than a raw storage error.
- Replace, handover, and resolve MUST serialize against concurrent Role and lifecycle changes. A stale precondition MUST refuse without partial effects.
- Join, Leave, Replace, message post activity update, and resolution closeout MUST commit their Participant and Activity effects together.
- Stored unknown Role or closed-reason values MUST be rejected as invalid records; they MUST never be coerced.
- All Control responses, including Participant pages and refusal details, MUST respect the existing maximum frame size.
- Runtime waits remain cancellation-safe and use the existing bounded Listen machinery; this slice adds no blocking work to the async runtime.

## 8. Proof obligations

| Requirements | Observable proof |
| --- | --- |
| U1–U4, U10 | Domain and storage tests show non-null closed Roles, role-less human create without a Participant, agent create with a Participant, repeat Join reopening one row, and note preservation. |
| U5, U12, U15 | CLI/protocol validation tests show every omitted choice and missing/ambiguous `self` input fails with the named flag or environment variable before mutation. |
| U6, U8, U9 | Storage concurrency and transaction tests show a single Orchestrator, exact Replace, atomic handover, and resolution closing all open Participants. |
| U7, U18 | Real Control-path tests show unjoined session post/Listen/resolve refusals occur at entry, while human read/post/Listen/resolve without Join succeeds and human resolution closes every open Participant. |
| U11 | Storage tests show `lastSeenActivity` advances only by the relevant Activity sequences for Join, post, Listen, Leave, Replace, and resolution. |
| U13 | Protocol and CLI tests show refusal-specific structured `nextAction` commands and no `nextAction` on successful results. |
| U14 | Thread show/list and CLI tests show holder identity or `null`. |
| U16 | Migration tests prove STRICT shape, foreign keys, partial unique index, migration from current schema, and Rust rejection of unknown closed-set values; SQLx checked metadata passes. |
| U17 | Diff and schema review show no endpoint, channel, delivery, liveness, daemon, or queue changes. |
| U19 | A Luna run given only the skill reference and CLI completes create, Join, Listen, post, Leave, and handover on a scratch project; its log records every refusal and whether `nextAction` led to the correct next command. |
| U20 | The distinct Requirements, Specification, and Program Design receive an independent Advisor review and Fable's final design decision before implementation. |

Repository proof also requires formatting, Clippy, applicable tests, and `git diff --check`.
