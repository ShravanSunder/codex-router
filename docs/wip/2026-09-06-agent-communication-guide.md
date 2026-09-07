# Local agent communication guide

This guide describes the current feature worktree, not a released installation. The [Specification](../specs/2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md) owns the complete contract. Final regression, schema/release packaging and independent implementation review still gate PR readiness. The implementation is grouped into one PR so native and ACP paths can be tested from the same build.

## Find the service and target

The Host stores remembered thread addresses, observed thread/server status, and the rolling lifecycle event log in `session-registry.sqlite` inside its communication service directory. Native Codex history and provider-routing state retain their own storage.

With the isolated debug Host running and `CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET` set to that Host’s dedicated backend socket, run `target/debug/agent-sessions` in a terminal to open the picker. Use `target/debug/agent-sessions --local` for browsing without hosted status. Its default scope is **Repo** and its default runtime view is **All**. The Status column precedes the existing Upd and New columns:

- `◆ Blocked`: waiting for approval or user input.
- `● Active`: running without either waiting flag.
- `○ Idle`: confirmed idle by the selected Host's app-server.
- Unknown, not loaded and system error remain visible only in All. Unknown is not an idle claim.

Ctrl+T cycles All → Blocked → Active → Idle; Ctrl+S cycles repository/all/current-directory scope; Ctrl+O switches Updated/Created sorting. Ctrl+R refreshes. Ctrl+/ or F1 toggles detailed help; Esc closes help first. Search and selection remain stable across background refreshes when the selected row still matches. Interactive discovery includes all thread sources.

Live status refreshes in the background from the selected local service. Stored history remains browseable when the service is unavailable, with live status shown as Unknown. `--local` does not claim hosted status. Browsing does not resume threads or submit work; Enter still explicitly opens the selected thread through the existing native launch path.

```sh
target/debug/agent-sessions endpoints list --json
target/debug/agent-sessions sessions list --endpoint codex-local --view stored --json
target/debug/agent-sessions addresses list --endpoint codex-local --json
```

A full address is a compact JSON `SessionRef` containing `endpoint.serviceId`, `endpoint.endpointId` and `sessionId`. Copy the target from discovery. An address-book entry may be historical; inspect the target before treating it as active. A sender must be given its own full reference by the caller. Never infer self-address from a directory, PID, display name or most recently updated thread.

Stored inventory reads the native catalog without loading threads and records discovered references in the separate lifecycle journal. A page can contain fewer rows than requested to fit the Control frame limit; follow its cursor. Titles are preserved, and a single unrepresentable row fails visibly. This does not establish live status. Journal entries contain no titles, working directories, prompts or transcripts. Invalid stored pagination cursors fail rather than restarting at the first page.

```sh
target/debug/agent-sessions session inspect \
  --endpoint codex-local --session '<thread-id>' --json
```

All communication examples may select a service explicitly with `--service-directory /absolute/owner-private/agent-communication`. Debug builds default to the debug Router root. They do not create a separate fake Codex home.

## Send information

Set `AGENT_SELF_ADDRESS` and `AGENT_TARGET_ADDRESS` to the exact supplied/discovered JSON references. Use a content file or stdin for messages containing shell metacharacters.

```sh
target/debug/agent-sessions message send \
  --from "$AGENT_SELF_ADDRESS" --to "$AGENT_TARGET_ADDRESS" \
  --text-file task-message.txt --json
```

Ordinary send uses `auto`: steer observed active work, submit to a loaded idle thread, or resume an existing stored thread then submit. It never creates a replacement for an unknown/lost identity. Resume is a real side effect and may make existing queued input eligible to run.

```sh
# Deferred input: target must be observed loaded before dispatch.
target/debug/agent-sessions message send \
  --from "$AGENT_SELF_ADDRESS" --to "$AGENT_TARGET_ADDRESS" \
  --delivery queue --text-file task-message.txt --json

# Exact active-turn steering; no fallback if that turn finishes.
target/debug/agent-sessions message send \
  --from "$AGENT_SELF_ADDRESS" --to "$AGENT_TARGET_ADDRESS" \
  --delivery steer --text-file task-message.txt --json

# Explicit human input; omit --from.
target/debug/agent-sessions message send --human-user \
  --to "$AGENT_TARGET_ADDRESS" --text-file human-instruction.txt --json
```

Input kind and delivery are independent. Agent input receives this declaration, rendered by the service:

```text
Agent communication
Self-declared sender: <full sender reference>
Intended recipient: <actual target reference>

<message body>
```

This declares origin; it does not authenticate it. Human input receives no agent declaration. The current native adapter carries the declaration as text; it does not claim Codex's internal inter-agent identity or authority semantics.

Queue admission is not a residency lease. If another native client unloads a target after the loaded check, an accepted item may remain pending. No compensating deletion or hidden resume follows an explicit queue operation.

## Interpret receipts and replies

`nativeInputAccepted` with `startedOrSteered` reports native turn/start acceptance without inventing which effect won a race. `steerAccepted` identifies exact accepted steering. `queueAccepted` contains the native queued-item submission ID. `clientUserMessageId` is correlation, not a deduplication guarantee.

Acceptance is not completion, and completion is not an exclusively attributable reply. B replies by explicitly sending another message to A's declared address. The service does not automatically route B's final answer back to A.

On an unknown outcome, do not automatically resend. Inspect the target or obtain further evidence. A failed auto send can still have successfully resumed the target; retain the error's independent resume/submission effects. CLI exit5 indicates an uncertain outcome. Exit3 indicates unavailable before dispatch; the command does not silently retry. Diagnostics expose fixed failure categories, not arbitrary backend error text or credentials.

## Observe or interrupt

```sh
target/debug/agent-sessions events listen \
  --endpoint codex-local --session '<thread-id>' --attach \
  --timeout-seconds 300
```

Executable Unix-carrier fixtures verify buffering, callback visibility, connection loss and refusal to report readiness after failed attachment or generation replacement. The real delivery harness also exercises readiness, queued execution, steering and exact interruption through this command. Attachment may load the target. Do not send on the assumption that a listener is ready until it emits `listenerReady` with target and generation. Bind subsequent send with both `--expected-service-epoch` and `--expected-generation` when using that receipt. Closing the listener or reaching its deadline does not interrupt the task, prove failure, or authorize resending.

```sh
target/debug/agent-sessions turn interrupt \
  --endpoint codex-local --session '<thread-id>' --turn '<exact-turn-id>' --json
```

Interruption is separate from message sending, queue deletion, session deletion and backend restart. It retains native exact-turn preconditions.

## ACP conversations

```sh
target/debug/agent-sessions conversation prompt --endpoint codex-local \
  --session '<thread-id>' --cwd /absolute/worktree \
  --text-file task-message.txt --json
```

Use `--new` instead of `--session` to create a new ACP session. Session creation uses the backend's model defaults; the Luna-only acceptance harness therefore loads a freshly created native thread whose model it has explicitly selected and verified before prompting. The CLI streams readiness, updates, permission-required and terminal prompt records. Its unattended permission handler returns cancellation; it does not approve tools. Ctrl+C cancels the current prompt on the same connection, and the prompt deadline defaults to300seconds. Cancellation settlement is bounded; lost transport remains an uncertain outcome and is never replayed.

Real debug permission tests exercised both a sole ACP responder and a competing native responder. Both paths correlated the permission callback with a native command completing as declined; neither executed the owned fixture. Sending a response does not prove which client won native callback resolution.

The Rust client requires successful session setup before prompting; a failed load does not leave the rejected target selected for work. Valid ACP error data is preserved, and malformed error responses leave the connection unusable.

For a client that implements ACP itself, use the raw stdio carrier:

```sh
target/debug/agent-sessions acp --endpoint codex-local
```

This bridge forwards JSONL between stdin/stdout and the published ACP socket. The caller supplies initialization, handles callbacks and decides permission responses. The bridge does not start another harness, inject prompts or replay requests. The reusable Rust `AcpTransportConnection` exposes the same carrier without terminal dependencies. Executable fixtures cover exact frame/identifier preservation and backend closure. The official ACP SDK has also completed load, prompt, streamed output and cancellation through this bridge against a fresh Luna thread on the debug Host.

## Agent sandbox access

Being able to execute the CLI does not imply permission to connect to its Unix socket. The debug proof established socket-connect denial in the ordinary read-only sandbox. A fresh-thread native permission profile extending read-only, with managed proxy enabled, credential brokering disabled, an empty domain allowlist and the exact canonical Control socket allowed, enabled agent CLI communication while denying unrelated live Unix/TCP connections.

This was a per-proof-thread configuration, not a change to global defaults or automatic permission injection. A skill explains the workflow; it does not grant access. Do not substitute broad network access or sandbox bypass. The test harness contains the validated invocation recipe and two-sided checks.

## Repeat the isolated proof

Build with a shared Cargo cache and one job. Use only existing debug routing/auth configuration and a free debug port. Never restart production to resolve a debug conflict.

```sh
cargo build -p codex-router-cli -p agent-sessions -p codex-router-host \
  --bin codex-router --bin agent-sessions --example debug_host_acceptance -j1

target/debug/examples/debug_host_acceptance \
  "$PWD/target/debug/codex-router" --agent-messages \
  "$PWD/target/debug/agent-sessions"
```

The harness uses port18787, a fresh private backend socket, the debug Router root/profile, normal Codex storage, fresh Luna-pinned threads and owned-child cleanup. It refuses an occupied debug port. It verifies backend routing and Remote Control disablement before model work, checks forbidden socket/TCP access, and requires both agents' successful CLI calls plus the checked result. Production identities must remain unchanged. Failure is not permission to retry, change models or widen sandbox access.

The independent ACP proof uses the pinned official SDK against the published ACP channel and verifies a Luna-owned thread before prompting. The replacement proof observes the old connection closing, explicitly reconnects and reloads the same persisted thread, then submits a distinct follow-up prompt. No request is replayed. A separate blank ACP session is created without prompting it. Native TUI persisted-thread recovery and continued Luna work have been demonstrated with the same native process after a bounded debug restart. Native attempt limits still apply to longer outages; Host readiness or successful cleanup alone is not a recovery receipt.

## Acceptance hook isolation

The acceptance harness disables ordinary hooks on fresh native proof threads. It also overrides the installed personal Stop-review runner only in the owned Host process tree so cold resumes cannot launch that classifier outside debug routing. It does not change home hooks/configuration or the parent environment. This was added after an earlier proof exposed a Stop hook that continued an old objective and used another provider endpoint; production process identity checks alone cannot establish routing isolation. Use the current harness for acceptance.

Additional bounded modes use the same debug setup and cleanup:

```sh
# Real event CLI, busy/idle queue, steer and exact interruption.
target/debug/examples/debug_host_acceptance \
  "$PWD/target/debug/codex-router" --delivery \
  "$PWD/target/debug/agent-sessions"

# Native status plus stored discovery through public address/journal readers.
target/debug/examples/debug_host_acceptance \
  "$PWD/target/debug/codex-router" --lifecycle-readers
```

Official ACP SDK modes accept the pinned SDK module path after --acp-client, --acp-permission, --acp-permission-race or --acp-replacement. They are explicit acceptance invocations; ordinary application clients use the published protocol.
