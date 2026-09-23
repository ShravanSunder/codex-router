# Shared work on boards

A board preserves discussion across sessions. Use the existing work thread so decisions, questions, evidence, and continuation stay together. `track-show-me-your-work` owns which milestones deserve a durable update; tool descriptions and schemas own the calls.

## Find and resume the work

Reuse a supplied discussion reference after checking its service and subject. Otherwise discover projects associated with the repository and read their descriptions and relevant discussions before choosing a home. A repository can belong to several projects; do not select the first match when the work's location is ambiguous.

The user controls project and board organization. Agents may organize topics and threads inside authorized boards. An empty search does not authorize creating a new project or board.

Read the root and relevant history before continuing, including returned pagination. Retain the exact work reference for handoff. A new session does not imply a new discussion, but it does have its own participation and reader state.

## Participate in the assigned role

Join the work thread with the role supplied by the calling workflow. Reading, posting, and watching are distinct from joining. Use your verified session identity; do not substitute a human actor to avoid a participation restriction.

Seats are local to each thread. The calling workflow maps agent functions to these seats:

- `orchestrator`: the agent responsible for the thread; may resolve it or hand it over.
- `implementer`: the continuing implementation participant.
- `advisor` and `reviewer`: the assigned advisory or review participants.
- `participant`: other contributors.

There is at most one open Orchestrator and one open Implementer per root. A seat does not grant design, execution, filesystem, or merge authority. Different assignment threads may have different implementers.

When the caller supplies coordination and execution roots, keep assignment discussion on its execution root and integration decisions on the coordination root. Relate discussions through existing message references; do not invent a hierarchy or registry. Correct a posted mistake with a new referenced message rather than pretending the old content changed.

## Watch, listen, and acknowledge

Watching selects future activity. Listening delivers selected activity. Neither means a recipient has understood or finished work.

For a real reply dependency, prefer supported session delivery: arm once, retain the listener identity, report it active, and yield. Use a bounded call/process wait when session delivery is unavailable. Activity is batched and debounced, so immediate silence is not failure. Do not create a second listener or keep checking lists while the first is active. A heartbeat needs no action.

Process received activity before acknowledging its exact scope. Fetching a page, receiving a batch, and acknowledging it are different operations. Your own posts and watch-start boundaries affect what appears unread; use history when older context is needed.

After listener finalization, inspect its reason and any delivery rejection before deciding whether to re-arm. Cancel an obsolete listener when the dependency ends. A timeout or cancelled wait does not prove the other agent stopped.

## Finish the discussion

Post the meaningful outcome with evidence and remaining work. A contributor finishing its assignment does not resolve the whole coordination thread. The responsible Orchestrator resolves completed work or explicitly hands it over; otherwise leave the thread open with a useful continuation point.
