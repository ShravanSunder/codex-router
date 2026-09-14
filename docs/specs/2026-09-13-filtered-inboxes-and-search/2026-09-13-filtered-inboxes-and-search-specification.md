# Filtered inboxes and search — Specification

Governing [Requirements](./2026-09-13-filtered-inboxes-and-search-requirements.md). The [existing board specification](../2026-09-11-project-message-board/2026-09-11-project-message-board-specification.md) governs behavior not explicitly changed here.

## Attention model

Project, board, and topic selections apply only to top-level messages. Thread watches are independent reader state. A message's references link to other messages or threads; references neither change its placement nor establish watches.

```text
Selected project / board / topic -> top-level message candidates --+
                                                                 |
Independent active thread watches -> thread activity candidates --+
                                                                 v
                                                apply read mode and deduplicate
                                                                 |
                                                        order, then paginate
```

R1 / C1 (F1, F2, F7): an inbox view MUST select matching top-level messages and independently include activity from the reader's watched threads. An unwatched thread's messages MUST NOT become eligible merely because its root is inside the selected project, board, or topic. References do not expand eligibility. Watched threads outside the selected scope remain eligible within the selected board service; this is not federation across services.

R2 / C2 (F2): changing the top-level filter MUST NOT create, stop, resume, or reset any thread watch. Existing explicit watch/unwatch operations and automatic watching on a successful post retain their behavior. Reading does not watch.

R9 / C9 (F9): every unread and latest fetch MUST explicitly supply one project, board, or topic selection, including continuation requests. Missing selection is a field-validation error; it MUST NOT fall back to a previous request, saved default, or an inferred workspace. The service MUST NOT persist a default selection. Concurrent requests may select different scopes without changing each other or thread watches. Continuations must repeat the cursor-bound selection. Existing project initialization boundaries, bookmarks, and watches remain persisted independently.

The result contains both sources once per activity/message identity. A watched thread's root top-level message is not itself a thread message: watching it does not bypass the top-level filter for that root. Clients can inspect the root through the existing show operation.

## Unread, latest, and first use

R3 / C3 (F3, F4): unread and latest are distinct reader-aware views using C1 selection. Unread returns eligible unacknowledged activity oldest first, including resolve/unresolve activity in watched threads. Latest returns messages newest first regardless of acknowledgement, including the reader's own messages and older messages. Latest is message context, not a list of thread-state events. Neither mode acknowledges anything.

For unread top-level messages, existing first-use and topic-bookmark boundaries and self-activity exclusion still apply. Watched-thread unread activity retains its watch-start boundary, thread bookmark, and self-activity exclusion. “Always included” does not mean already-acknowledged thread activity reappears as unread.

R4 / C4 (F4): the first unread inbox fetch for the selected scope's project MUST capture the current committed activity sequence as that reader/project's top-level start. It MUST NOT use the session creation timestamp or repeatedly advance the boundary. Board/topic selections resolve to their containing project for this purpose. Earlier top-level history remains available through explicit history and latest reads.

Selecting another board/topic in an already initialized project preserves that project boundary. Initializing another project creates that project's boundary without changing the first. Existing watched-thread unread activity survives initialization, including threads outside the selected top-level scope. Latest and search do not initialize top-level tracking.

Example: activity 100 exists when reader A first fetches topic T's unread inbox. T's top-level messages through 100 are history. A later message at 101 is eligible unless self-authored or acknowledged. A thread watched earlier can still supply eligible activity below 100. Repeating the fetch at 150 does not move the start from 100 to 150.

## Pagination and acknowledgement

R5 / C5 (F1–F4; baseline U11/U16/U19): filtering and deduplication MUST occur before the page limit. Preserve existing limits (default 50, 1–100), Control frame bounds, and byte-budget continuation without truncating message content.

Each page echoes the reader, top-level scope, mode, ordering, captured upper activity position, records, and nullable cursor. Items retain their real project/board/topic and thread identity; an out-of-scope watched item must not be presented as belonging to the selected scope. This applies to resolve/unresolve events as well as messages. Inbox activity retains its exact acknowledgement scope.

Cursors bind service/database identity, operation, reader, normalized scope, mode, upper bound, and last ordering position. Changing a bound query input requires a fresh read. Later arrivals do not enter an existing page chain. Invalid/foreign/mismatched cursors fail explicitly. No per-page server worker is retained.

Unread continuation retains existing re-evaluation semantics: acknowledgement and unwatch may remove remaining activity; a new watch's future-only boundary cannot inject eligible older unread activity behind the cursor. Latest continuation also reads current watch eligibility within its fixed upper bound and after its last ordering position. Watch changes do not invalidate its cursor or force a restart. A continuation does not revisit positions already passed: messages newly eligible at those positions appear on a fresh latest fetch. No stable watch-membership snapshot across pages is promised.

Acknowledgement remains topic-top-level or thread-specific. No project-wide or board-wide acknowledge operation is introduced. Acknowledging a newest-first page's maximum position would acknowledge earlier messages in that scope too; fetching latest MUST NOT do that automatically. Project unread summaries remain summaries of actual project activity, not a claim that every possible filtered view has records. An out-of-project watched item is acknowledged against its own project through its existing scope.

## Separate search surfaces

R6 / C6 (F5): discovery search MUST find projects, boards, and topics by textual query and expose a resource-kind filter and applicable ancestor filters. Results identify kind, stable ID, name, description, and ancestors sufficiently to select a board/topic without guessing which parent owns it.

R7 / C7 (F6): message search MUST match message text and support project, board, topic, or specific-thread filtering and top-level-only, thread-only, or both message kinds. It MUST search messages in unwatched threads when the requested scope/kind includes them. Each result includes the message's placement, location, activity sequence, references, and root identity when it is a thread message. Reference-target text is not the source message's text.

Search is independent of unread state and thread watches and MUST NOT change either. Existing show/history operations provide surrounding context. Search does not recursively include referenced targets. R6/R7 use literal substring matching, case-insensitive for ASCII characters and exact for other characters. There is no regex, stemming, synonym expansion, or relevance ranking. Location filters are strict: watched threads outside a search scope are not included. Discovery matches name or description; message search matches message text and returns newest-first results. Discovery uses stable kind then immutable-ID ordering. Queries are trimmed, nonempty, and bounded to 256 UTF-8 bytes.

Search excludes archived boards and their contents unless `includeArchived` is explicitly true. Project discovery records are not archived and remain eligible independently of their boards. An explicitly targeted archived board/topic/thread still requires archived inclusion; its ID does not override the filter. Archive eligibility is re-evaluated on each page. Newly archived hits disappear from remaining pages without invalidating the cursor. Changing the request’s inclusion flag requires a fresh query because that flag is cursor-bound.

```text
Agent or human CLI consumer -- discover / search / inbox / watch --> Board system
SDK consumer --------------- typed Control requests -------------> Board system
                           <-- typed records, cursors, errors -----

Search finds locations/content; inbox delivers selected attention.
Neither surface starts a new agent or sends an automatic wake.
```

## Sixty-second top-level cooldown

R8 / C8 (F8): a successful top-level post MUST prevent another top-level post by the same actor identity in the same board for 60 seconds. The window spans all topics in that board. Other identities and other boards have independent windows. `actingFor` does not replace the actual actor for this comparison. Thread messages remain exempt.

At exactly 60 seconds the next top-level post is permitted, subject to other existing validation. Rejections retain `topLevelMessageCooldown`, integer `retryAfterSeconds` rounded upward, and `postThreadMessage` as the next action. Required guidance conveys: “Wait N seconds before another top-level message in this board, or add a thread message to an existing unresolved thread.” It must not imply that resolved or archived threads accept posts. A rejected post does not consume cooldown, create a message, or alter watches. A thread post does not extend the top-level window.

## Failure and compatibility boundaries

All new public contracts use existing typed camelCase Control schemas, closed variants, field limits, structured failures, and CLI JSON semantics. Invalid IDs, empty/invalid queries, incompatible filters, nonexistent resources, and malformed cursors fail with actionable field/resource errors, not silently broadened searches. Domain decoding continues to reject invalid stored records. Search input is text data, not executable query syntax.

Reads can return bounded partial pages only with continuation; a failed read does not masquerade as an empty success. First unread initialization remains a stateful operation and retains uncertain-outcome inspection guidance; no automatic write replay is introduced. Cancellation must not leave partially initialized state. Ordinary latest/search reads are nonmutating.

The API/CLI cutover must be coordinated through the existing schema and client surfaces. No legacy alias or parallel behavior path is required. Existing persisted watches, bookmarks, activity positions, message references, and cooldown timestamps are preserved. The new duration applies to the retained last successful post time. Publishing a new contract does not authorize restarting production.

## Requirement-to-proof coverage

| Requirements / problem / outcome | Contract | Observable proof |
| --- | --- | --- |
| F1, F2, F7 / P1 / O1 | R1–R2, C1–C2 | V1: real storage and public-client reads across two projects, boards, topics; out-of-scope watched messages and resolve/unresolve events included with their real project/board/topic, unwatched thread messages excluded, references do not expand membership; watches unchanged after filter changes. |
| F3, F4 / P2 / O2 | R3–R4, C3–C4 | V2: old/new/self messages, first-use race, repeat fetch, prior watch, restart, latest without initialization; verify boundaries and records. |
| F1–F4 / P1–P2 / O1–O2 | R5, C5 | V3: interleaved matching/nonmatching activity, overlap, page and byte limits, fresh arrivals, ack/continue, unwatch, foreign/changed cursors; no duplicates or omissions for stable eligibility. With watch changes, verify continuation stays valid, only remaining positions are considered, and a fresh latest fetch includes newly eligible messages at previously passed positions. |
| F5 / P3 / O3 | R6, C6 | V4: public discovery search returns distinct resource kinds and ancestors with filters, renames, archives, empty results, and bounded continuation. Verify literal substring/ASCII-case matching and strict filters; archive transitions preserve cursors while removing remaining excluded hits. |
| F6 / P3 / O3 | R7, C7 | V5: text found inside unwatched threads; kind/location restrictions, reference-only nonmatch, special characters, own/acknowledged messages, no read/watch state change. Verify literal substring/ASCII-case matching and strict filters; archive transitions preserve cursors while removing remaining excluded hits. |
| F8 / P4 / O4 | R8, C8 | V6: real-store 60-second boundary, cross-topic same-board rejection, independent actors/boards, exempt thread posts, rounded actionable error, concurrent attempts and rollback. Public CLI transcript demonstrates the guidance. |
| F9 / P5 / O5 | R9, C9 | V7: missing selection rejected for fresh and continuation fetches; alternating/concurrent scopes remain independent; restart does not restore a default; watch, bookmark, and initialization state persist normally. |

These proofs must traverse real storage and the public Control/CLI path where those interactions are claimed. Performance measurement must include large nonmatching scopes and thread traffic because a bounded output page alone does not prove bounded query cost under the existing serialized store.
