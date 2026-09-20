# Equivalent CLI and MCP collaboration

This specification realizes [Requirements U1–U7](requirements.md). CLI and MCP are two presentations of the same collaboration operations.

```text
Human / shell agent                 MCP / code-mode agent
       |                                    |
       | CLI flags or JSON                  | Streamable HTTP on loopback
       +----------------+-------------------+
                        |
                Collaboration system
                        |
             Codex app-server conversation
                 one thread/session ID

Both callers also use the existing board and automation capabilities.
The system returns results, errors and observations, not task verdicts.
```

## R1 — Equivalent operations (U1, U5)

Every finite collaboration operation exposed in the supported CLI catalog MUST have an MCP counterpart with equivalent inputs, effects, result meaning and domain errors. Human formatting and transport framing may differ. The scope is the existing endpoint, session, conversation, message, approval, board, instruction, wake, schedule, run, delivery, operation and journal capabilities, plus the observation contract below. Their existing policy remains authoritative.

Interactive session picking, terminal layout, shell file/stdin convenience, and raw `native`/`acp` byte bridges are presentation or transport facilities, not separate domain operations requiring tools. No new command family called `task` is introduced.

Tool descriptions MUST explain side effects, prerequisites, and the difference between accepted input, a completed turn, and an agent reply. Help and machine-readable discovery MUST expose the same supported operation inventory. Unsupported service capabilities produce explicit errors; tools never invent a successful fallback.

## R2 — Typed programmatic use (U1, U2)

MCP MUST publish an input schema and output schema for each tool and return structured results. CLI JSON output MUST carry the equivalent domain result. Callers MUST NOT need to parse prose, terminal tables, shell snippets or JSON encoded inside string fields to obtain a target or result.

Tools are ordinary named collaboration operations, not a generic shell command or execute-code tool. Code-mode clients can generate code that calls these tools; Router supplies no code-mode execution engine.

## R3 — Resolve the existing address (U3)

The authoritative conversation address remains:

```json
{"endpoint":{"serviceId":"<service UUID>","endpointId":"codex-local"},"sessionId":"<Codex thread ID>"}
```

Operations that create a conversation require an endpoint target; operations on an existing conversation require its session target. Service-scoped board/automation operations do not acquire a meaningless endpoint field.

Both clients MAY resolve an omitted service ID from their explicitly selected local service connection. The full resolved address MUST be returned. An explicit service ID that differs from the handshake MUST be rejected before mutation. Endpoint selection MUST be explicit or resolve to exactly one compatible available endpoint; ambiguous selection is an error with candidates. No provider is selected by guessing from a model name.

The supplied service directory chooses the local service. Remote addresses are not supported merely because a reference includes a service ID. Unsupported remote routing fails clearly; no local substitution occurs.

## R4 — Start and continue the same conversation (U2, U3, U4)

Creation MUST accept an existing absolute `cwd`, model, effort and the existing harness access selection, with caller/approver information required by the existing creation contract. It returns the actual `SessionRef` and observed settings when available. Unsupported or mismatched settings remain errors; they are not silently substituted.

Creation and submitting a message MUST be independently available so an agent can obtain and name a conversation before sending work. An existing create-and-prompt CLI convenience may compose those same operations; it must preserve the created address if the subsequent prompt fails. Resume/load keeps the same conversation ID. Explicit existing fork capability creates another conversation; it does not create a Router-owned version of the source.

Creation does not create a Git worktree, apply patches, install tools, or decide assignment completion. Existing board association is optional and retains its existing meaning.

## R5 — Caller and approval semantics (U2, U4, U5)

Agent messages MUST retain their self-declared sender address. Human input remains an explicit separate choice. MCP MUST NOT attribute every caller to the server process's creator session. A configured connection identity or explicit caller input supplies attribution; neither is proof of authenticated identity.

This applies to both message submission and prompt-and-wait, including an initial prompt after creation. Each prompt identifies its current sender; creation metadata is not a substitute when another agent later resumes the conversation. Public prompt inputs admit agent or explicit human content, not the internal Router-originated content variant.

Existing approver routing and harness access policy MUST remain enforced. Missing required identity fails before creation/submission. MCP does not auto-approve permissions or broaden access. CLI and MCP must expose the existing approval operations with the same authorization checks.

## R6 — Message and interruption behavior (U1, U2, U5)

Send preserves existing `auto`, `steer`, and `queue` semantics, including their loaded/active preconditions. No fallback delivery mode is silently selected. Results preserve actual acceptance evidence and turn/submission identifiers when provided.

Interruption targets the exact existing session and turn with the existing generation guard. A turn ID identifies one execution within a conversation; it is not another conversation identity. An interrupt receipt does not assert that an agent's assignment succeeded or that cessation was observed when the backend has not established it. No automatic replacement operation is added.

## R7 — Observation and uncertain effects (U1, U2, U5)

Both surfaces MUST offer bounded observation of a selected conversation, with explicit attachment because observing may resume/load the conversation. Result data distinguishes attachment readiness, observed events, timeout, overflow and backend disconnection. Events remain observations, not durable replay guarantees.

Cancelling observation closes observation only; it MUST NOT interrupt the running turn. Cancelling a prompt-and-wait operation follows the existing prompt cancellation behavior, stated in its tool description. A transport loss after mutation may mean `outcomeUnknown`; neither surface automatically repeats that mutation. Results that can still be delivered MUST retain available target and turn identifiers after partial failure.

An application deadline is distinct from cancelling the MCP request. A deadline may return structured timeout/settlement evidence while the client connection remains usable. MCP cancellation or disconnection does not guarantee a final response, successful backend cancellation, or delivery of the created session ID. Clients retain any previously received identity and inspect existing state where possible; absence of a response does not authorize automatic replay. No durable cancellation-receipt store is required.

A bounded observation call returns events collected during that call, ends at its deadline or a configured result-size bound, and reports whether the stream ended or was cut short when a response can be delivered. It creates no durable cursor/session registry. Concurrent observe/send calls do not guarantee attachment before submission. Completion-sensitive callers use prompt-and-wait for correlated execution evidence. Existing streaming CLI observation can expose early readiness, but that presentation feature is not a guarantee of the bounded MCP call. Late observation does not guarantee earlier events.

## R8 — Existing state ownership and extension boundary (U3, U4, U6)

No new durable task/session mapping, transcript store or lifecycle database is introduced for Codex. Existing Router board, automation, identity, approval and observation state retains its current ownership. External ACP providers may later supply equivalent operations only where their actual capabilities support them. That extension does not change the public conversation address or require an ACPX dependency.

## R9 — HTTP foundation, local deployment (U7)

MCP MUST use Streamable HTTP at a documented `/mcp` endpoint, not stdio. The configured listener MUST reject non-loopback addresses, including wildcard binds, before accepting requests. Local plain HTTP is supported. V1 requires no bearer credential, OAuth flow, login, or TLS configuration. Non-loopback exposure and remote-client authentication are not supported in this release.

The normal default URL is `http://127.0.0.1:8788/mcp`; the debug default is `http://127.0.0.1:18788/mcp`. An explicit loopback bind override selects another address/port. Restart preserves the configured URL. An occupied port fails visibly; no automatic fallback silently changes the client URL. Explicit port zero is allowed for isolated tests, with the actual bound URL published.

The server MUST negotiate MCP initialization and protocol version and expose tool discovery and calls through that endpoint. HTTP method, content negotiation and cancellation handling follow the selected supported MCP protocol. Invalid supplied Origin headers MUST be rejected before tool execution; this is HTTP protocol boundary validation, not user authentication. Requests without Origin may be used by local non-browser clients.

An MCP transport session, if used by the protocol implementation, MUST NOT become a new Codex session ID, authenticated identity, or durable conversation store. Reinitializing a lost MCP connection does not replace the addressed Codex conversation. No mutation is replayed automatically by Router after reconnect, and no cross-restart event replay or load-balancer failover is promised.

## Reader journey and failure example

```text
Discover endpoint -> create(cwd, settings, caller) -> SessionRef
                                                     |
                          +--------------------------+------------+
                          |                                       |
                     observe(target)                         send(target)
                          |                                       |
                     readiness/events                       acceptance
                          +------------------+--------------------+
                                             |
                              inspect or interrupt exact turn

Send loses connection after dispatch
  -> outcomeUnknown + known target/turn evidence
  -> inspect existing conversation; do not resend automatically
```

## Proof obligations

| Proof | Requirements | Evidence that distinguishes pass from fail |
| --- | --- | --- |
| V1 | R1–R2 | Catalog/schema checks and paired CLI/MCP behavioral cases demonstrate equal domain inputs/results/errors and no missing supported operations. |
| V2 | R3 | Wrong-service and ambiguous-endpoint cases cause no mutation; local resolution returns the actual full address. |
| V3 | R4–R5 | Real debug Router creation through each surface verifies cwd/settings, same-ID resume, recipient-visible agent attribution on initial prompt and resume by a different sender, rejection of internal Router content, and preserved permission behavior. |
| V4 | R6 | Real message acceptance, busy/precondition errors, and exact-turn interruption through both surfaces; no invented completion verdict. |
| V5 | R7 | Observation timeout/cancellation, concurrent observe/send without ordering guarantees, late attach, bounded output, backend loss and uncertain submission retain the specified distinctions. MCP request cancellation is verified through cleanup/effects without expecting a late response; application deadlines are verified separately. |
| V6 | R8 | Source/state inspection verifies no additional Codex registry or transcript persistence and no external-provider execution in the foundation. |
| V7 | R9 | Actual HTTP MCP initialization, discovery and calls on loopback; non-loopback/wildcard bind rejection; invalid Origin rejection before effects; no authentication requirement; reconnect retains the Codex address without automatic mutation replay. |

Proof uses the debug Router and bounded disposable conversations. Production Router replacement is not authorized. Test doubles may exercise faults but cannot substitute for the real CLI/MCP-to-Router-to-Codex proof.
