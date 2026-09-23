# PR2 External ACP Requirements

## Purpose

PR2 extends the Router conversation surface to external ACP providers, beginning with Claude Code and Cursor. It preserves the existing Router conversation model while using provider-owned ACP runtimes and provider-owned settings.

## Owner-confirmed outcomes

- Conversation operations come before automation. Scheduling and timed wake delivery are PR3.
- The supported operations are create, prompt, resume/load where the provider advertises it, permissions, cancellation, and internal live updates.
- A normal caller disconnect detaches from work; it does not cancel provider work.
- A provider binding is admitted for mutating work only when it can keep that work alive after caller detach; unsupported bindings fail before mutation rather than silently weakening detach semantics.
- Explicit cancellation is a separate operation and is cooperative.
- If a crash or disconnect leaves the provider outcome unknown, Router retains uncertainty for explicit reconciliation and never auto-replays.
- Router may retain a metadata-only operation record for reconciliation, but does not read, tail, parse, copy, or store provider session JSONL or transcripts.
- Agents coordinate through the existing collaboration board and `listen`; ACP updates are an internal Router mechanism, not a new public observe feature.
- Provider collaboration tools are supplied through MCP where the provider supports that configuration.

## Bounded TODO track

The existing work-thread TODO is **PR2 external ACP provider design and implementation planning** for Claude Code and Cursor. Its required content is provider capabilities, lifecycle, provider-owned session mapping, permissions, and failure behavior. Current launch/configuration/capability evidence and the MCP handoff are supporting inputs to that TODO; they are not a separate product scope.

## Boundaries

PR2 does not add scheduling, timed wakes, a transcript/session-log store, a second task system, automatic replay or replacement, worktree provisioning, native-subagent control, remote MCP exposure, or provider-specific persistence. A small Router-neutral operation record is allowed only for operation identity, provider binding, target, stage, effect, and reconciliation state. It does not contain prompts, replies, tool output, or transcripts. PR2 does not claim that an ACP permission callback provides OS/filesystem confinement.

## Provider strategy

- Claude uses the maintained `claude-agent-acp` bridge over the Claude Agent SDK and native Claude Code runtime. The Claude CLI itself is not the ACP transport.
- Cursor uses the installed `agent acp` command and its ACP implementation.
- Router uses the official Rust ACP client SDK for transport and typed protocol handling. It does not hand-write a second JSON-RPC implementation.
- Unsupported provider capabilities remain explicit unsupported or unverified results; Router must not silently manufacture stronger guarantees.
