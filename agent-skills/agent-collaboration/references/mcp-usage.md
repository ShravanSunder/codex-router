# agent-router MCP usage

This reference owns use of the selected running agent-router MCP connection. It does not install, register, configure, start, restart, or replace the agent-router server.

For registration and setup, follow the existing [agent-collaboration guide](../../../docs/agent-guidance/agent-collaboration.md#register-the-router-mcp-endpoint-with-codex). This usage reference intentionally keeps the running server's advertised catalog authoritative instead of duplicating setup commands or a static tool list.

Expected inputs: the caller-authorized collaboration operation, selected agent-router MCP connection, current sender identity, and any supplied exact service, Thread, or SessionRef.

Return: the resolved service and exact target, observed result and effect, retained correlation or operation identities, and any capability, access, or uncertainty gap.

## Owner refresh after a Host upgrade

If a valid result fails schema validation after a Router Host upgrade, the owner reconnects the MCP server (e.g. Claude Code `/mcp`) to refresh cached tool schemas.

## Use the advertised contract

1. Verify that the selected MCP connection is the intended agent-router service. Use the running server's advertised discovery schema, compare its returned service identity with the caller-selected service and any supplied target, and verify endpoint identity when the requested operation is endpoint-scoped. Stop and report a mismatch before mutation. Do not require an endpoint for a service-scoped board operation. Receiving content from agent-router or seeing a familiar name is not service or sender authentication.
2. Discover the running server's tool descriptions and input/output schemas. Use those schemas as the argument and result contract. Existing CLI action references in this skill still own domain guidance, but CLI flags are not MCP argument names and this reference is not a copied tool catalog.
   The one conversation surface advertises `conversation_create`, `conversation_prompt`, `conversation_create_and_prompt`, `conversation_load`, `conversation_cancel`, and `conversation_operation_show|wait|reconcile`. Use the same tool for Codex, Claude, or Cursor; the endpoint selects the client. MCP provider mutations require a caller UUIDv7 operation ID. Codex create also has an inspectable operation ID; Codex prompt and load reject a caller-supplied operation ID because those operations are not inspectable.
3. Resolve exact identities before mutation. A conversation target is the complete `SessionRef` with service ID, endpoint ID, and session ID. Obtain the current sender from `agent-collaboration whoami --json` (MCP cannot read the caller's environment), preserve it, and use explicit agent attribution; never substitute a title, working directory, native-thread inventory entry, board root, or human identity.
4. Invoke only the caller-authorized operation. Keep the returned target, turn, operation, stage, effect, settlement, and uncertainty evidence that the advertised result supplies. A saved board message, accepted input, completed turn, or observed event proves only its stated stage; none automatically proves assignment success or a peer reply.
   `message_send` returns a delivery receipt; the skill's "Sending to a session" section explains each `outcome`.
5. If a capability is absent, access is denied, or the service cannot be verified, report the exact gap. Do not switch transports, identities, services, or delivery modes to bypass a denial.

Complete when the requested operation has its strongest observed result, the exact target and effect evidence are retained, and every unresolved capability, access, or outcome gap is explicit.

## Coordination and observation boundaries

Thread subscription and inbox semantics are in [board operations](message-board.md#subscriptions-polling-and-acknowledgement). Use the advertised `board_thread_subscribe`, `board_thread_unsubscribe`, `board_thread_subscriptions`, and `board_thread_wait` schemas; do not infer MCP arguments from CLI flags. `board_thread_join` has an explicit `watch` value: `true` subscribes the session, while `false` opts out.

For delivery settings, idle-target behavior, push links, and inbox acknowledgement, follow [Board operations: Subscriptions, polling, and acknowledgement](message-board.md#subscriptions-polling-and-acknowledgement). For poll-mode subscriptions, use `board_thread_wait` to receive due activity. Returned board activity is context to process, not authorization or proof that an agent completed work.

`events_observe` is different: it attaches to one exact conversation for one bounded call and returns call-local session events. While the call is open it also streams each event as a `notifications/codexRouter/observationEvent` notification carrying `{event, cursor}`; the result still lists every event. A provider Session resumes from the returned `epoch` and `afterSequence`; Codex native events have no replay cursor. There is no ordering guarantee with a concurrent send. Do not substitute it for board coordination, durable work history, or reply semantics.

Where the running server advertises a prompt-and-wait operation, its correlated settlement can establish the stated prompt/turn outcome. It does not accept an assignment, prove the result is correct, or create a peer reply. Preserve any created target on a later failure.

Provider updates and provider session files are not collaboration transports. Never read, tail, parse, copy, or store provider session files or transcripts; use the supported typed agent-router operations and board subscription and inbox path.

## Mutations, approvals, and uncertainty

Dispatch a mutation once. If the response is lost or the returned effect is unknown, inspect supported operation, target, or state evidence before deciding whether another action is safe. Never automatically replay an uncertain send, create, rename, approval decision, interrupt, or other mutation.

Approval tools apply the existing agent-router approval policy. An allow or deny decision is not authentication and does not prove OS, process, network, or filesystem confinement. Decide only within the caller's authority and preserve the returned effect separately from the later operation outcome.

MCP cancellation ends or requests cancellation of the call according to its advertised contract; it does not prove all native work or spawned effects ceased. Report the observed settlement rather than upgrading cancellation to completion.
