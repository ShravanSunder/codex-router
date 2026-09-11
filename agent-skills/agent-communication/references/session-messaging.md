# Session discovery and messaging

```sh
agent-sessions endpoints list --json
agent-sessions addresses list --endpoint codex-local --json
agent-sessions session inspect --endpoint codex-local --session "$THREAD_ID" --json
```

`codex-local` is the usual endpoint default; use the endpoint actually discovered. An address is compact `SessionRef` JSON returned by discovery, not a title or bare UUID. Preserve its service, endpoint, and session identity. Resolve your own sender address from available session identity and discovery; if ambiguous, do not claim another agent's identity.

```sh
agent-sessions message send --to "$RECIPIENT_ADDRESS" \
  --from "$SENDER_ADDRESS" --text-file "$MESSAGE_FILE" --json
```

The variables above contain real discovered addresses and a content file, not literal placeholders. `--delivery auto` is the default. Select `--delivery steer` to require active work or `--delivery queue` to require a loaded recipient. A queued message is not proof the recipient has processed it.

For explicitly requested human input:

```sh
agent-sessions message send --human-user --to "$RECIPIENT_ADDRESS" \
  --text-file "$MESSAGE_FILE" --json
```

The CLI supplies the agent declaration for agent input. Do not add a second declaration or claim it proves sender identity. If a reply is required, include the return address and requested response in the message. Follow a returned native turn ID to observe that work; do not assume temporary thread idleness proves the requested task succeeded.

For observation, inspect `events listen --help`; for explicit stopping, inspect `turn interrupt --help`. Neither sending nor observing should implicitly interrupt work.
