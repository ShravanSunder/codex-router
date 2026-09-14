# Session discovery and messaging

Resolve the intended recipient and your sender from actual session references, then send and report the returned evidence. Use an exact supplied address when available; otherwise use the requested title or working directory to find candidates.

```sh
agent-collaboration endpoints list --json
```

Set `ENDPOINT_ID` from that discovery and pass any supplied `--service-directory` on each call. Inventory titles can contain full prompts: save each page to an owner-private scratch file and filter locally before displaying it. Never dump unfiltered session pages into model context or infer absence from truncated output. For a working-directory lookup, set `RECIPIENT_CWD` from the task and `SELF_SESSION_ID` from your supplied self reference or current `CODEX_THREAD_ID` (see sender resolution below); leave the latter empty for recipient-only discovery.

```sh
INVENTORY_PAGE=$(mktemp)
agent-collaboration sessions list --endpoint "$ENDPOINT_ID" --view stored --json > "$INVENTORY_PAGE"
jq --arg cwd "$RECIPIENT_CWD" --arg self "${SELF_SESSION_ID:-}" \
  '.result | {nextCursor, sessions: [.sessions[] | select(.workingDirectory == $cwd or .target.sessionId == $self) | {target, workingDirectory}]}' \
  "$INVENTORY_PAGE"
```

For title hints, filter on the title locally and display only a short identifying excerpt with each matching target. Keep the full page on disk for cursor extraction. `stored` finds saved sessions, including unloaded ones. `loaded` shows sessions currently loaded by that endpoint; `active` is only running work. Absence from loaded or active does not mean a session does not exist; check stored. A newly created session may appear in loaded before its stored metadata is available. Follow `nextCursor` with `--cursor` until the chosen view is exhausted; keep endpoint and view unchanged while paging.

Match the requested title/workingDirectory against candidate metadata, then copy that entry's `.target` verbatim. Inspect a candidate with `session inspect --endpoint ID --session ID --json` when more context is needed. A title or cwd is a discovery hint, not a unique identity; ask for an exact target when matches remain ambiguous. If the intended session is missing, report that result. Do not create a replacement session merely to make discovery succeed.

`SessionRef` is `{endpoint:{serviceId,endpointId},sessionId}`. `addresses list` is lifecycle/coverage inventory and its `address.nativeThreadId` shape is not a message target; do not pass that object as --to or --from. Preserve the full `.target`, not a bare UUID, title or hand-renamed field.

For your sender, prefer the exact self SessionRef supplied by the caller or the matching Intended recipient header of an incoming Router message. In Codex, `CODEX_THREAD_ID` identifies the current native thread: when using it for discovery, match it exactly to `.target.sessionId` in the selected endpoint's session inventory and use the complete matching target. `CODEX_SESSION_ID` may identify a shared root and must not substitute for a different current thread. If you cannot establish your own full address, request it from the caller; never invent a session ID, create a duplicate session, or use a human identity as a workaround.

```sh
agent-collaboration message send --to "$RECIPIENT_ADDRESS" \
  --from "$SENDER_ADDRESS" --text-file "$MESSAGE_FILE" --json
```

Use actual resolved addresses and a content file. `--delivery auto` starts/resumes or steers as needed; `steer` requires active work and `queue` requires a loaded recipient. A queued message is not proof the recipient processed it. For a reply, use the incoming Self-declared sender address as the recipient, while keeping your own sender identity. If a reply is needed, include the return address and requested response.

Only for explicitly requested human input:

```sh
agent-collaboration message send --human-user --to "$RECIPIENT_ADDRESS" \
  --text-file "$MESSAGE_FILE" --json
```

The CLI adds the agent declaration; do not add a duplicate header. When terminal-turn evidence is required, start `events listen --endpoint ID --session ID --attach` for the recipient before sending and wait for `listenerReady`. Preserve the receipt's `acceptance.turnId`; correlate that exact recipient and turn with a `turn/completed` event and inspect its status/error. A listener attached after completion may miss it: if no matching terminal evidence is observed, report accepted input with completion unresolved. Queue acceptance can lack a turn ID and still does not prove execution. An actual reply requires a separate incoming message. Use `events listen --help` for observation options and `turn interrupt --help` only for an explicitly requested interruption. Sending or observing must not implicitly interrupt work.

Complete with the exact target and strongest observed stage: accepted input, completed turn or actual reply. Preserve returned IDs and uncertainty; temporary idleness and successful sending alone do not prove a reply or task completion.

## Continuing managed conversations

`manage-agents` selects roles, models, permissions and persistence. Sidekicks and Advisors use separate persistent conversations; a follow-up or new assignment within that relationship reuses its exact SessionRef. Workers may be native children or separate conversations. A native child's inspectable ID alone does not establish that Router accepts direct input; inspect capability before promising steering.

For a recorded Router relationship, inspect the exact target and use it unchanged for the next authorized message. If missing or unavailable, return the discrepancy for lifecycle recovery through management; do not create a duplicate or change authorship/delivery mode to force success. A title, board-thread root, ACPX relationship name and ACPX record ID are not interchangeable with SessionRef. Map an ACPX provider-native session to Router only when discovery verifies that same conversation; otherwise continue through its existing ACPX transport.

This skill does not promise a general coding-session creation command. On the documented installed surface, `conversation prompt --new` cancels permission requests and has no model/effort controls; it is not a substitute for a configured Sidekick launcher. Management uses a supported named ACPX session or an already established authorized Router conversation. If a newer endpoint exposes suitable creation, check its actual contract and returned identity first; if no route satisfies the assignment, report the exact capability gap.
