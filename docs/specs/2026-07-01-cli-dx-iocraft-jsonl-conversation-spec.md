# CLI DX iocraft JSONL Conversation Spec

Status: implementation contract
Date: 2026-07-01
Supersedes: the session detail/prototype-location parts of `docs/specs/2026-06-29-cli-dx-iocraft-layout-spec.md`

## Problem

The iocraft prototype has converged on the right interaction shape for `sessions`
and `quota`, but the product contract is not complete until the CLI uses real
data and keeps the prototype source of truth in the right place.

The most important missing piece is the sessions detail panel: "conversation"
means recent real user and assistant messages extracted from Codex JSONL history,
not SQLite preview text, synthetic labels, or hard-coded prototype copy.

## Current Evidence

- `crates/codex-router-cli/src/sessions.rs` currently loads session rows from
  Codex `state_5.sqlite`, but the selected columns do not include
  `threads.rollout_path`.
- `threads.rollout_path` exists in the current Codex state schema and points at
  the local JSONL rollout/session history file.
- `crates/codex-router-cli/src/presentation/session_picker/*` currently owns UI
  state and rendering. It must not discover Codex history files ad hoc.
- `crates/codex-router-cli/examples/ux_prototype.rs` is useful visual prior art,
  but it is in the wrong location and contains mock conversation text.
- `crates/codex-router-cli/src/quota.rs` owns quota report construction and
  current human rendering. Quota math and routing decisions are not part of this
  redesign.

## Product Requirements

### Prototype Contract

1. Authoritative UX prototypes for this work live under `prototypes/*`.
2. No authoritative sessions/quota UX prototype remains under
   `crates/codex-router-cli/examples/*`.
3. Prototypes must render with real iocraft surfaces. SVG/string-only mockups are
   not acceptance evidence.
4. Prototype captures may be converted to PNG for review, but ANSI/TTY captures
   from the real runnable prototype or CLI path are the source of truth.

### Sessions Picker

1. The sessions picker keeps the converged two-pane shape at wide widths:
   list on the left, detail panel on the right.
2. At narrow widths the same detail sections move below the list instead of
   becoming cramped or clipped.
3. Each session is a two-row block:
   - row 1: marker, title, updated, created
   - row 2: branch and cwd/context
4. There is visible breathing room between session blocks.
5. The selected highlight covers the full two-row session block.
6. The selection marker is `❯`.
7. Branch is shown with `⎇`.
8. Cwd/current-directory context is shown with `📂`.
9. Typed input searches sessions.
10. `Cmd+N` starts a new thread from the picker. The footer must show this as a
    primary yellow affordance.
11. Filters are explicit and not overloaded:
    - path scope: cwd, worktree/checkout, repo, all
    - thread set: interactive, subagents, all
    - sort: updated, created
12. The detail panel has exactly three sections:
    - Preview
    - Conversation
    - Metadata
13. Preview shows the selected session title/summary. It must not show literal
    `user`/`assistant` role labels.
14. Conversation shows the last couple of actual user/assistant messages from
    JSONL history for the selected session.
15. Metadata shows useful extra data that is not already visible in the list.
    Do not repeat branch, cwd, updated, or created in metadata.

### JSONL Conversation Extraction

1. `threads.rollout_path` is the authoritative history source when present.
2. The loader must not derive a JSONL path from session id/date while
   `rollout_path` is available.
3. History extraction is read-only and local.
4. The parser reads JSONL line by line and tolerates malformed or truncated
   lines.
5. The parser only considers top-level events with:
   - `type == "response_item"`
   - `payload.type == "message"`
   - `payload.role in {"user", "assistant"}`
6. Text extraction supports string content and list/dict content containing text
   fields such as `text`, `content`, or `output_text`.
7. The parser skips framework/control content:
   - AGENTS/bootstrap instructions
   - hook prompts
   - `<turn_aborted>` markers
   - tool calls and tool outputs
   - reasoning events
   - system/context events
   - encrypted summaries
8. Conversation snippets are bounded:
   - keep only the recent messages needed by the panel
   - truncate long single messages for the available width
   - never dump raw tool output or full history into the UI
9. Missing/unreadable/no-message history falls back gracefully to a clear
   unavailable state. It must not fabricate conversation text.
10. Conversation data is human-interactive UI only. Existing machine JSON output
    must not gain prompt-derived content unless a separate machine-output spec
    explicitly changes that contract.

### Quota View

1. Quota uses the same block rhythm as the accepted prototype, not a dense
   plaintext table.
2. Each account is a four-row block with visible spacing between accounts.
3. The selected highlight covers the full account block.
4. Indentation is consistent across the account title and detail rows.
5. No alternating row tint is used.
6. The main account row shows account, status, and compact 5h/weekly quota.
7. The selected detail area shows:
   - 5h quota
   - weekly quota
   - activity
   - burn rate
   - guards
   - reset
   - note
8. Burn rate uses a different bar glyph/style from quota remaining.
9. The `5h` label in details is normal text, not accent-colored.
10. Wide widths use a side detail panel; narrow widths move details below.

## Ownership Boundaries

### Sessions Data Boundary

`crates/codex-router-cli/src/sessions.rs` owns:

- reading Codex SQLite metadata
- selecting `threads.rollout_path`
- resolving whether a selected session has an available history source
- preparing the data handle needed by the picker

It does not own terminal layout.

### Conversation History Boundary

A dedicated history extraction module owns:

- bounded JSONL reading
- JSONL event parsing
- role/message filtering
- control-content exclusion
- snippet truncation/fallback data

It does not own SQLite discovery or iocraft rendering.

### Sessions Presentation Boundary

`crates/codex-router-cli/src/presentation/session_picker/*` owns:

- search/filter/selection state
- responsive layout
- rendering the prepared preview/conversation/metadata data
- key handling for resume, search, filter changes, and `Cmd+N`

It must not scan Codex home or guess JSONL paths.

### Quota Boundary

Quota report construction remains owned by `crates/codex-router-cli/src/quota.rs`.
The iocraft/block renderer may be extracted under `src/presentation/`, but the
redesign must not change quota selection math, burn-rate calculation, OAuth,
router runtime behavior, or persistence semantics.

### Prototype Boundary

`prototypes/*` owns runnable visual prototypes and capture scripts for the UX
contract. Production code must not depend on prototype modules.

## Proof Gates

### Unit

- JSONL extraction covers valid `response_item.payload` message events.
- JSONL extraction skips malformed lines, tool/system/control events, AGENTS
  payloads, hook prompts, and `<turn_aborted>`.
- JSONL extraction returns a graceful unavailable state when `rollout_path` is
  absent, unreadable, or has no allowed messages.
- Existing `sessions --list --format json` no-leak behavior remains intact.
- Session model tests cover two-row selection blocks, filtering, and search.
- Quota render/model tests cover four-row account blocks, gaps, no alternating
  tint, and selected-block highlighting.

### Smoke / Visual

- Real iocraft prototype runs from `prototypes/*`.
- Real user-facing sessions CLI path renders from current Codex state.
- Real user-facing quota CLI path renders from current quota state or a scoped
  deterministic fixture.
- Captures are produced at multiple widths, including at least narrow, medium,
  and wide. Current acceptance widths are 80, 96, 110, 120, 140, and 150 columns
  unless the implementation documents a stricter supported terminal minimum.
- At least one sessions capture shows real JSONL conversation snippets.
- At least one sessions capture shows graceful unavailable conversation state.
- At least one quota capture shows selected account details with weekly and burn
  bars visible.

## Non-Goals

- No change to quota routing or account selection math.
- No change to auth/OAuth/websocket behavior.
- No change to Codex upstream JSONL schema.
- No full transcript viewer.
- No machine JSON expansion with prompt-derived conversation content.
- No SVG-only mockup as proof.
- No production dependency on prototype code.

## Stop Conditions

Stop and reconverge before more implementation if:

1. `threads.rollout_path` is not available or trustworthy enough to locate local
   JSONL history.
2. Conversation extraction would require reading arbitrary untrusted paths.
3. History I/O cannot be kept out of the pure picker render/model boundary.
4. The UI cannot render at the required width set without clipping or overlap.
5. The prototype no longer matches the contracted UX shape.
