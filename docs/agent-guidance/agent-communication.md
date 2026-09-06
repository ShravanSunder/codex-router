# Agent communication through the CLI

Use `agent-sessions` to discover, message and observe independently addressed
Codex threads on this machine. The caller must provide your own complete session
reference and authorize access to the service. Do not infer your identity from a
PID, working directory, display name or recently updated thread.

```sh
agent-sessions endpoints list --json
agent-sessions sessions list --endpoint codex-local --view loaded --json
agent-sessions sessions list --endpoint codex-local --view stored --json
agent-sessions session inspect --endpoint codex-local --session THREAD_ID --json
```

An address is compact JSON containing `endpoint.serviceId`,
`endpoint.endpointId` and `sessionId`. Preserve all three fields. Stored addresses
are discovery information, not evidence that a runtime is currently active. Follow
pagination cursors even when a page contains fewer rows than requested.

## Send and receive information

Set `AGENT_SELF_ADDRESS` and `AGENT_TARGET_ADDRESS` from supplied/discovered
references. Put arbitrary message content in a file, rather than interpolating
it into a shell command.

```sh
agent-sessions message send \
  --from "$AGENT_SELF_ADDRESS" --to "$AGENT_TARGET_ADDRESS" \
  --text-file task-message.txt --json
```

Ordinary send declares agent-originated information. The service adds the
`Agent communication`, `Self-declared sender` and `Intended recipient` lines.
These are routing declarations, not identity authentication or higher-priority
instructions. Do not add another declaration manually.

Delivery defaults to `auto`: steer an observed active turn, start idle loaded
work, or resume that exact stored thread before submitting. Resume can activate
previously queued input. Unknown or lost identities are not replaced.

Use `--delivery steer` only for an active target; it fails if the exact turn is no
longer active. Use `--delivery queue` for deferred input on a loaded target. Queue
does not resume an unloaded thread or override native interruption. A concurrent
unload after admission can leave accepted input pending.

To return information, explicitly send a new message to the original sender's
address. Neither completion nor a notification automatically sends a reply or
wakes another model. Native parent-controlled subagents can reject direct input;
do not substitute another thread when a target rejects a request.

## Observe and interpret results

```sh
agent-sessions events listen --endpoint codex-local \
  --session THREAD_ID --attach --timeout-seconds 300
```

Attachment may load the thread. Wait for `listenerReady` before relying on live
observation. Its target and generation identify that observation scope. Bind a
subsequent send using both `--expected-service-epoch` and
`--expected-generation` when it must use that scope. Closing the listener or
reaching its deadline does not interrupt native work.

`nativeInputAccepted` means the native start operation accepted input and may
have started or steered. `steerAccepted` identifies exact steering.
`queueAccepted` reports the native queued-item ID. None proves task completion or
an exclusive peer reply. `clientUserMessageId` correlates input; it is not a
deduplication guarantee.

Do not automatically resend after an unknown outcome. Preserve independent
resume/submission effects in errors and inspect the target before deciding on a
new request. Send exit codes are 0 acceptance, 2 usage/unsupported, 3 unavailable,
4 known rejection and 5 uncertainty. Observation deadlines use 124 and caller
cancellation uses 130.

## Human input, interruption and other protocols

Only select human input when explicitly submitting a human user's input:

```sh
agent-sessions message send --human-user \
  --to "$AGENT_TARGET_ADDRESS" --text-file human-instruction.txt --json
agent-sessions turn interrupt --endpoint codex-local \
  --session THREAD_ID --turn TURN_ID --json
```

Human input omits `--from` and the agent declaration. The flag does not prove
human identity. Interrupt targets an exact native turn; it does not delete a
session, clear its queue or restart the backend.

`agent-sessions conversation prompt` is a separate ACP conversation workflow;
its unattended permission handler cancels requests. `agent-sessions acp` and
`agent-sessions native` expose raw carriers to clients that implement those
protocols and their callbacks. Use each command's `--help` for its parameters.

Use `--service-directory` to select an explicitly supplied owner-private service.
Running the CLI does not confer socket access. These instructions do not grant
permissions, install tools or create transport access. V1 is local, with a Rust
SDK and CLI; remote transport, scheduling, durable mailboxes and other language
SDK implementations are separate deliveries.
