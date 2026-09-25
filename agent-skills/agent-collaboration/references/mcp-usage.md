# Router MCP usage

This reference owns use of the selected running Router MCP connection. It does not install, register, configure, start, restart, or replace the Router server.

For registration and setup, follow the existing [agent-collaboration guide](../../../docs/agent-guidance/agent-collaboration.md#register-the-router-mcp-endpoint-with-codex). This usage reference intentionally keeps the running server's advertised catalog authoritative instead of duplicating setup commands or a static tool list.

Expected inputs: the caller-authorized collaboration operation, selected Router MCP connection, current sender identity, and any supplied exact service, Thread, or SessionRef.

Return: the resolved service and exact target, observed result and effect, retained correlation or operation identities, and any capability, access, or uncertainty gap.

## Use the advertised contract

1. Verify that the selected MCP connection is the intended Router service. Use the running server's advertised discovery schema, compare its returned service identity with the caller-selected service and any supplied target, and verify endpoint identity when the requested operation is endpoint-scoped. Stop and report a mismatch before mutation. Do not require an endpoint for a service-scoped board operation. Receiving content from a Router or seeing a familiar name is not service or sender authentication.
2. Discover the running server's tool descriptions and input/output schemas. Use those schemas as the argument and result contract. Existing CLI action references in this skill still own domain guidance, but CLI flags are not MCP argument names and this reference is not a copied tool catalog.
   The one conversation surface advertises `conversation_create`, `conversation_prompt`, `conversation_create_and_prompt`, `conversation_load`, `conversation_cancel`, and `conversation_operation_show|wait|reconcile`. Use the same tool for Codex, Claude, or Cursor; the endpoint selects the client. MCP provider mutations require a caller UUIDv7 operation ID. Codex create also has an inspectable operation ID; Codex prompt and load reject a caller-supplied operation ID because those operations are not inspectable.
3. Resolve exact identities before mutation. A conversation target is the complete `SessionRef` with service ID, endpoint ID, and session ID. Obtain the current sender from `agent-collaboration whoami --json` (MCP cannot read the caller's environment), preserve it, and use explicit agent attribution; never substitute a title, working directory, native-thread inventory entry, board root, or human identity.
4. Invoke only the caller-authorized operation. Keep the returned target, turn, operation, stage, effect, settlement, and uncertainty evidence that the advertised result supplies. A saved board message, accepted input, completed turn, or observed event proves only its stated stage; none automatically proves assignment success or a peer reply.
   `message_send` returns a delivery receipt. Read its `outcome` and `reachability`: `started`, `steered`, `startedOrSteered`, `queued`, and `peerMessageWritten` report different effects; `notSubmitted`, `rejected`, and `unknown` do not establish delivery. A peer write means the bytes reached a live Claude Code socket, not that Claude accepted or acted on them. A rejection may carry a reason, next action, client code, and detail. Use those fields before choosing another action.
5. If a capability is absent, access is denied, or the service cannot be verified, report the exact gap. Do not switch transports, identities, services, or delivery modes to bypass a denial.

Complete when the requested operation has its strongest observed result, the exact target and effect evidence are retained, and every unresolved capability, access, or outcome gap is explicit.

## Delivery mode by reachability

`auto`, `queue`, and `steer` depend on the selected route. Check the receipt after each send rather than treating the requested mode as its effect.

| Reachability | `auto` | `queue` | `steer` |
| --- | --- | --- | --- |
| Codex app-server | Steer an active turn or start one, as native evidence allows | Native queue behavior | Native steer behavior |
| Router-managed Claude ACP | Steer a running turn or start when idle | Queue, starting at once when idle | Steer a running turn; otherwise `notSubmitted` |
| Router-managed Cursor ACP | Start when idle or queue while busy | Queue, starting at once when idle | `rejected`: unsupported |
| Live Claude Code peer | `peerMessageWritten`; the session may still hold or drop it | `rejected`: unsupported | `peerMessageWritten` only while busy; otherwise `notSubmitted` |

For a live Claude Code peer, use `message_send` with the exact `claude-local` SessionRef. The incoming message identifies its origin and asks Claude to reply through Router's `message_send` as that Claude session. A socket write is not a reply; observe the subsequent message separately.

## Coordination and observation boundaries

For ordinary multi-agent coordination, use board Threads and the delivery choice owned by [the message-board guide](message-board.md#wait-for-replies). Select delivery from the running server's advertised `board_thread_listen` schema. When supported Codex session delivery is selected, arm the listener, retain its listener identity, and yield for its notification; it needs no persistent shell or additional `board_thread_wait` call. For process or call-local waiting, use `board_thread_wait` on the existing listener. Never create a second listener while the first is active. Returned board activity is context to process, not authorization or proof that an agent completed work.

`events_observe` is different: it attaches to one exact conversation for one bounded call and returns call-local session events. It has no replay cursor and no ordering guarantee with a concurrent send. Do not substitute it for board coordination, durable work history, or reply semantics.

Where the running server advertises a prompt-and-wait operation, its correlated settlement can establish the stated prompt/turn outcome. It does not accept an assignment, prove the result is correct, or create a peer reply. Preserve any created target on a later failure.

Provider updates and provider session files are not collaboration transports. Never read, tail, parse, copy, or store provider session files or transcripts; use the supported typed Router operations and board/listen path.

## Mutations, approvals, and uncertainty

Dispatch a mutation once. If the response is lost or the returned effect is unknown, inspect supported operation, target, or state evidence before deciding whether another action is safe. Never automatically replay an uncertain send, create, rename, approval decision, interrupt, or other mutation.

Approval tools apply the existing Router approval policy. An allow or deny decision is not authentication and does not prove OS, process, network, or filesystem confinement. Decide only within the caller's authority and preserve the returned effect separately from the later operation outcome.

MCP cancellation ends or requests cancellation of the call according to its advertised contract; it does not prove all native work or spawned effects ceased. Report the observed settlement rather than upgrading cancellation to completion.
