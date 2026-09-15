# Thread Participants — Requirements

## Purpose

Agents and humans coordinating through a project board need one explicit, durable answer to who has joined a Thread, which Role each Participant holds, and which Participant currently owns orchestration. Presence must come from declared Thread participation and Activity sequence, without inferring liveness from Router state or silently registering an identity as a side effect.

## Consumers

- Session identities participating through the `agent-collaboration` CLI or SDK.
- Human identities creating, reading, posting to, or optionally joining Threads.
- Orchestrators handing work to another Participant or resolving a Thread.
- Reviewers and operators inspecting current and closed Participant records.
- Luna agents exercising the documented Thread process without hidden context.

## Authority

- Primary source: `docs/specs/2026-09-14-thread-participants.md`, including the 2026-09-14 Fable amendments made on the owner's behalf.
- Cohesion source: `docs/specs/2026-09-14-thread-listen.md` for Thread, Activity, Reader, Watch, Delivered position, Acknowledged position, Listen, and Batch.
- Repository constraints: root `AGENTS.md`, especially SQLx checked queries, STRICT tables, Rust validation for closed sets, and async/runtime boundaries.
- Current-behavior evidence: the message-board domain, storage, Control, client, and CLI sources at PR #58 head `904a1c2`.

## Authorized requirements

| ID | Requirement | Priority | Source |
| --- | --- | --- | --- |
| U1 | A Thread exposes its Participants, each Participant's Role, `last_seen_activity`, optional note, and closed state through one board read. | Must | Participants §§1–3 |
| U2 | A Role is always one of `orchestrator`, `advisor`, `reviewer`, or `participant`; a Participant never has a missing or invented Role. | Must | Participants §2, Fable decision 2026-09-14 |
| U3 | Join is explicit. Posting, watching, listening, reading, and unresolving never create or reopen a Participant. | Must | Participants §2 invariants 2, 4, 7 |
| U4 | An agent `thread create` states a Role and creates the root plus its Participant atomically. A human `thread create` may omit Role; it creates the root and stated Watch choice without creating a Participant. | Must | Participants §§2–3, Fable decision 2026-09-14 |
| U5 | Every `thread create` and `thread join` states exactly one of Watch or no Watch. There is no implicit Watch choice. | Must | Participants §§3, 7 |
| U6 | At most one open Participant is the Orchestrator for a Thread. Replacement is explicit and names the current holder; liveness never causes replacement. | Must | Participants §2 invariants 1, 6 |
| U7 | A session identity must have an open Participant before it posts a Thread reply, starts a Listen, or resolves the Thread. A human identity may read and post without a Participant; resolving still requires the Orchestrator Role. | Must | Participants §2 invariant 3; owner correction |
| U8 | Resolving a Thread closes every open Participant with reason `resolved`; unresolving does not reopen any Participant. | Must | Participants §2 invariant 7 |
| U9 | Leave closes the caller's Participant and unwatches the Thread. An Orchestrator can leave only by handing orchestration to a named joined Participant or resolving the Thread. | Must | Participants §§2–3 |
| U10 | Rejoining the same Reader and Thread updates that one Participant's Role or note and reopens it without creating a duplicate row. | Must | Participants §3 |
| U11 | `last_seen_activity` advances by Activity sequence when a Participant joins, posts, listens, leaves, is replaced, or is closed by resolution. Wall-clock time never establishes presence or ordering. | Must | Participants §2 invariant 5 |
| U12 | `--actor self` resolves a supported current session without guessing. Missing or ambiguous session evidence fails before board mutation and names the missing or conflicting input. | Must | Participants §3 |
| U13 | Participant and orchestration refusals return stable machine-readable kinds plus a `nextAction` that leads to the exact corrective command. Successful results return state and contain no instruction hint. | Must | Participants §3, owner decision |
| U14 | `thread show` and `thread list` expose the current Orchestrator holder, including the explicit absence of a holder. | Must | Participants §3 |
| U15 | No semantic choice defaults: actor, Thread or topic selection, Role when required, Watch choice, Listen mode and bound, acknowledgement choice, replacement identity, and Orchestrator handover or resolve choice are explicit. Missing choices name the corresponding flag. | Must | Participants §2 invariant 8 |
| U16 | Persistence follows repository SQLx conventions with a STRICT migration, checked queries, relational constraints, a storage-enforced single-Orchestrator rule, and Rust validation of closed sets on requests and stored rows. | Must | Participants §4; root `AGENTS.md` |
| U17 | The feature changes only board domain, storage, Control/client, CLI, and board skill guidance. It adds no endpoint, channel, delivery path, liveness probe, automatic registration, presence state, daemon, or external queue. | Must | Participants §§1, 8 |
| U18 | Thread Listen remains process-owned stdout delivery and, once this slice lands, refuses an unjoined session at the single request-handler entry gate established by PR #58. | Must | Participants §2 invariant 3; Listen §9 |
| U19 | A Luna DX run, given only the skill reference and CLI, completes create, join, listen, post, leave, and Orchestrator handover on a scratch project; every refusal records whether its `nextAction` led to the correct command. | Must | Participants §6, owner decision |
| U20 | The Participants slice is designed and independently reviewed before implementation, then receives Fable's final design review and later implementation review. | Must | Participants §5 |

## Confirmed boundary

In scope: Participant and Role contracts; create/join/leave/list commands; explicit Watch choice; optional Listen arming during Join; the session join gate for post/Listen/resolve; Orchestrator replacement, handover, and resolution; Activity-sequence presence; Thread show/list ownership projection; actionable refusals; skill guidance; SQLx migration and proof.

Out of scope: automatic registration, stored active/idle presence, liveness probing, Router endpoint discovery for self-declared Claude/Cursor identities, changes to message delivery, new daemons or queues, multi-project operations, merge, installation, release, or production restart.

## Settled tradeoffs

- Explicit Join costs one command and prevents an accidental Participant or Orchestrator.
- A single Orchestrator simplifies ownership and requires explicit replacement/handover during turnover.
- Activity sequence provides durable ordering without claiming real-time liveness.
- Humans remain outside the Participant registry until they opt in with a Role; this preserves non-null Role semantics.
- Closed sets remain evolvable Rust domain rules; SQLite enforces relationships and single-Orchestrator uniqueness.

## Derived transport constraints

- A present Participant note is trimmed descriptive metadata containing 1–16,384 UTF-8 bytes. This reuses the board's established descriptive-text budget and keeps Participant pages within the Control frame budget.
- Participant listing uses the existing bounded `Page<Participant>` transport with optional `--limit` and `--cursor`; omitted page sizing uses the established board page size. Pagination controls transport framing rather than a domain choice, so this does not create a Role, Watch, Listen, acknowledgement, replacement, or handover default.

These constraints are executor-authored Specification inputs derived from the current board contracts. They require the independent Advisor and Fable design reviews; they are not represented as owner decisions.
