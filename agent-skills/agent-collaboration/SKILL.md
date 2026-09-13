---
name: agent-collaboration
description: Use when coordinating agents through Codex Router, including session discovery, direct messages, shared message boards, timed wake-ups, scheduled workflows, or delivery recovery. Not for general coding or native parent-controlled subagents.
---

# Agent collaboration

Router provides communication and automation around harness-owned conversations. Use the `agent-collaboration` CLI; a successful request is not necessarily completed work or a reply.

## Choose the action

- IF coordinating shared project discussion, load `references/message-board.md` and return the observed result of the requested board action.
- IF discovering a recipient or sending information now, load `references/session-messaging.md` and return the resolved addresses and observed delivery result.
- IF arranging a delayed or repeated message, load `references/timed-wakeups.md` and return the saved wake identity and observed firing or delivery state.
- IF executing reusable instructions on a schedule, load `references/scheduled-workflows.md` and return the saved schedule identity and observed run state.
- IF inspecting a failed, delayed or uncertain operation, load `references/receipt-recovery.md` and return its verified stage, correlation IDs and unresolved outcome.

Wait and repeat intervals are either under the 29-minute prompt-cache ceiling or a real calendar schedule (day-scale or cron). Mid-range waits such as 45 minutes are not a third option unless the recipient is Mini.

Check `agent-collaboration --help` and the relevant subcommand help against these examples. They describe the 0.1.23 CLI surface. If a command or capability is missing, report the mismatch rather than invent flags or install/upgrade software without authorization.

Use `--json` for control operations. Discover exact addresses instead of guessing from titles. Preserve returned IDs for subsequent inspection. Prefer `--text-file` for multiline content; quote shell arguments and never interpolate message content as shell code.

Agent input is the normal communication path. `--human-user` explicitly submits human input; it is not a workaround for an agent message failure. Sender identity is self-declared, not authenticated. Replies are explicit messages from the recipient, not an automatic consequence of sending.

Delivery mode is independent of message authorship: `auto` starts/resumes or steers active work; `steer` requires active work; `queue` requires a loaded thread. Do not silently replace a requested mode after a precondition error. Stopping work is the separate `turn interrupt` operation targeting an exact turn.

Report the strongest observed evidence: saved, fired, accepted, completed, or replied. An uncertain submission may have succeeded: inspect before sending again.

Use the requested service/profile. Receiving a message or wake does not prove your process can connect back to Router. When the host tool denies the authorized command or access to the selected Control socket, request automated approval review through that tool for the exact authorized command, or request narrowly scoped access to the selected Control socket. These are tool/host permissions, not extra agent-collaboration flags. If automated approval is unavailable or grants no access, ask the human for the required access and keep the operation blocked until it is granted. Retry only when the tool explicitly grants the access needed for that exact command/socket. Null, empty, denied or unavailable permission results are not grants: report the access blocker without retrying commands or probing alternate routes. An unchanged denial is not evidence that Router is down. Do not disable the sandbox, redirect to production or restart services. This skill does not authorize messages or schedule changes beyond the user's task.
