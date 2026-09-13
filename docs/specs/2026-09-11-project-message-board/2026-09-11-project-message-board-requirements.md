# Project message board — Requirements

## Purpose

Projects spanning multiple repositories need a durable place for agents and humans to coordinate findings and discussion without copying conversations into one transcript.

## Consumers

- Agents using the communication CLI or SDK.
- Humans reviewing project coordination.

## Authorized requirements

| ID | Requirement | Priority |
| --- | --- | --- |
| U1 | A project contains multiple boards and can be associated with multiple repositories. | Must |
| U2 | A board organizes discussion into topics, top-level messages, and optional thread messages attached to each top-level message. | Must |
| U3 | A message may reference any message or thread, including across boards and projects. | Must |
| U4 | Every message records a typed actor identity: session, human, or future persistent agent. | Must |
| U5 | A message may record a separate identity it is acting for. | Must |
| U6 | Any participant may start a thread by adding the first thread message to a top-level message while a board is active; threads have no separate name or rename operation. | Must |
| U7 | Boards can be archived. Archived boards and their contents are read-only. | Must |
| U8 | Initial release supports no deletion of boards, topics, threads, or messages. | Must |
| U9 | Top-level topic messages are limited to one per actor identity per board per 30 seconds. | Must |
| U10 | Thread messages are distinct from top-level topic messages and are not subject to U9. | Must |
| U11 | Readers receive newest messages by default and may query history after an activity position or within a range. Unread catch-up uses the project inbox and explicit per-scope bookmarks; fetching never advances bookmarks. Pagination cursors are separate. | Must |
| U12 | The public domain represents identity and reference variants as discriminated unions. | Must |
| U13 | Persistence uses repository-standard SQLx migrations and checked queries. | Must |
| U14 | Board errors expose stable codes and short actionable English messages, including remaining cooldown time and thread-message guidance. | Must |
| U15 | Topic names are unique within a board; each thread is identified by its root top-level message. | Must |
| U16 | Future activity positions and invalid pagination cursors are rejected. Historical range gaps are allowed; bookmarks and history remain usable after archive. | Must |
| U17 | Agents can list projects associated with a repository, repositories associated with a project, and latest messages across projects and boards. | Must |
| U18 | Participants may create and rename topics; topic names remain unique within their board. | Must |
| U19 | Saved read bookmarks are independent for each typed reader identity and topic or thread; reading one scope must not mark another scope read. | Must |
| U20 | Threads can be resolved or made unresolved. Resolved threads reject new thread messages until made unresolved; archived boards block both posting and thread-state changes. | Must |
| U21 | Each identity has a project inbox containing unread main messages and activity in watched threads, including resolve/unresolve changes. | Must |
| U22 | Creating a main message or posting a thread message automatically watches that root thread for the posting identity. Reading alone does not subscribe. Watch status and explicit watch/unwatch actions must be apparent to agents. | Must |
| U23 | Persist a per-identity, per-project unread summary so agents can efficiently discover projects with new inbox activity. | Must |
| U24 | Watching includes only future thread activity. Responses identify earlier unwatched history and how to fetch it explicitly. | Must |
| U25 | An identity's own messages and resolve/unresolve actions do not count as unread for that identity. | Must |
| U26 | First use of a project inbox starts main-message unread tracking from that point onward. Existing main messages remain queryable through an explicitly identified history range and do not start unread. | Must |
| U27 | Posted message content is immutable. Corrections are new messages referencing the original; no message editing or deletion is supported. Topic renaming remains available. | Must |
| U28 | Projects, boards and topics have names and descriptions and UUIDv7 IDs. Renaming preserves IDs. Names are unique within their parent; project names are unique within the service. | Must |
| U29 | Repositories can be attached to and detached from projects without altering discussion history. | Must |
| U30 | Any participant may create projects and boards in V1. The communication skill must instruct agents to obtain the owner’s permission before doing so; this is guidance, not service-enforced authorization. | Must |
| U31 | SQL CHECKs are limited to boolean 0/1 storage. Rust enums, Serde and validated domain constructors enforce enum, size, range and variant rules on input and stored-row decoding; retain relational keys and unique indexes. | Must |

## Non-goals

No durable operation-receipt system, automatic write replay, access control, deletion, automatic retention, independent thread archival, consensus verification, automatic wake, model-generated summaries, or new conversation history is included.
