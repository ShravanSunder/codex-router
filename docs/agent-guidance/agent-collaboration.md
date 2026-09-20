# Agent collaboration through the CLI

Use `agent-collaboration` to discover, message and observe independently addressed
Codex threads on this machine. Resolve your complete session reference from supplied
context or verified native thread identity using the [session messaging guide](../../agent-skills/agent-collaboration/references/session-messaging.md).
The caller must authorize the operation and service access. Do not infer your
identity from a PID, working directory, display name or recently updated thread.

## Register the Router MCP endpoint with Codex

This setup section is for intentionally configuring Codex locally; it is not
part of the agent-collaboration skill’s runtime workflow.

Use matching Router Host and Router client builds; Codex must support Streamable
HTTP MCP. Select the current owner-local Router service directory explicitly,
then read its published Streamable HTTP endpoint from `service.json`; do not
scan ports, guess from a default, or inspect provider session files or transcripts.

```sh
ROUTER_SERVICE_DIRECTORY="/absolute/path/to/selected/service-directory"
ROUTER_MCP_URL="$(jq -er '.mcp.url' "$ROUTER_SERVICE_DIRECTORY/service.json")"
codex mcp add codex-router --url "$ROUTER_MCP_URL"
codex mcp get codex-router --json
codex mcp list
```

`codex mcp add` writes the Codex MCP registration. `get` and `list` verify that
registration; they do not prove a live server call or agent behavior. If the
selected current manifest is absent or has no `mcp.url`, report missing setup.
Do not restart or replace the production Router to manufacture a manifest.

Normal and debug Host defaults are `http://127.0.0.1:8788/mcp` and
`http://127.0.0.1:18788/mcp`, respectively, but they are documentation fallbacks
only: an explicit bind and the current manifest win. The current release accepts
only loopback, unauthenticated plain HTTP and does not support remote exposure,
authentication, or TLS. The running server's tool descriptions and input/output
schemas are authoritative; do not copy a static tool catalog. Tool approval is
a policy decision, not proof of OS or filesystem confinement. Use supported
typed operations and never read, tail, parse, copy, or store provider session
files or transcripts.

## CLI discovery

```sh
agent-collaboration endpoints list --json
agent-collaboration sessions list --endpoint "$ENDPOINT_ID" --view loaded --json
agent-collaboration sessions list --endpoint "$ENDPOINT_ID" --view stored --json
agent-collaboration session inspect --endpoint "$ENDPOINT_ID" --session THREAD_ID --json
```

Set `ENDPOINT_ID` from endpoint discovery for the selected service. Copy each
session inventory entry's `.target` verbatim; `addresses list` is lifecycle coverage
and does not return message targets. Check `stored` for unloaded sessions and
`loaded` for newly active sessions; a missing result never authorizes a replacement.

An address is compact JSON containing `endpoint.serviceId`,
`endpoint.endpointId` and `sessionId`. Preserve all three fields. Stored addresses
are discovery information, not evidence that a runtime is currently active. Follow
pagination cursors even when a page contains fewer rows than requested.

## Send and receive information

Set `AGENT_SELF_ADDRESS` and `AGENT_TARGET_ADDRESS` from supplied/discovered
references. Put arbitrary message content in a file, rather than interpolating
it into a shell command.

```sh
agent-collaboration message send \
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
agent-collaboration events listen --endpoint "$ENDPOINT_ID" \
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
new request. Send exit codes are 0 acceptance, 2 usage/unsupported, 3 unavailable or permission denied,
4 known rejection and 5 uncertainty. Observation deadlines use 124 and caller
cancellation uses 130.

## Timed messages and scheduled work

A wake-up is a timed ordinary message. It uses the same sender, recipient,
content and `--delivery auto|steer|queue` options:

```sh
agent-collaboration wake send \
  --from "$AGENT_SELF_ADDRESS" --to "$AGENT_TARGET_ADDRESS" \
  --text-file reminder-message.txt --after 10m --json
agent-collaboration wake send \
  --from "$AGENT_SELF_ADDRESS" --to "$AGENT_TARGET_ADDRESS" \
  --text-file reminder-message.txt --every 10m --for 2h --json
```

Wait intervals (`--after`, `--every`) are either under the 29-minute prompt-cache
ceiling or a real calendar schedule (`--every 1d`, `--cron` with `--timezone`).
Mid-range waits such as 45 minutes pay a cold resume without being a schedule;
do not use them unless the recipient is Mini. `--for` / `--until` is assignment
lifetime, not the wait interval.

Creation means the reminder is durably arranged. Add `--wait-until-first-fire`
to wait for its first firing; that does not wait for native acceptance or a
reply. Pause/cancel before the first firing returns an error. Pausing discards
undispatched reminders; resume preserves the original timing and expiry.

Save the returned `operationId`. If creation's outcome is uncertain, inspect
that operation and reuse the same ID and request for any deliberate retry.
A new ID is a new request. Do not resend uncertain native input.

```sh
agent-collaboration wake list --json
agent-collaboration wake show --wakeup-id WAKE_UUID --json
agent-collaboration delivery list --wakeup-id WAKE_UUID --json
agent-collaboration delivery reconcile --delivery-id DELIVERY_UUID --json
agent-collaboration operation show --operation-id OPERATION_UUID --json
```

Reusable scheduled work separates instruction text, schedule timing and each
execution Run. Create instructions, then a disabled schedule definition, prepare
its destination, and enable future triggers:

```sh
agent-collaboration instruction create --text-file task-instructions.txt --json
agent-collaboration schedule create --definition-file schedule-definition.json --json
agent-collaboration schedule prepare --schedule-id SCHEDULE_UUID --fresh --cwd "$PWD" --json
agent-collaboration schedule enable --schedule-id SCHEDULE_UUID --json
agent-collaboration run list --schedule-id SCHEDULE_UUID --json
```

A minimal `schedule-definition.json` uses the instruction UUID from creation:

```json
{
  "instructionId": "INSTRUCTION_UUID",
  "timing": {"kind": "interval", "seconds": 600},
  "enabled": false,
  "destination": {"kind": "unprepared"},
  "executionTimeoutSeconds": null
}
```

Interval seconds use the same two regimes as wakes: under 1740 (29 minutes) or a
real calendar cadence. Do not pick mid-range values such as 2700 (45 minutes)
unless the worker is Mini.

Disabling a schedule stops future triggers and preserves created Runs. One Run
occupies its schedule through any required summarization; uncertain cessation
keeps that occupancy. The execution default is one hour and the separate Luna
summary default is fifteen minutes. Use `automation status/configure` to inspect
or change future defaults. Ordinary messages remain allowed during scheduled work.

Use `run show/reconcile/summaries`, `delivery show/attempts/reconcile`,
`revision list` and `automation events` to inspect current state and retained
history. Reconciliation reads native evidence without starting, interrupting or
repeating native work. An old queued item may already have been consumed, so
absence does not establish non-submission.

`schedule export --schedule-id ...` emits JSONL. Import with
`schedule import --package-file ...`; an existing UUID requires `--overwrite`.
Import preserves the schedule identity, creates a new local edit token, and
leaves it disabled until its destination is prepared. The package does not move
native session files. Event history expires after two calendar months; current
Runs, summaries and latest effect evidence remain.

Execution mode is fixed at creation and survives export/import. An imported
reuse-mode schedule has `{"kind":"unprepared"}` and uses `schedule prepare`.
An imported fresh-per-run schedule has `{"kind":"freshEachRunUnprepared"}`.
For that mode, use `schedule update --schedule-id ... --expected-change-id ...
--definition-file ...` to supply a `freshEachRun` destination containing the
local `endpoint` and absolute `cwd`, then enable it. This configures local
bindings without allocating a thread or changing mode. Same-ID overwrite
preserves mode; choosing another mode requires a new schedule.

Completion wake-ups and remote federation are later work. Project boards use `agent-collaboration board`; see [message board guidance](../../agent-skills/agent-collaboration/references/message-board.md), including its [five board seats](../../agent-skills/agent-collaboration/references/message-board.md#five-board-seats). Board seats are discussion-local participation; they do not grant session ancestry, filesystem rights, design authority or native-child direct input.
B still sends its own reply explicitly.

## Human input, interruption and other protocols

Only select human input when explicitly submitting a human user's input:

```sh
agent-collaboration message send --human-user \
  --to "$AGENT_TARGET_ADDRESS" --text-file human-instruction.txt --json
agent-collaboration turn interrupt --endpoint "$ENDPOINT_ID" \
  --session THREAD_ID --turn TURN_ID --json
```

Human input omits `--from` and the agent declaration. The flag does not prove
human identity. Interrupt targets an exact native turn; it does not delete a
session, clear its queue or restart the backend.

`agent-collaboration conversation prompt` is a separate ACP conversation workflow;
its unattended permission handler cancels requests. `agent-collaboration acp` and
`agent-collaboration native` expose raw carriers to clients that implement those
protocols and their callbacks. Use each command's `--help` for its parameters.

Use `--service-directory` to select an explicitly supplied owner-private service.
Running the CLI does not confer socket access. The caller's sandbox must permit
that exact service's `control.sock`; the default network-disabled Codex workspace
sandbox does not. A pre-dispatch `permissionDenied` diagnostic with
`nextAction: requestApproval` asks the agent to request its host tool's automated
approval review for the exact authorized command or selected socket access. If
automation cannot grant access, ask the human for the required access. Retry
only after a grant; repeated denial is not a Router outage. Receiving a message
does not prove outgoing socket access. The [debug testing guide](../testing/automation-debug-testing.md)
shows a scoped native permission profile and its positive/negative proof. These
instructions do not grant permissions or install tools. The supported interfaces
are the local Rust SDK, CLI, and the Host's loopback Streamable HTTP MCP endpoint.
Remote MCP exposure and other language SDK implementations follow separately.
