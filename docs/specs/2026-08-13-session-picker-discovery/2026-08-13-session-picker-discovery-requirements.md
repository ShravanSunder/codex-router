# Session Picker Discovery Requirements

## Authority and boundary

This document records the product needs confirmed by the repository owner on
2026-08-13 for `codex-router sessions`. It governs session discovery, picker
search, scope choices, and the detail pane. It does not authorize changes to
Codex's storage, session files, resume protocol, production router process, or
release lifecycle.

The affected user is a developer who uses Codex across a repository's main
checkout, temporary worktrees, renamed worktrees, and worktrees that may later
be deleted. The relevant system surface is the router-owned session picker and
its read-only interpretation of normal Codex state.

## Current problem

The picker treats the current output of `git worktree list` as the complete
identity of a repository. Once a worktree is deleted, its path disappears from
that output, even though its Codex session remains in `state_5.sqlite`. Those
sessions consequently disappear from repo scope.

Search is also inconsistent. The SQL query can match ID, title, preview, first
user message, and provider, while the in-memory picker only matches the
already-truncated display title, ID, and provider. Branch cannot be searched
deliberately. The detail pane spends scarce rows on a duplicated preview and
low-value metadata while conversation text is clipped to one line.

Observed evidence on 2026-08-13:

- the current repository has one registered worktree;
- 962 active Codex thread rows share its persisted Git origin across seven cwd
  values, including deleted worktree paths;
- only 71 of those rows use the current cwd;
- 25 additional active rows with a codex-router-shaped cwd have no persisted
  origin;
- upstream Codex at `e766f7598993ce37cf61b9c26c80cc2ba3a4f2d7`
  persists `git_origin_url`, `git_branch`, `title`, `preview`, and
  `first_user_message`, and its `thread/list.searchTerm` implementation matches
  name, title, or preview.

## Authorized needs

| ID | Priority | Need and intended outcome | Authority |
| --- | --- | --- | --- |
| U1 | Must | Repository scope keeps a repository's sessions discoverable after a worktree is renamed or deleted. | Repository owner, 2026-08-13 |
| U2 | Must | Search finds a known session by session ID and by useful persisted conversation summary text. | Repository owner, 2026-08-13 |
| U3 | Must | Branch search is explicit through `b:` or `branch:`; bare terms never match branch. | Repository owner, 2026-08-13 |
| U4 | Must | Repository search can target repository identity and historical worktree context without treating branch as repository identity. | Repository owner, 2026-08-13 |
| U5 | Must | Scope choices express the useful distinctions `cwd`, `repo`, and `all`, without a redundant `Scope` label or a separate worktree choice in the picker. | Repository owner, 2026-08-13 |
| U6 | Must | The detail pane prioritizes readable conversation content over duplicated preview and low-value metadata. | Repository owner, 2026-08-13 |
| U7 | Must | Search and preview loading remain responsive and correct under rapid input and selection changes; blocking Git, SQLite, and JSONL work must not run on Tokio async worker threads. | Repository owner, 2026-08-13 |
| U8 | Should | The design reuses Codex's current read-only state and the existing lazy picker loader rather than adding an index, cache, migration, or new persisted identity. | Repository owner and repository constraints |

Priorities are assigned by the repository owner. “Must” is required for this
change; “Should” may be revisited only if current-state performance evidence
shows the existing read-only foundation cannot satisfy U7.

## Success boundary

Success means a developer can enter the picker from any live checkout, start
in repository scope, find sessions from live or deleted sibling worktrees,
search with ordinary text or explicit ID/branch/repository qualifiers, and read
more of the selected conversation without stale results replacing current
results.

The accepted scope choices are:

```text
cwd  -> exact current working directory
repo -> same repository, including current and historical worktrees
all  -> all active Codex sessions
```

## Non-goals

- Do not modify, backfill, or migrate `state_5.sqlite` or rollout JSONL files.
- Do not make branch part of repository identity or bare-term matching.
- Do not full-text search rollout JSONL conversation bodies; persisted title,
  preview, and first-user-message fields are the searchable conversation
  summaries.
- Do not add a repository registry, background indexer, cache database, daemon,
  or filesystem scan of deleted paths.
- Do not promise perfect recovery for legacy rows that have neither a matching
  origin nor an unambiguous repository-shaped cwd.
- Do not change how Codex resumes a selected session.
- Do not remove the explicit noninteractive `--checkout` CLI filter merely
  because the interactive picker no longer cycles through a worktree scope.

## Accepted-requirements set

U1 through U8 are the complete accepted set for the bounded design. No
additional persistence, compatibility layer, production-process action, or
release action is authorized.
