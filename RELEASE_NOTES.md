# Release Notes

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
