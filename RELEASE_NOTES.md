# Release Notes

## 0.1.28 - 2026-09-16

- Require explicit model, effort, and access choices for new and forked Codex sessions; preserve those choices through resume and report source-labeled native settings evidence.
- Add `write-restricted` and `workspace-write` access with owner-private shared scratch, explicit project work areas, inherited approval behavior, and supported exact Control-socket configuration without broad Unix-socket grants.
- Add persisted session rename and explicitly scoped session discovery by cwd, checkout, repository, source, and query, with explicit names separated from derived titles.
- Standardize finite collaboration CLI JSON results on `result.page.records` for lists, `result.record` for reads, and `result.record` plus `result.effects` for mutations. Remove the former top-level result fields `sessions`, `message`, and `thread`; every result now identifies its CLI and service versions.
- Accept either `--to <SessionRef JSON>` or `--endpoint` plus `--session` across commands that target a session, resolve `self` consistently on board actor and reader flags, reject forbidden C0 bytes at text write boundaries, and expose reasoned native rejections.
- Add repository-scoped board thread discovery across every project attached to an explicit repository path.
- Add the unique `implementer` Thread seat and report both Orchestrator and Implementer holders on Thread and Participant reads.
- Add Topic Watch and Topic Listen selection, including roots created after arming. Thread Listens now use fixed `short` and `long` lifetimes, a five-minute debounce with a twenty-minute cap, terminal finalization records, and Codex-only `--deliver session` background delivery with silent-mark heartbeats.

## 0.1.27 - 2026-09-15

- Add explicit Thread Participants with closed Roles (`orchestrator`, `advisor`, `reviewer`, `participant`), one open Orchestrator, sequence-based presence, and Participant listing on Thread reads.
- Add `board thread create`, `join`, `leave`, and `participant list`, with explicit Watch and Listen choices, `--actor self` for Codex and Claude Code sessions, and corrective `nextAction` details on refusals.
- Require session Participants for Thread replies and Listens and the session Orchestrator for resolution. Humans remain able to post, Listen, and resolve without joining; human resolution closes every open Participant.
- Stop message posting from changing Watch state. Session topic-placement posts now direct callers to `thread create`; human topic-placement posts remain available without creating a Participant.

## 0.1.26 - 2026-09-14

- Add process-owned `board thread listen` for watched or named Threads, with Once and Repeating modes, durable Delivered positions, explicit `--acknowledge` or `--no-acknowledge`, and JSON Batch sets on stdout. Once Listens require `--max-wait <duration>`.
- Use the Control protocol's long-poll `ThreadWait` fallback because Control 1.0 has one response per request; the CLI loops that request for Repeating Listens.
- Coalesce Activity with the fixed `THREAD_LISTEN_DEBOUNCE = 30s` window and `THREAD_LISTEN_DEBOUNCE_CAP = 120s` cap.
- Restore `agent-session` as the singular standalone executable name for the session picker while `agent-collaboration` remains the collaboration and board CLI.

## 0.1.2

- `codex-router sessions --new` starts a fresh Codex session through the router profile.
- `codex-router sessions` always offers a `New Codex session` picker choice.
- Session launches pass trailing Codex flags through, including `--yolo`, for both new and resumed sessions.
- Legacy router-owned `sessions --scope` remains rejected; use `--checkout`, `--repo`, or `--any`.
