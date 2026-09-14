# Filtered inboxes and search — Program Design

Governing [Requirements](./2026-09-13-filtered-inboxes-and-search-requirements.md) and [Specification](./2026-09-13-filtered-inboxes-and-search-specification.md). The existing [board Program Design](../2026-09-11-project-message-board/2026-09-11-project-message-board-program-design.md) supplies process, transport, transaction, schema, and identity ownership.

## Composition

Top-level scope selection and thread-watch membership are two independent inputs to one result query. Neither is implemented by creating copies of inbox messages. The board store remains authoritative for eligibility, ancestry, pagination, and initialization.

```text
CLI / SDK consumer
  -> typed request: reader + top-level scope + read mode + page
  -> existing Control transport and board dispatch
  -> existing serialized BoardStore
       resolve scope ancestry
       read top-level candidates and independent watch candidates
       apply unread/history rules, combine, order, bound output
  <- typed page / structured failure along the same awaited path
```

This realizes C1–C5 without a daemon, polling worker, inbox-copy table, or message fan-out queue. C9 requires explicit scope on every request. The typed request owns that selection; no preference table, saved-filter operation, or default-selection lookup is introduced.

## Existing source and owners

The source baseline is commit `6af9ddfe39955a2cea4c43e400e92d3104a6b0ee`; paths below describe current code, not executed runtime proof.

| Owner / source | Current responsibility | Target responsibility and reason to change |
| --- | --- | --- |
| [CLI board preparation](../../../crates/agent-collaboration/src/board_commands/board_preparation.rs) | Builds project-only inbox requests and history requests. | Parse one explicit top-level scope and mode; separate discovery/content search commands. CLI ergonomics only, no eligibility filtering. |
| [Domain operations](../../../crates/message-board/src/board_operations/inbox_operations.rs), [message types](../../../crates/message-board/src/board_messages.rs) | Project-only inbox request, unread-only page, typed placement/reference/watch contracts. | Closed top-level scope and inbox-mode types, typed search requests/results; distinct acknowledgement and cursor types. |
| [SDK](../../../crates/collaboration-client/src/board_operations.rs) | Typed async RPC; inbox fetch uses mutation-style uncertainty because it initializes tracking. | Preserve uncertainty handling for unread first use; latest/search use nonmutating calls. Never post-filter pages. |
| [Control schema](../../../crates/collaboration-protocol/src/control_schema_document.rs), [dispatch](../../../crates/collaboration-service/src/board_request_dispatch.rs) | Typed method registration, request validation, mutex acquisition, awaited store call and structured result/error. | Register revised inbox and separate search methods. Keep admission, errors, and lock ownership. |
| [Inbox store](../../../crates/message-board-storage/src/inbox_records.rs) | Immediate transaction; project initialization, project-limited two-branch eligibility, oldest-first cursor. | Resolve project/board/topic scope; constrain only top-level branch; union independent watches, add latest mode and mode-specific cursor validation. |
| [History reads](../../../crates/message-board-storage/src/message_history_reads.rs) | Latest/range/after reads; project/topic top-level-only, board all-message history. | Preserve raw-history behavior. Reader-aware latest is an inbox view, so it does not inherit board history's all-thread behavior. |
| [Storage support](../../../crates/message-board-storage/src/storage_support.rs) | Start-boundary validation, persisted project summary, exact-scope bookmarks. | Reuse invariants; validate all projects contributing watch activity, not merely selected top-level project. Per-project summaries keep project-local meaning. |
| [Message write operations](../../../crates/message-board-storage/src/message_write_operations.rs), [domain failures](../../../crates/message-board/src/board_failures.rs) | Atomic 30-second actor/board cooldown, post/watch/activity, rounded guidance. | Change interval to 60 seconds at the existing owner; retain timestamp rows, actor/board key, thread exemption, rollback and error shape. |

CLI and SDK depend on domain contracts through the existing client exports. Domain does not depend on SQLx or CLI; storage does not depend on SDK. Host still opens, injects, and closes the same store. No caller opens SQLite directly.

## Changed paths

```text
Inbox, current:
CLI prepare_inbox -> SDK board_inbox_fetch -> Control board/inboxFetch
  -> dispatch -> BoardStore::fetch_inbox
  -> initialize selected project; select project-local unread activity
  <- InboxFetchResult or BoardError

Inbox, proposed:
[changed] CLI/domain carry top-level scope + mode
[unchanged] SDK -> Control -> awaited dispatch under existing store mutex
[changed] store resolves scope project and selects two independent branches
[unchanged] unread first-use writes only the selected project start/summary
[added] latest branch reads context without initialization/bookmark writes
[changed] result/cursor echo scope and mode; records retain actual location
  <- typed result/error through the same owners

Search, proposed only (no predecessor search method):
[added] CLI discovery search / message search -> matching typed SDK methods
[added] schema registration -> existing Control dispatch
[added] store search responsibilities -> existing metadata/messages/activity
  <- typed hits and cursor / existing structured failures

Cooldown:
[unchanged] message post -> store immediate transaction -> actor/board check
[changed] minimum interval 30,000 ms -> 60,000 ms
[unchanged] post + automatic watch + activity + summary commit together
  <- message/watch result or rounded cooldown guidance
```

All cross-process/store calls above are awaited async calls; parsing/domain construction is synchronous. No detached effect is added. Search query implementations have separate metadata and message responsibilities because their matching sources, ordering, and result shapes differ. They remain BoardStore-owned methods, not a new service layer.

## Eligibility and initialization

Resolve the top-level scope to one project and, where applicable, board/topic IDs using existing require-resource helpers. A typed scope avoids inconsistent combinations of independently supplied parent IDs. Validate before any initialization write.

The unread predicate has two disjoint branches:

```text
top-level:
  kind = mainMessageCreated
  AND activity belongs to selected top-level scope
  AND sequence > selected project's main_start
  AND sequence > this topic's bookmark
  AND actor != reader

thread activity:
  kind = threadMessageCreated | threadResolved | threadUnresolved
  AND this reader has an active watch for activity.root_id
  AND sequence > that watch's starts_after_activity
  AND sequence > this thread's bookmark
  AND actor != reader
  [no selected project/board/topic predicate]
```

Apply the shared captured upper bound and keyset position to both branches. Their kinds are disjoint; use the unique activity sequence as the deduplication key. Filter in SQL before limiting; never fetch one page from each branch and concatenate them because that cannot guarantee a correctly bounded global page. Decode retained records through existing validated domain conversions and preserve the existing byte-budget continuation path.

The inbox storage row projection must also select `board_activity.project_id` and `board_id`; they already exist in the schema. Domain `InboxActivity::MessageCreated` gains `projectId` alongside the existing message containing board/topic IDs. `ThreadStateChanged` gains `projectId` and `boardId` alongside its existing topic/root IDs. The inbox decoder owns construction and validates the projected ancestry against the loaded message or required thread location. It must not populate location from the selected query scope. Latest inbox records likewise pair the actual `projectId` with their `Message`; this is a reader-aware inbox result, not a change to raw history's `Message` type. No location lookup service, copied persisted ancestry, or schema migration is needed. Public schema/CLI output must expose these fields for both kinds of activity, with a cross-project state-change Control proof.

Existing project_reader_state rows, topic bookmarks, and thread watches remain the sources of truth. A thread-watch row can exist while that project's main_start is null. Validate contributing stored boundaries without creating or initializing other projects merely because their watches contribute activity. Do not treat corrupt out-of-scope watch state as safely ignorable when it contributes to this query.

| Initiator | Before | Transition / invariant |
| --- | --- | --- |
| First unread fetch in project P | main_start null/absent | In the existing immediate transaction, capture checkpoint S, set main_start once, recompute P's local summary. Older top-level activity remains history. |
| Repeated unread fetch, another filter in P | main_start S | Preserve S, bookmarks and watches. Filter is query input. |
| Latest or search | Any state | No initialization, watch, acknowledgement, or summary mutation. |
| Explicit/automatic watch | Existing baseline state | Existing watch owner captures/preserves boundary and recomputes that thread's own project summary. |
| Acknowledge returned out-of-scope thread | Thread belongs to Q | Existing acknowledgement derives Q from root, validates position, updates only that bookmark and Q summary. |

The first-use/post race retains serialization through BEGIN IMMEDIATE: activity is before the captured boundary and historical, or after it and potentially unread. Initialization rollback cannot leave a published partial boundary. A lost response may leave a committed boundary; existing inspection guidance applies.

## Latest and cursor consistency

Latest uses the same top-level scope plus currently active watched threads, but selects only message activity and ignores unread/bookmark/watch-start cutoffs. Thus it can show pre-watch history and self-authored messages without making either unread. The root distinction remains: watched-thread messages have root_id set; roots are top-level candidates.

The authenticated cursor payload includes an operation/version discriminator, reader, normalized top-level scope, mode, upper sequence, and last sequence. Use the existing checkpoint cursor key; do not add a key store. Old inbox cursor payloads are incompatible with the changed query and must fail with invalidCursor. Raw-history cursor behavior remains independent.

Latest continuation re-evaluates current active watches and applies the existing fixed upper bound and last-emitted sequence. It does not store, hash, or version the watch set, invalidate a cursor when watches change, or force a restart. A newly watched thread may contribute messages at remaining positions; messages at positions already passed are visible on a fresh latest fetch. This intentionally provides ordinary keyset pagination over current eligibility rather than stable membership across pages. Unread continuation keeps its existing future-only watch boundary rules.

## Search realization

For C6/C7, preserve separate typed operations and store-owned matching. The new request types carry query, strict scope, kind, page, and the explicit archived-inclusion flag. An all-location search can omit a narrowing ancestor through an explicit closed scope variant; conflicting inputs fail validation instead of silently broadening.

Use bound literal text in SQL against current name/description or message text, with explicit ASCII case normalization and no wildcard interpretation. A substring predicate avoids accidental `%`/`_` query language. The existing domain decoder reconstructs messages and references. A thread filter resolves ancestry from its root, never from a second independently trusted parent field.

Discovery combines typed metadata hits with parent joins and stable kind/ID keyset ordering. Names are mutable: continuation exposes current metadata, consistent with existing metadata-list semantics; it does not promise a historical snapshot across renames. Message search joins unique message-created activity for newest-first ordering and captured upper bounds. References are returned metadata, not traversal inputs. For the search archive contract, join current board state on every page and apply the same archived-inclusion flag even for explicit board/topic/thread scopes. The flag is cursor-bound; a board becoming archived removes its remaining hits without invalidating the cursor. Verify archive-between-pages and explicitly targeted archived threads. No retained archive snapshot is needed. Project discovery does not apply a board archive predicate to the project itself.

Direct substring scans have no index-maintenance drift but may hold the serialized store longer on large histories. Existing scope/activity indexes can narrow candidates; an output limit alone does not bound scan cost. Measure representative no-match and broad-match searches before claiming acceptable latency. Full-text indexing is a credible alternative for scale but changes matching and adds synchronization/migration obligations; it is not silently interchangeable with literal substring semantics. No external search process or unmeasured read pool is selected.

## Failures, trust, and cutover

```text
Caller input -> typed validation -> store admission -> transaction/query
  invalid field/root/scope ------> structured validation/resource error
  invalid cursor ---------------> invalidCursor; request fresh query
  invalid stored domain value --> invalidRecord; no coercion or body leak
  unavailable/overloaded -------> existing admission error
  cancellation before commit ---> rollback stateful first use / end read
  lost response after commit ---> inspect first-use state; no automatic replay
```

Existing Control admission/frame budgets bound request and response handling. Parameter binding and domain validation own untrusted query input. The selected service remains the data boundary; self-declared identity remains self-declared and search adds no authentication claim. Logs/errors must not include message bodies or search content unnecessarily. Reads retain archive access under the chosen contract. No retention, deletion, or extra telemetry service is introduced.

The settled filter/cooldown changes need no new authoritative tables. Existing cooldown timestamps are interpreted against 60 seconds after cutover; state need not be reset. Request-only selections require no persistence or migration. The selected direct substring queries require no search index or migration; no baseline migration rewrite is authorized.

All changed domain, SDK, CLI, dispatch, and schema consumers move to the same contract; no compatibility adapter or dual path. Persisted existing state remains authoritative. If a newer read contract cannot be served, return the existing protocol failure rather than pretending the old project-only result satisfies it. A binary rollback does not require deleting watches or bookmarks; old behavior would again be visible and must not be represented as satisfying this Specification. Production process replacement is outside this design.

## Proof seams and tradeoffs

| Contract | Owner / proof seam | Required observation |
| --- | --- | --- |
| C1–C2 | Real BoardStore eligibility and public Control roundtrip | Mixed scoped top-level and global watched-thread records; no unwatched traffic or reference expansion; unchanged watch rows. |
| C3–C4 | Real SQLite transaction boundary and reopen | Current first-use sequence, historical latest, preserved pre-existing watches, both first-use/post serial orders, no latest/search state creation. |
| C5 | Real store pagination plus frame-limited Control response | Interleaved branches, byte limits, fresh messages, ack/continue, out-of-project acknowledgement, cursor mismatch, and valid latest continuation across watch changes with fresh-fetch visibility for already-passed positions. |
| C6–C7 | Real metadata/message queries through Control | Scope/kind restrictions, matching behavior, actual thread messages, parent identity, unchanged personal state; no mocks for matching or storage. |
| C9 | Required domain request field, CLI validation, real Control roundtrip | Missing scope rejected; concurrent/alternating scopes and continuation validation; no saved selection after reopen, with existing reader state preserved. |
| C8 | Existing post transaction and domain failure; CLI output | 59,999 ms rejected, 60,000 ms allowed, rounded wait, independent actor/board keys, no rejected effects, thread exemption. |

The repository already has real-store tests in `crates/message-board-storage/tests/inbox_history_and_summary.rs` and socket-pair Control proof in `crates/collaboration-service/tests/board_control_path.rs`. Those are existing seams, not runtime proof already performed. CLI proof must traverse actual commands and the debug service, not a client mock. Baseline migration/schema, domain, Control, and relevant end-to-end proof gates remain mandatory.

Keep query composition at the store rather than in agents: storage bears the SQL complexity so callers receive correctly filtered, bounded pages. Keep summary meaning project-local rather than making one mutable boolean represent every filter. Keep thread watches independent rather than expanding topic selections into watch rows. These choices preserve the spam-control boundary. Revisit query/index strategy if representative search or cross-project watched-thread workloads exceed existing service budgets; do not trade away completeness or add asynchronous summaries silently.
