---
name: agent-communication
description: Use Codex Router to discover agents, send direct messages, participate in shared message boards, and manage communication receipts and activation. Use when coordinating through Router, rather than for general coding or native parent-controlled subagents.
---

# Agent communication

Router provides communication and automation around harness-owned conversations. Use the `agent-sessions` CLI; a successful request is not necessarily completed work or a reply.

## Choose the action

- Discover a recipient or send information now: read [session messaging](references/session-messaging.md).
- Send information later or periodically: read [timed wake-ups](references/timed-wakeups.md).
- Execute reusable instructions on a schedule: read [scheduled workflows](references/scheduled-workflows.md).
- Inspect a failed, delayed, or uncertain operation: read [receipt recovery](references/receipt-recovery.md).

Wait and repeat intervals are either under the 29-minute prompt-cache ceiling or a real calendar schedule (day-scale or cron). Mid-range waits such as 45 minutes are not a third option unless the recipient is Mini.

Check `agent-sessions --help` and the relevant subcommand help against these examples. They describe the 0.1.18 CLI surface. If a command or capability is missing, report the mismatch rather than invent flags or install/upgrade software without authorization.

Use `--json` for control operations. Discover exact addresses instead of guessing from titles. Preserve returned IDs for subsequent inspection. Prefer `--text-file` for multiline content; quote shell arguments and never interpolate message content as shell code.

Agent input is the normal communication path. `--human-user` explicitly submits human input; it is not a workaround for an agent message failure. Sender identity is self-declared, not authenticated. Replies are explicit messages from the recipient, not an automatic consequence of sending.

Delivery mode is independent of message authorship: `auto` starts/resumes or steers active work; `steer` requires active work; `queue` requires a loaded thread. Do not silently replace a requested mode after a precondition error. Stopping work is the separate `turn interrupt` operation targeting an exact turn.

Report the strongest observed evidence: saved, fired, accepted, completed, or replied. An uncertain submission may have succeeded: inspect before sending again.

Use the requested service/profile. If the agent sandbox cannot reach it, report that access failure; do not bypass it, redirect to production, or restart services. This skill does not grant authority to send messages or change schedules beyond the user's task.
