# Sessions, recovery and log shipping (research track, R10)

Research for [the main file](../2026-10-04-distributed-agent-collaboration.md), 2026-10-04. This file records what is known and what lane P5 must answer. It does not choose a design.

**Status labels:**
- **verified:** read in current source, or in vendor docs or code retrieved on the date given;
- **inference:** reasoning that hasn't been tested.

## 1. What a session is made of

A session's native logs are only one part of it. Moving or recovering a session involves all of these:

```text
 native conversation logs  ── owned by the provider harness (Codex, Claude)
 workspace / checkout      ── git plus dirty and untracked files; moved separately
 credentials, tools, MCP   ── provisioned separately; never shipped in log backups
 running turn / processes  ── live; no provider moves these
 Router facts              ── bindings, attempts, receipts, unknown outcomes
```

## 2. Recovery levels: choose the promise first

| Level | Promise | Known support |
|---|---|---|
| 1 | Readable archive of the history | Codex: `thread/read` returns history without loading a live thread (https://developers.openai.com/codex/app-server, retrieved 2026-10-03, verified). Claude: `/export` renders the transcript as text (https://code.claude.com/docs/en/sessions, retrieved 2026-10-03, verified). |
| 2 | A new turn rebuilt from the history | possible for both (inference); it isn't the same native conversation |
| 3 | The same session resumed on another machine | **Codex:** resume by path or supplied history is marked unstable in the app-server protocol (https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/thread.rs, read 2026-10-03, unpinned main, verified), and one log file isn't enough when its context spans earlier rollout prefixes (https://github.com/openai/codex/blob/main/codex-rs/thread-store/src/local/model_context.rs, read 2026-10-03, verified). **Claude:** the Agent SDK's external session storage supports cross-host resume, but mirror writes are best-effort (https://code.claude.com/docs/en/agent-sdk/session-storage, retrieved 2026-10-03, verified). |
| 4 | A running turn continues on another machine | no supported operation was found in the sources examined (inference; lane P5 confirms) |

**Router today (verified):**
- Router is not a transcript store. Its live event replay is in memory (`crates/collaboration-service/src/provider_session_event_hub.rs:1`).
- Tool calls are projected with their title and status only (`crates/acp-client-runtime/src/provider_item_projection.rs`, `observe_tool_call`).

## 3. Safe shipping order (inference, from object-storage guarantees)

```text
 capture a consistent cut ─► upload immutable chunks ─► verify each object
   ─► publish an immutable manifest ─► advance the "latest" pointer, guarded by owner epoch
 restore = the last complete manifest; anything after it is a known gap
```

- **Never advance on partial uploads,** multipart part success, or an event notification.
- **The safe recovery point** is the highest contiguous verified prefix that an accepted manifest references.
- **A command accepted after the last checkpoint and before shipping stays unknown.** Restoring must not assume it never ran.

## 4. Questions for lane P5

1. **Recovery level.** Which level is the goal, per provider? The lean: level 1 for everything now; level 3 as a Claude-only experiment.
2. **Capture cut.** What's a consistent capture point for each provider's native store?
3. **Loss and time goals.** What recovery-point and recovery-time goals, and what retention?
4. **Separate shipping.** How do project and automation stores ship, as distinct from session logs? A snapshot plus a change feed? Which feed is complete, given that acknowledgments produce no activity today?
5. **Bindings.** How do recovered sessions bind to stable session IDs in the fabric registry? How are "new conversation" recoveries labelled?
6. **Turso (R14).** Where does it fit for project and automation store shipping? See [architecture-options.md](architecture-options.md) §3.
