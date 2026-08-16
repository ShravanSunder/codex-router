# Session Picker Discovery Specification

Governing Requirements:
[2026-08-13-session-picker-discovery-requirements.md](./2026-08-13-session-picker-discovery-requirements.md)

## Observable outcome

`codex-router sessions` presents one coherent discovery model: scope decides
which active Codex sessions are eligible, and the search expression narrows
that eligible set. The interactive picker defaults to `repo`; its scope cycle
is `cwd -> repo -> all -> cwd`. An explicit CLI root option still determines
the initial scope, and noninteractive `--checkout` remains available.

The controls show the active value (`cwd`, `repo`, or `all`) directly. They do
not prefix it with `Scope:`. Search help shows the supported qualified forms.

## Repository scope contract

R1 (U1, U4): In `repo` scope, membership MUST follow this decision table.
“Known origin” means normalization succeeded; a malformed non-empty origin is
present but unknown, not absent.

| Current origin | Row origin | Inclusion rule |
| --- | --- | --- |
| known | known and equal | include |
| known | known and different, or present but malformed | exclude; cwd shape cannot override the conflict |
| known | absent | include only when cwd is under a live worktree root or passes the bounded historical-basename fallback |
| unknown | any present value | include only when cwd is under a live worktree root; no deleted-path basename inference |
| unknown | absent | include when cwd is under a live worktree root or passes the bounded historical-basename fallback |

The current repository basename MUST come from the normalized origin's
repository-path basename when origin is known. Otherwise it MUST come from the
Git common-directory owner when available, then the primary worktree root, and
only finally the current checkout leaf. A suffixed linked-worktree leaf MUST
not replace a stable basename available from an earlier source.

The basename rule is a legacy fallback, not an equal source of repository
truth. Path comparisons MUST retain the existing `/var` and `/private/var`
alias handling. If Git metadata for the current directory cannot be read, repo
scope MUST degrade to the current checkout/path evidence that is available and
MUST not silently become `all`.

Origin normalization MUST compare repository identity across common URL forms:
trim surrounding whitespace and trailing slashes, parse SCP-style SSH and URL
forms, ignore user-info and transport, lowercase the host, and remove one
trailing `.git` from the repository path. Path case remains significant except
where the hosting platform's established identity rules make it insensitive.
Credentials and URL query/fragment data MUST not participate in identity or be
rendered.

Examples for a current basename `codex-router`:

| Session metadata | Repo result | Reason |
| --- | --- | --- |
| matching normalized origin, deleted cwd | include | origin is authoritative |
| absent origin, cwd leaf `codex-router.impl-x` | include | bounded legacy fallback |
| absent origin, cwd leaf `codex-router-live-fix` | include | bounded legacy fallback |
| absent origin, cwd leaf `my-codex-router` | exclude | basename is not at the leaf start |
| nonmatching present origin, cwd leaf `codex-router-copy` | exclude | conflicting origin wins |

R2 (U5): Interactive scope MUST expose only `cwd`, `repo`, and `all`.
`cwd` is exact current cwd. `repo` uses R1. `all` includes every otherwise
eligible active session. The picker MUST default to `repo` when no root option
was supplied. The explicit noninteractive checkout filter remains an accepted
CLI behavior but is not a picker scope state.

`--checkout` MUST remain valid with `--list` and MUST be rejected in interactive
picker mode with a direct error. This preserves its exact containment meaning
where it remains representable without silently mapping it to narrower `cwd`
or broader `repo` picker semantics.

## Search language and matching

R3 (U2, U3, U4): Search MUST accept whitespace-separated terms, quoted values
for spaces, and these case-insensitive qualifiers:

| Form | Fields searched |
| --- | --- |
| bare term | session ID, title, preview, first user message, normalized origin, cwd |
| `id:value` | session ID |
| `b:value` or `branch:value` | branch |
| `repo:value` | normalized origin and cwd |

Bare terms MUST NOT match branch or provider/model metadata. Qualified prefixes
with an empty value MUST match no sessions and MUST remain visibly editable.
An unrecognized `name:value` token is treated as a bare token so punctuation in
ordinary searches does not unexpectedly empty the list.

Every term is ANDed with every other term. Within one bare or `repo:` term, a
match in any listed field is sufficient. Matching is a Unicode lowercase
substring comparison after trimming the query syntax; it does not perform
locale-specific collation or compatibility normalization. SQL wildcard
characters in user input are literal characters. A quoted phrase is one term.

Examples:

| Query | Required behavior |
| --- | --- |
| `019abc` | match a session ID containing `019abc` |
| `router preview` | both bare terms must match, possibly in different bare fields |
| `b:main crash` | branch contains `main` AND a non-branch bare field contains `crash` |
| `main` | never match solely because the branch is `main` |
| `repo:codex-router b:fix` | repository context and branch both match |

R4 (U2): Search MUST use the complete persisted values returned from Codex
state, not the 96-character display title or other presentation truncation.
Codex `preview` is a supported searchable field: upstream Codex itself applies
`searchTerm` to `name`, `title`, and `preview`. Conversation JSONL bodies MUST
remain outside search.

R5 (U2, U7): Reload and immediate visible-row matching MUST use the same pure
matcher over complete records. SQLite MAY narrow candidates only with a
predicate proven not to exclude any record that matcher would accept; it MUST
not supply a second case-folding or repository-identity authority. Candidate
rows MUST be paged until the requested number of final matches is collected or
the eligible store is exhausted. A reload result produced for an obsolete
query MUST never replace records for the current query. Empty search MUST not
alter the selected scope.

## Conversation-focused detail contract

R6 (U6): The selected-session detail pane MUST contain a Conversation section
and MUST not contain separate Preview or Metadata sections. Conversation
snippets MUST wrap within the pane and use the vertical space reclaimed from
those sections. Text that exceeds the available pane height MAY be clipped by
the pane boundary, but each displayed snippet MUST not be pre-truncated merely
to one terminal row.

The list remains compact: its title and context lines MAY be width-truncated.
That display truncation MUST not change search matching. A selected session
whose rollout is missing, unreadable, malformed, or has no recent messages MUST
show the existing bounded unavailable reason and MUST leave navigation usable.

## Responsiveness, concurrency, and failure

R7 (U7, U8): Git subprocess discovery, SQLite access, and rollout filesystem
reads/parsing MUST execute on a blocking-capable boundary, never directly on a
Tokio async worker. Conversation loading remains lazy for the selected session
and reads at most the existing 256 KiB tail. Search MUST not read rollout files.

At most one blocking record reload may execute at a time. While it runs, query
changes MUST coalesce to one replaceable latest query; after completion, that
latest query runs if it differs from the completed query. At most one result
may become authoritative: the result whose query identity equals the current
picker query. Cancellation before blocking work starts MAY reduce work but is
not the correctness or capacity mechanism. Conversation loads MUST be keyed by session ID,
deduplicated while in flight, and cached only for that ID. A late completion
for a previously selected session MUST NOT replace or label the current
session's conversation.

Load failure, join failure, malformed metadata, or Git discovery failure MUST
degrade to an empty/unavailable bounded result appropriate to that surface;
the picker MUST remain interactive and MUST NOT mutate Codex state.

## Compatibility and negative space

The change reads the current `state_5.sqlite` schema and rollout JSONL files
read-only. No backfill is required. Rows missing both reliable origin and a
bounded cwd-family match remain outside repo scope and are still discoverable
through `all` or explicit ID search within `all`.

The exact ranking of multiple matches remains recency/creation sort behavior;
this specification does not introduce relevance scoring.

## Requirement coverage and proof obligations

| Need | Problem | Outcome | Requirement/contract | Evidence class |
| --- | --- | --- | --- | --- |
| U1 | deleted worktrees vanish | historical sessions remain discoverable | R1 | state-backed integration cases plus CLI transcript |
| U2 | inconsistent limited search | ID and summary text are findable | R3-R5 | parser/matcher behavior plus SQLite-backed candidate paging |
| U3 | branch is not searchable safely | branch is explicit only | R3 | positive qualified and negative bare-term behavior |
| U4 | repo identity is incomplete | origin and bounded historical context work | R1, R3 | normalization/fallback boundary cases |
| U5 | scope is redundant/noisy | only useful picker scopes remain | R2 | picker interaction and visual evidence |
| U6 | detail space is wasted | wrapped conversation dominates detail | R6 | supported-width visual/manual evidence |
| U7 | async work can race or block | responsive, stale-safe loading | R5, R7 | deterministic overlap tests plus live interaction |
| U8 | avoid new state machinery | current read-only foundation is reused | R7, compatibility | code inspection and read-only state proof |
