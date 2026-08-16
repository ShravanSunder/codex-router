# Session Picker Discovery Program Design

Governing documents:

- [Requirements](./2026-08-13-session-picker-discovery-requirements.md)
- [Specification](./2026-08-13-session-picker-discovery-specification.md)

## The smallest structural change

Keep the existing read-only SQLite query, picker model, iocraft component, and
lazy JSONL conversation reader. Add two pure domain values—repository identity
and parsed search expression—so SQL loading and in-memory filtering consume the
same meaning. Keep all blocking work behind the picker's existing
`spawn_blocking` boundary and make request identity explicit at completion.

No index, background service, migrated state, or second session store is
needed. The cost is that legacy rows without origin use a conservative basename
fallback and may require `all` for ambiguous history; the user bears that small
false-negative risk instead of every repo bearing false-positive scope leaks.

## Current and target ownership

```text
Session command / picker composition
  owns: initial filters and read-only Codex-home boundary
  consumes: RepositoryIdentity discovery, SessionQuery

RepositoryIdentity                         [new pure domain value]
  owns: normalized current origin, live roots, basename fallback rules
  consumed by: SQL scope compiler, picker matcher

SessionSearchExpression                    [new pure domain value]
  owns: tokenization, qualifiers, AND semantics, field matching meaning
  consumed by: SQL predicate compiler, picker matcher, search help

Session record loader                      [existing integration boundary]
  owns: read-only SQLite page loading and complete searchable record fields
  runs: blocking job containing Git discovery + a current-thread SQL runtime

SessionsPickerModel                        [existing state owner]
  owns: current query, focused row, derived visible indices
  depends on: pure scope/search matchers

SessionsPickerComponent                    [existing async/effect owner]
  owns: reload and conversation task dispatch, loading cache, stale guards
  renders: list, conversation-focused detail, bottom controls

SessionConversationPreview                 [existing blocking reader]
  owns: validated bounded JSONL tail parsing and sanitized snippets
```

Dependency direction is command/component -> domain values -> persisted record
fields. Pure matching must not invoke Git, SQLite, filesystem I/O, or rendering.
Rendering must not reconstruct search or repository rules. The SQL compiler and
in-memory matcher may depend on the same parsed value but neither owns its
semantics.

## Repository identity

`RepositoryIdentity` is an immutable request input:

```text
normalized_origin: optional host/path identity
live_roots: normalized current git-worktree paths
fallback_cwd: exact invoking cwd used only when Git identity is undiscoverable
repository_basename: stable repository leaf used only for missing-origin rows
discovery_quality: origin-and-worktrees | worktrees-only | exact-cwd-fallback
```

Discovery runs once for the initial picker request and again only if a fresh
command/query construction requires it; it is not global cached state. Git
commands collect the current top-level, common directory, primary and linked
worktree roots, and `remote.origin.url` as one fallible blocking operation.
The stable basename comes from normalized origin path, then the owner directory
of the Git common directory, then the primary worktree root, then the current
checkout as a final degradation. Partial Git success is retained. Secrets,
userinfo, query strings, and fragments are discarded before the value crosses
into presentation state.

The row matcher applies the Specification R1 decision table. Known equal or
different origins decide first. With known current origin and absent row
origin, live roots and the basename fallback are allowed. With unknown current
origin, a present row origin may be admitted by a live root but never by a
deleted-path basename; an absent row origin may use either path rule.

SQLite is a candidate source, not repository-identity authority. It applies
archived/source/provider filters and any root predicate proven to be a superset
of the pure matcher, pages in requested sort order, and lets the matcher decide
final membership. If no safe normalized-origin SQL superset exists, repo scope
omits that narrowing rather than losing valid rows. Paging stops after the
requested final match count or store exhaustion. The picker uses the same pure
matcher for already-loaded records.

Command validation keeps Checkout as a list-query-only root. Interactive
initialization maps no explicit root to Cwd, `--repo` to Repo, and `--any` to
Any; it rejects `--checkout` before picker construction. The picker model thus
never needs a hidden fourth root or a lossy Checkout mapping.

## Search expression

`SessionSearchExpression::parse(input)` produces ordered terms:

```text
SearchTerm
  Bare(value)
  SessionId(value)
  Branch(value)
  Repository(value)
```

The parser is pure and total: unmatched quotes consume the remaining text as a
single editable value; recognized empty qualifiers produce an unsatisfied term;
unknown prefixes remain bare. Values are normalized once for case-insensitive
matching while the original input remains available for rendering.

Each loaded `SessionPickerRecord` carries complete searchable fields separately
from compact display fields: full session ID, title, preview, first user
message, branch, cwd, and normalized-or-raw origin. Display title/context may
still be truncated during rendering. Provider/model remain record metadata for
launch behavior or compact list context but are not bare-search fields.

One matcher evaluates terms using Rust Unicode lowercasing; terms are joined
with `AND` and fields inside a term with `OR`. SQLite does not perform final
search matching because its stock `lower`/`LIKE` behavior is ASCII-only. It may
apply an ASCII-only candidate predicate only when it is a proven superset, then
streams pages to the pure matcher. Tests exercise candidate paging plus final
matching so display and reload cannot diverge.

## Current-to-target call paths

```text
INITIAL LOAD
current:
  command -> repo_roots() [sync Git] -> build picker request
          -> component spawn_blocking(loader)
          -> loader creates current-thread Tokio runtime -> async sqlx query
          <- records / error <- join result <- render

target changes:
  command -> discover RepositoryIdentity [changed blocking Git operation]
          -> build SessionQuery(identity, parsed empty search) [added]
          -> component spawn_blocking(loader)                 [unchanged, critical]
          -> loader -> SQL candidate query + pure matchers     [changed]
          -> current-thread runtime -> read-only sqlx          [unchanged, critical]
          <- records tagged with query identity               [changed]
          <- apply only when identity == current query         [unchanged guard, explicit]

SEARCH RELOAD
current:
  key event -> model mutates raw search -> spawn every changed query
            -> SQL search over one raw string
            <- equality guard -> replace records
            -> in-memory matcher uses a smaller field set

target:
  key event -> parse expression -> update derived visible rows [changed]
            -> if reload active, replace one pending query       [added]
            -> otherwise spawn one blocking paged load           [changed]
            <- generation + value guard                         [changed]
            <- replace/discard; then run only latest pending query

CONVERSATION DETAIL
current and preserved:
  focus -> cache miss -> mark session ID Loading
        -> spawn_blocking(validated 1 MiB JSONL-tail read)
        <- cache Loaded under the requested session ID
        -> current focus reads only its own cache key

target presentation change:
  current session-keyed result -> wrapped Conversation-only pane [changed]
```

All return/error edges terminate at the component. Record-load failure leaves
the last authoritative record set visible; conversation failure stores a
bounded unavailable result for only that session. No error path writes Codex
state.

## Async state and concurrency

| State | Owner | Transition and guard | Late/illegal completion |
| --- | --- | --- | --- |
| current query generation | component/model | increment when scope, source, sort, or parsed search changes | older generation cannot replace records |
| reload coordinator | component | Idle -> Running(query); changes while Running replace `pending_query`; completion starts only the latest pending value or returns Idle | at most one blocking reload executes; generation/value guard discards stale data |
| conversation cache entry | component, keyed by session ID | Missing -> Loading -> Loaded | completion writes only its requested ID |
| focused conversation | derived at render | current focused ID looks up its cache entry | no cross-ID assignment is possible |

The component owns a small in-memory reload coordinator, not a service or
cache. An input event starts the async reload handler only when it transitions
Idle to Running. While Running, later events only replace `pending_query`.
After the blocking job joins, the handler applies its identity guard and loops
with the latest pending query, repeating only if input changes again.

`spawn_blocking` cancellation is cooperative only before a blocking job starts;
aborting its async handle does not stop a running OS thread. Correctness rests
on identity guards and capacity rests on component-local single-flight
coalescing, not cancellation or Tokio's blocking-pool limit. Conversation loads
remain one in-flight load per session. Rapid refocus may create work for
several different IDs, but duplicates for one ID are forbidden and every read
remains bounded to 1 MiB.

SQLite stays async inside the loader's current-thread runtime because `sqlx`
owns that interface. Git and filesystem operations stay synchronous inside the
outer blocking job. No nested `block_on` or blocking filesystem/Git call occurs
on a Tokio worker.

## Failure and degradation

| Boundary | Detection | Containment and recovery | Observable result |
| --- | --- | --- | --- |
| Git repository identity unavailable | all Git evidence fails | use the normalized exact invoking cwd only | repo scope equals cwd; never descendants or `all` |
| malformed row origin | normalization failure | apply the R1 present-unknown cell; never treat it as absent | no basename false-positive |
| SQLite open/query/join failure | loader result | retain last authoritative rows; allow subsequent query | picker remains usable |
| obsolete reload | query generation/value mismatch | discard entire result | current rows remain authoritative |
| missing/unreadable/malformed JSONL | validated reader result | cache bounded unavailable state for requested ID | detail shows reason; navigation works |
| terminal too small | existing component guard | preserve current behavior | bounded narrow-terminal outcome |

There is no retry loop: user input naturally issues a later query, and automatic
retry would add load without a new source of truth. There is no migration or
dual-read phase because the design changes only read interpretation.

## Conversation-focused iocraft composition

```text
Picker View (column)
  search/help Text
  filter controls: [cwd|repo|all] ...
  list View (flex-growing)
  Conversation detail View (content/flex allocation)
    heading Text
    wrapped snippet Text children
  spacer View (flex-growing when room remains)
  shortcuts Text (bottom sibling)
```

Preview and Metadata children are deleted. Conversation snippets are separate
wrapped `Text` children inside an overflow-bounded `View`; they are not manually
padded terminal strings. The list remains compact and width-fitted. This keeps
navigation, content, flexible space, and shortcuts as distinct iocraft
siblings, matching the repository layout constraint.

## Requirement realization and proof seams

| Requirement | Structural owner and realization | Proof seam |
| --- | --- | --- |
| R1 | RepositoryIdentity decision table plus pure final scope matching | origin-availability, invoking-worktree, and fallback fixture matrix over paged SQLite candidates |
| R2 | command mode validation, initial-root mapping, and three-state picker scope machine | CLI matrix plus key-driven component snapshot/manual picker interaction |
| R3-R5 | SessionSearchExpression, complete record fields, Unicode pure matcher, candidate paging, query guard | parser/matcher unit matrix; SQLite candidate integration; rapid-query overlap harness |
| R6 | Conversation-only iocraft subtree with wrapped Text children | mock-terminal snapshots at narrow/normal heights plus live terminal inspection |
| R7 | component task ownership, spawn_blocking boundaries, generation/session guards | deterministic delayed loader/read fixtures and runtime interaction |

The SQLite and JSONL integration seams use real temporary stores/files with the
same schema/event shapes; pure parser and normalization tests need no process or
filesystem. Manual proof uses normal `$HOME/.codex` read-only state and must not
replace the production router process.

## Tradeoffs and revisit signals

The rejected path-only alternative is smaller in code but cannot satisfy R1
after deletion. A persisted router-owned repository registry could reduce
legacy ambiguity, but it duplicates Codex state, creates migration and cleanup
ownership, and is outside the accepted boundary. Revisit new persistence only
if measured false negatives remain material after origin-first matching and the
bounded fallback, or if Codex removes `git_origin_url` from its supported state.

The normalized-origin identity assumes a repository's origin is the durable
lineage desired by the user. Sessions created before an origin change may fall
outside repo scope if their old origin is present. That is intentional debt:
`all` remains the recovery surface, and automatically merging old and new
origins would require owner-controlled repository alias policy not authorized
here.

Stock-SQLite final matching was rejected because its ASCII case folding cannot
equal Rust Unicode matching. Pure final matching may scan more candidate pages;
the component-local single-flight coordinator pays and bounds that cost. If
measured candidate scans make interaction miss an accepted responsiveness
threshold, revisit a read-only SQLite custom function or upstream search API,
not a router-owned persisted index, unless the owner expands the boundary.
