# One SDK, two collaboration surfaces

This design implements [Specification R1–R9](specification.md) within [Requirements U1–U7](requirements.md).

## Composition

```text
CLI caller                          MCP/code-mode client
    |                                      |
    v                                      v
CLI argument/rendering adapter      MCP HTTP endpoint [new]
    |                                      |
    +-------------------+------------------+
                        v
              collaboration-client SDK
       typed operations + shared request preparation
                        |
        +---------------+----------------+
        |               |                |
   ControlClient    AcpConversation   NativeObservation
        |               |                |
        v               v                v
 Router Control    Router Codex ACP   Router native relay
        |               |                |
        +---------------+----------------+
                        v
                  Codex app-server
            conversation and execution owner

Board/automation SDK calls -> existing Router stores (unchanged)
```

The MCP endpoint uses Streamable HTTP on a dedicated loopback listener owned by the existing Host collaboration runtime. It calls the shared SDK over the same local Control/native connections used by CLI, rather than bypassing Router enforcement through internal calls. The endpoint is part of the running Router, not a client-launched stdio process or another daemon. Authentication, TLS and non-loopback exposure are deferred; the HTTP protocol is already suitable for a later remote deployment.

The existing model proxy listener is intentionally loopback-only (`codex-router-proxy/src/server.rs`, `LoopbackBindAddress`) and handles model traffic with its local token policy. The collaboration listener remains separate: reusing that model credential would invent authentication the owner declined, while exposing the model proxy would change an unrelated boundary. The Host owns MCP bind/start/shutdown alongside the existing collaboration listeners in `codex-router-host/src/collaboration_runtime.rs`.

## Current source and what changes

Source paths below are relative to the repository, inspected at `74aaa88`.

| Path | Existing responsibility | Target change |
| --- | --- | --- |
| `crates/collaboration-protocol/src/endpoint_identity.rs` | `EndpointRef`, `SessionRef`, opaque session ID | Reuse unchanged identity; clarify wording only. |
| `crates/collaboration-protocol/src/control_schema_document.rs` | Typed Control schemas | Reuse types and method catalog; do not invent a second domain schema for MCP. |
| `crates/collaboration-client/src/control_connection.rs` and operation modules | Typed Control client, validation and receipts | Remain authoritative SDK calls. |
| `crates/agent-collaboration/src/message_commands.rs` | Discovery/generation preparation mixed with CLI rendering | Move reusable preparation/error classification to SDK; leave flags and presentation in CLI. |
| `crates/agent-collaboration/src/session_target_arguments.rs` | Exact target or endpoint/session pair | Keep CLI parsing; share service/endpoint resolution with MCP in SDK. |
| `crates/agent-collaboration/src/conversation_commands.rs` | Create/load/fork then ACP prompt, caller from process environment | Share creation/prompt orchestration; separate create result; resolve caller explicitly for MCP. |
| `crates/collaboration-client/src/acp_conversation.rs` | Existing Codex ACP conversation client | Reuse creation and prompt logic; expose owned serializable operation inputs at SDK boundary. |
| `crates/collaboration-client/src/observation_session.rs` | Explicit attach and native event observation | Reuse for CLI stream and bounded CLI/MCP observation. |
| `crates/collaboration-service/src/acp_channel_listener.rs` | Inbound ACP translation to Codex | Preserve; this is not the external-provider launcher. |
| `crates/codex-router-host/src/collaboration_runtime.rs` | Starts and stops collaboration listeners | Own the new HTTP MCP listener; no predecessor MCP server exists here. |

## Shared operation definition

Use the existing protocol types where available. SDK-level convenience operations (creation, target resolution, bounded observation) receive owned serializable request/result types next to their existing SDK owner. One compile-time operation catalog binds public names, input/output schema generators, descriptions and typed SDK handlers. It is code, not a persistent registry or dynamic plugin framework.

CLI handlers and MCP tools call the same handler for each domain operation. Existing command vocabulary remains where its meaning fits. No parallel legacy implementation is retained after extraction. JSON result meaning and error classification come from the shared handler; CLI controls human formatting and exit codes; MCP controls protocol framing and tool errors.

Representative mapping:

```text
CLI                                 MCP tool                 Shared SDK owner
endpoints list                      endpoints_list           ControlClient
sessions list                       sessions_list            ControlClient
session create [new entry]          session_create           creation orchestration
session inspect/rename              session_inspect/rename   ControlClient
message send                        message_send             send preparation + ControlClient
turn interrupt                      turn_interrupt           ControlClient
events observe [bounded entry]      events_observe           NativeObservation
conversation prompt                 conversation_prompt      AcpConversation
board <operation>                   board_<operation>        board_operations
approval <operation>                approval_<operation>     ControlClient
automation/wake/schedule/etc.        corresponding operation  existing SDK modules
```

The finite operation catalog covers the command families listed in R1. The adapter does not advertise raw transport bridges, the TUI picker, or a shell execution tool. Existing streaming CLI observation remains a presentation of the same observation source; bounded observation is available to both clients.

## Request and return flow

```text
BEFORE: CLI flags -> CLI-owned preparation -> SDK -> Router -> Codex
AFTER:  CLI flags ------------------+
        MCP structured arguments --+-> shared SDK preparation -> Router -> Codex
                                   ^                               |
                                   +-- typed result/error ---------+

Added: MCP transport/dispatch and operation metadata.
Changed: request preparation moves from CLI to existing SDK owner.
Unchanged: Router enforcement, generation checks, provider IDs and state owners.
```

All calls across sockets are asynchronous. The SDK resolves the selected service directory, checks handshake identity, validates explicit targets, discovers endpoint capabilities, and dispatches once. The service continues validating requests independently. Wrong-service references fail before the mutation. Omitted endpoint resolves only with exactly one compatible available endpoint. There is no mesh forwarding or name-derived routing.

For creation, the SDK calls existing `AcpConversation::open_session`, returns the backend's session ID, and closes the setup connection without deleting the stored Codex thread. Later operations load/address that same thread. This detach/load behavior must be proven through the real backend before implementation is accepted. A create-and-prompt request keeps the connection for prompting and reports partial creation if prompting fails. It introduces no assignment object.

Caller identity is a request field or explicitly configured MCP connection context, not inherited blindly from the MCP process environment. CLI may continue resolving its current caller from its own environment. The shared operation receives an explicit resolved caller and approver. Existing self-declared attribution and approval authorization remain separate.

The shared prompt handler accepts the existing typed `MessageContent`, admitting only `Agent` or explicit `HumanUser` at this public boundary. It rejects internal `Router` content before dispatch. Unlike today's raw-text `AcpConversation::prompt`, the handler renders agent content with the same sender/recipient declaration used by message send, then passes the rendered text to the existing ACP prompt transport. Extract the existing renderer from `collaboration-service/src/agent_declaration.rs` into the shared message/protocol boundary so both service-side send and SDK-side prompt reuse it once. No client fabricates headers, no transport appends a second header, and creation's `createdBy` is never reused as a later sender. The unchanged upstream ACP translation still receives ordinary text, with explicit self-declared attribution rather than privileged instructions.

## State and failure ownership

```text
SDK call                         Durable authority
  validate                       Router service identity / existing configuration
  dispatch --------------------> Codex conversation or existing Router domain store
  return receipt <-------------- existing backend response
  connection lost
    before dispatch: unavailable
    after possible dispatch: outcomeUnknown
    never automatic replay
```

The SDK owns only call-local connections, cancellation tokens and bounded event buffers. Codex owns stored/loaded/active conversation state and exact turn execution. Router owns its existing board/automation stores and approval routes. No new persistence, session aliases, task leases or transcript history is introduced.

Overlapping clients remain subject to current backend queue/steer/busy rules and generation guards. MCP does not add a queue to change those rules. An interrupt of an old turn cannot be retargeted to a newer turn. Known turn/target evidence survives failures.

Observation uses one `NativeObservation` connection per active observation call. A bounded result includes target, generation, attachment evidence, collected events, end reason and a continuation-gap warning when applicable. It stops at the earlier of its deadline, caller cancellation, backend disconnect, or result limit. The buffer has explicit event/byte limits validated against transport budgets; no silent truncation is allowed. No subscription ID is persisted. A later call is a new observation, not replay from an implied cursor.

Readiness in the bounded result describes attachment that already happened; it cannot order a concurrent send. Clients needing correlated completion use `AcpConversation` prompt-and-wait, which owns prompt submission and response observation on one connection. The streaming CLI may retain its existing early `listenerReady` output. The bounded CLI/MCP operation makes the same weaker observation guarantee on both surfaces.

MCP request cancellation closes observation only. Prompt-and-wait cancellation requests backend cancellation through its existing cancellation token while the connection permits; it does not guarantee cessation. Application deadlines can return settlement evidence through a normal response. MCP `notifications/cancelled` can suppress that response, and disconnection can prevent delivery entirely: the adapter must not promise a post-cancellation receipt. Send/create cancellation after dispatch cannot be assumed to reverse an effect or leave a caller-visible identity. Inspection of existing state is the recovery path when sufficient identity is known, with no automatic replay and no new receipt store. Exact interruption is a distinct operation. This follows the maintained MCP SDK's [cancelled-response test](https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/tests/test_cancelled_response.rs).

## MCP schema and transport boundary

Use the official Rust MCP SDK, `rmcp`, for initialization, Streamable HTTP framing, tool discovery and dispatch. Select and pin a released version compatible with the workspace toolchain and schema crates during dependency validation; inspected upstream `main` is API evidence, not a release pin. The source is [the Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk), specifically `ServerHandler`, `StreamableHttpService`, tool input/output schemas and structured tool results. The HTTP transport is protocol machinery, not a replacement for the existing collaboration SDK.

Use `rmcp` tool-routing/schema helpers where they fit the shared operation catalog; do not implement a second JSON-RPC dispatcher, SSE parser, MCP handshake, or session manager. Adapt its Tower HTTP service to the repository's Tokio/Hyper stack using standard integration helpers. Introduce another web framework only if the integration actually needs it, not as a parallel HTTP stack. Retain library Host/Origin validation and explicitly configure Origin enforcement: inspected upstream defaults do not enable it for an empty allowlist.

Follow the repository's pinned toolchain, workspace dependency/lint configuration, and `/Users/shravansunder/dev/devfiles/shared/language-rules/rust-rules.md`: one Host-owned Tokio runtime; typed payload enums/newtypes; Serde/Schemars for owned contracts; `thiserror` domain errors; tracked shutdown; explicit timeout units; and responsibility-bearing names such as `mcp_http_listener` and `collaboration_tool_dispatch`. Keep arbitrary JSON only at genuinely upstream/unknown envelopes. No production unwrap/panic, nested runtime, generic `utils` dumping ground, or handwritten substitutes for standard MCP helpers. Dependency compatibility and protocol-version selection require compile and real HTTP conformance evidence; do not guess an unreleased SDK API.

The HTTP adapter belongs to a dedicated collaboration MCP module/package consumed by the Host. It depends on `collaboration-client` and MCP transport types; the client never depends on HTTP server or Host internals. Host configuration supplies a loopback bind address, port, and explicit permitted origins; bind validation rejects all non-loopback addresses. The listener publishes its actual local URL through local service discovery. Bind failure is reported explicitly; there is no fallback to stdio or an external interface. The model proxy's token/configuration is not reused.

The existing foreground Host CLI owns `--mcp-bind HOST:PORT`, passed through `HostConfig` to `CollaborationRuntimeInputs`. Default to `127.0.0.1:8788` for normal runs and `127.0.0.1:18788` for debug runs; do not derive it arithmetically from the provider port. Preserve the override on Host re-execution. Port zero is an explicit test configuration, never an implicit fallback.

Bind the HTTP listener before advertising it. Extend the existing service manifest with a typed `mcp` selector (`transport: streamableHttp`, actual loopback HTTP `/mcp` URL) and advance its strict version from 1 to 2. Update in-repository readers, writers and tests together, without a version-1 compatibility branch or a second discovery file. Version-skew rejection must identify the mismatch clearly. New clients and Host must be used together; installing a binary does not authorize replacing a running production Host. Debug acceptance uses matching built clients/Host and an isolated service directory.

The `/mcp` route delegates POST, GET/SSE and DELETE handling to the supported Streamable HTTP implementation, including protocol-permitted unsupported-method responses. Validate supplied Origin against configured local origins before dispatch; absent Origin is accepted for local tools. Use only in-memory MCP connection/session state needed for initialization, in-flight calls and cancellation. MCP transport identifiers stay separate from `SessionRef` and are never persisted as conversation mappings. HTTP session teardown does not delete Codex threads. Reconnect creates transport context and reuses the caller's existing `SessionRef`; no application retry or durable event replay is added. Host shutdown closes the listener and transport resources; ambiguous in-flight mutations retain the existing unknown-outcome semantics.

Tool schemas are generated from the same domain request/results used by CLI JSON. Schema export must resolve references for MCP clients rather than publish unusable private Control-schema URIs. For upstream native payload fields, use the service's advertised native schema when available; unavailable capability-dependent tools fail explicitly or are omitted consistently from capability discovery. Do not claim stronger typing than the advertised schema provides.

MCP protocol errors handle malformed/unknown calls. Domain failures are structured tool results with `isError`, retaining the same kind, stage, target and effect evidence as CLI JSON. Text content is a readable projection, not the source of machine meaning. HTTP responses carry MCP frames; diagnostics never enter protocol payloads.

Local HTTP makes the MCP endpoint reachable to processes on the computer, rather than granting access through filesystem socket ownership alone. This is the explicit V1 no-auth deployment boundary, not an authenticated-user guarantee. Harness permissions, approver checks and self-declared sender semantics remain unchanged. Origin validation does not authenticate local clients. A later remote deployment must define authentication/TLS before relaxing the loopback restriction; this design adds neither a placeholder credential scheme nor an identity service.

## Why this amount of structure

A shell wrapper around CLI would duplicate parsing and require clients to interpret presentation. Separate MCP business logic would drift. Shared SDK preparation plus a thin MCP adapter supplies the requested parity while reusing enforcement and storage. The cost is extracting CLI-embedded behavior and maintaining an operation catalog; parity checks make that cost visible when adding an operation.

A new Router task service, provider supervisor or mesh directory is unnecessary for these obligations and is excluded. External ACP adapters will be a separate design; their mapping needs cannot justify adding Codex persistence now.

## Proof boundaries

```text
CLI process ----+
                +-> real shared SDK -> debug Router -> real Codex app-server
HTTP MCP client+       |                    |                   |
                       +-- schemas          +-- receipts         +-- thread/turn effects

Fault-only tests may replace socket peers to induce disconnection.
They do not replace the real-path proof above.
```

| Specification | Realization | Proof seam |
| --- | --- | --- |
| R1–R2 | Shared operation catalog, SDK handlers and two adapters | V1 checks catalog coverage, schema resolution, normalized request/result parity and actual MCP discovery/calls. |
| R3 | SDK target resolution plus service validation | V2 observes rejected mismatch/ambiguity before backend effects and full target in successful results. |
| R4–R5 | Existing creation adapter, explicit caller preparation, reused message renderer and approval broker | V3 observes real creation/detach/resume, effective settings, recipient-visible attribution for initial and different-sender resumed prompts, internal-variant rejection and approval routing through both clients. |
| R6 | Existing send/interrupt client methods | V4 correlates acceptance and exact-turn effects, including concurrent/stale calls. |
| R7 | Existing observation/prompt connections plus bounded adapter | V5 exercises unordered concurrent observation, application deadline results, MCP cancellation cleanup without late response, loss, bounds and partial-effect reporting. |
| R8 | Unchanged storage/provider ownership | V6 inspects state writes/dependencies and confirms no new durable owner. |
| R9 | Host-owned loopback HTTP listener, MCP transport and Origin validation | V7 uses a real HTTP client and listener; validates bind rejection, HTTP initialization/discovery/calls without credentials, invalid Origin before effects, and reconnect with the same conversation reference. |

Schema checks alone cannot prove execution or cancellation. A successful turn does not prove an assignment, test suite or review complete. The real-path proof must verify the operation effects actually claimed.
