# Session discovery and messaging

Resolve the intended recipient and your sender from actual session references, then send and report the returned evidence. Use an exact supplied address when available; otherwise use the requested title or working directory to find candidates.

```sh
agent-collaboration endpoints list --json
```

Set `ENDPOINT_ID` from that discovery and pass any supplied `--service-directory` on each call. Inventory titles can contain full prompts: save each page to an owner-private scratch file and filter locally before displaying it. Never dump unfiltered session pages into model context or infer absence from truncated output.

```sh
agent-collaboration sessions list --endpoint "$ENDPOINT_ID" --view stored \
  --checkout "$RECIPIENT_CWD" --source interactive --query "$SESSION_QUERY" --json
```

Choose exactly one scope: `--cwd`, `--checkout`, `--repo`, or `--any`; state `--source interactive|subagents|all` as well. Read `result.page.records` and return `result.page.nextCursor` with the same endpoint, view, scope, source, and query until exhausted. `stored` finds saved sessions, including unloaded ones. `loaded` shows sessions currently loaded by that endpoint; `active` is only running work. Absence from loaded or active does not mean a session does not exist; check stored.

Match the requested title/workingDirectory against candidate metadata, then copy that entry's `.target` verbatim. Inspect a candidate with `session inspect --endpoint ID --session ID --json` when more context is needed; `session inspect` also accepts `--to <SessionRef JSON>` like every other session command. A title or cwd is a discovery hint, not a unique identity; ask for an exact target when matches remain ambiguous. If the intended session is missing, report that result. Do not create a replacement session merely to make discovery succeed.

`SessionRef` is `{endpoint:{serviceId,endpointId},sessionId}`. `addresses list` is lifecycle/coverage inventory and its `address.nativeThreadId` shape is not a message target; do not pass that object as --to or --from. Preserve the full `.target`, not a bare UUID, title or hand-renamed field. A session command accepts either `--to` with that exact JSON or both `--endpoint` and `--session`; confirm the selected service supports the pair before using it.

For your sender, prefer the exact self SessionRef supplied by the caller or the matching Intended recipient header of an incoming Router message. In Codex, `CODEX_THREAD_ID` identifies the current native thread: when using it for discovery, match it exactly to `.target.sessionId` in the selected endpoint's session inventory and use the complete matching target. `CODEX_SESSION_ID` may identify a shared root and must not substitute for a different current thread. If you cannot establish your own full address, request it from the caller; never invent a session ID, create a duplicate session, or use a human identity as a workaround.

```sh
agent-collaboration message send --to "$RECIPIENT_ADDRESS" \
  --from "$SENDER_ADDRESS" --text-file "$MESSAGE_FILE" --json
```

Use actual resolved addresses and a content file. `--delivery auto` starts/resumes or steers as needed; `steer` requires active work and `queue` requires a loaded recipient. A queued message is not proof the recipient processed it. For a reply, use the incoming Self-declared sender address as the recipient, while keeping your own sender identity. Router's own deliveries (Batch sets, heartbeats, finalizations from a session listen) arrive with the header `Router delivery` and an intended recipient, no self-declared sender; they carry no reply address and are not replied to. If a reply is needed, include the return address and requested response.

Only for explicitly requested human input:

```sh
agent-collaboration message send --human-user --to "$RECIPIENT_ADDRESS" \
  --text-file "$MESSAGE_FILE" --json
```

The CLI adds the agent declaration; do not add a duplicate header. When terminal-turn evidence is required, start `events listen --endpoint ID --session ID --attach` for the recipient before sending and wait for `listenerReady`. Preserve the receipt's `acceptance.turnId`; correlate that exact recipient and turn with a `turn/completed` event and inspect its status/error. A listener attached after completion may miss it: if no matching terminal evidence is observed, report accepted input with completion unresolved. Queue acceptance can lack a turn ID and still does not prove execution. An actual reply requires a separate incoming message. Use `events listen --help` for observation options and `turn interrupt --help` only for an explicitly requested interruption. Sending or observing must not implicitly interrupt work.

Complete with the exact target and strongest observed stage: accepted input, completed turn or actual reply. Preserve returned IDs and uncertainty; temporary idleness and successful sending alone do not prove a reply or task completion.

## Persistent top-level session flow

Use this route only after the selected service and its `conversation prompt`, `sessions list`, and `session rename` help expose the required contract. If any required capability is unavailable, return that gap; do not create a second recipe track or infer a field shape.

A caller supplies the exact model, effort, and role or assignment context. Router mechanics create or continue one persistent top-level session; the visible name is cosmetic and never replaces its returned `SessionRef`. A new or forked session must appear as `source: interactive` in the capability's scoped result before it is treated as a top-level implementation session. Idle age is observability and cost information only; it neither expires a session nor guarantees a warm cache.

```sh
agent-collaboration conversation prompt --endpoint "$ENDPOINT_ID" --cwd "$WORKING_DIRECTORY" \
  --new --model "$MODEL_ID" --effort "$EFFORT" --access "$ACCESS" \
  --approver "$APPROVER_SESSION_REF" --root-message-id "$ROOT_ID" --text-file "$MESSAGE_FILE" --json

# add --model/--effort only to change them
agent-collaboration conversation prompt --endpoint "$ENDPOINT_ID" --cwd "$WORKING_DIRECTORY" \
  --fork "$SOURCE_SESSION_ID" --access "$ACCESS" \
  --root-message-id "$ROOT_ID" --text-file "$MESSAGE_FILE" --json

agent-collaboration conversation prompt --endpoint "$ENDPOINT_ID" --session "$SESSION_ID" --cwd "$WORKING_DIRECTORY" \
  --text-file "$MESSAGE_FILE" --json
```

For `--new`, set `--access write-restricted|workspace-write` as well as the required model and effort; `--fork` states `--access` and inherits the rest unless a change is deliberate. `write-restricted` permits the project's `tmp/` and `docs/wip/` plus shared scratch; `workspace-write` permits the selected worktree plus shared scratch. `--root-message-id` is optional and names the shared scratch scope by the board root message ID. `--approver` records the client approval authority; it defaults to the creating session. Resume passes the session and cwd only; model, effort, access, root association, and scratch path persist. Pass `--effort` on resume only to change it deliberately: a changed effort or model is a cache-affecting setting, the receipt reports `effortChange`, and the provider prompt cache for that session is not reused. Fork may omit `--model` and `--effort` to inherit the source thread's values. A model name, session name, or board role does not establish assignment write permission.

Router abstracts the native permission arguments. Approval policy and reviewer are inherited from the app-server configuration and saved-session behavior; the CLI does not set them from the access enum or a board role. Setup evidence reports available settings as `observed` with their source and observation time, or `unavailable` with its reason. Do not populate an unavailable field from the request, and do not treat `thread/read` after a turn as a new permission observation. Socket and domain enforcement require proof from the configured runtime; setting presence alone does not establish those capabilities.

## Client-exposed approvals when supported

Use this route only after the selected service exposes `approval list` and `approval decide`, and only for a request that reached the client approver. Approval policy and reviewer come from the app-server and saved-session settings; inspect their available observed setting or unavailability rather than requiring a hardcoded policy. A guardian denial or timeout is final inside Codex and appears to the requesting model as a failed command; Router does not expose it as an approval record, override it, switch reviewer mode, or retry it.

```sh
agent-collaboration approval list --pending --json
agent-collaboration approval decide --request-id "$REQUEST_ID" --allow \
  --actor "$APPROVER_SESSION_REF" --json
```

Choose exactly one of `--allow`, `--allow-for-session`, or `--deny`. The actor must be the configured approver's exact SessionRef, not the requesting session. Router accepts only a still-pending request in the current generation and only a decision that the native request offered; self decisions, another actor, duplicate decisions, expired requests, and old generations are refused. `approval list` records the closed states `pendingClientDecision`, `decided`, `timedOut`, `approverUnreachable`, and `cancelled`. An approver decision records the native choice and, for `--allow-for-session`, its returned scope. It does not prove the requested operation ran or succeeded; inspect the requesting session's later operation evidence separately.

Use your own configured approver identity and decide only within authority the owner delegated for this task and the host permits. Route a request beyond that authority to the owner. Matching an actor field is not a new grant, and do not substitute another identity.

## Name a session

Find the exact target first, then name it. The caller supplies the visible name, including an emoji when desired; name and title are display fields, while the returned target remains identity. List records are `result.page.records`, a single read is `result.record`, and mutations return `result.record` plus `effects`. Inspect the returned record before treating the name or target as confirmed.

```sh
agent-collaboration sessions list --endpoint "$ENDPOINT_ID" --view stored \
  --checkout "$WORKING_DIRECTORY" --source interactive --query "$SESSION_QUERY" --json
agent-collaboration session rename --endpoint "$ENDPOINT_ID" --session "$SESSION_ID" \
  --name "$VISIBLE_SESSION_NAME" --json
```

If help or service behavior lacks any required flag, field, assignment access, or identity proof, return the exact capability gap. Do not create a substitute session, fall back to another model, or make the name stand in for the target.

## Continuing an existing conversation

The caller supplies an existing target and required model/access capability. A caller-requested continuation reuses its exact SessionRef. A native child's inspectable ID alone does not establish that Router accepts direct input; inspect capability before promising steering.

For a recorded SessionRef, inspect the exact target and use it unchanged for the next authorized message. If missing or unavailable, return the discrepancy to the caller; do not create a duplicate or change authorship/delivery mode to force success. A title, board-thread root, ACPX relationship name and ACPX record ID are not interchangeable with SessionRef. Map an ACPX provider-native session to Router only when discovery verifies that same conversation; otherwise return the mapping gap to the caller.

If the target or required capability is unavailable, report the exact gap. Do not create a replacement or change delivery mode to force a route.
