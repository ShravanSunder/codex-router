# Sessions runtime picker

## Agreed outcome

The existing agent-sessions picker displays runtime status in a new column immediately before the unchanged Upd and New columns. Preserve title, branch/directory metadata, selection, conversation preview, search, native open/new/fork, and current layout shape. Runtime views are Blocked, Active, Idle and All; All is the default. Default interactive scope is Repo, while scope choices remain cwd/repo/all. No worktree scope is invented. Hide the old Threads/source cycling control; interactive discovery includes all sources. Explicit noninteractive source filters remain supported.

Blocked uses ◆ and means native active with waiting-on-approval or waiting-on-user-input flags. Active means active without these flags. Idle requires a native idle observation. Unknown, not loaded and system-error rows appear only in All. Lack of runtime observation never means idle or not loaded. Detailed help is toggled with Ctrl+/ (including equivalent terminal encoding) or F1, with a compact discoverable hint. Ctrl+T cycles views, Ctrl+S scope, Ctrl+O sort, Ctrl+R refresh. Esc closes help before clearing search/exiting.

## Ownership and data flow

Stored catalog -> existing row metadata/history lookup
Public Rust Control client -> loaded inventory + runtime metadata
Both -> picker row projection -> view/scope/search -> existing iocraft layout

The picker loader consumes existing public read-only operations, not Host internals or native control mutations. Runtime-only threads are included even without a catalog row. Scope/provider/search filtering applies after their metadata is available. Background bounded refresh leaves search, scope and selected identity intact when the selected row remains visible. Endpoint loss or failed/inconsistent refresh invalidates live status. Opening an explicitly selected row uses the existing native launch path; mere browsing does not load/resume/interrupt or send messages.

Runtime observations cover only the selected local service's codex-local endpoint, not all independent app-servers on the machine. Debug builds use the existing debug service root and never mutate production. Local mode has no claim of hosted runtime observation. No V2 scheduling/mailbox/board work is included.

## Proof

Permanent tests cover default scope, status mapping/filtering (including blocked flags), no false-idle on unavailable service, runtime-only inclusion, existing metadata preservation, paged/generation-consistent reads and absence of mutating requests. Component tests cover column order, ◆ icon, help toggle, keyboard filtering, selection/search preservation and narrow/wide layout. Actual debug-only runnable TUI proof observes the service-backed picker and key changes in a PTY; model-backed fixture setup uses Luna only if needed. Run affected Rust tests, format and Clippy, independent implementation review and current PR CI before readiness. No production process replacement or merge.
