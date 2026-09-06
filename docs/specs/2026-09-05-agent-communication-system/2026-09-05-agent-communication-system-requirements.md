# Agent communication system — Requirements

## The outcome

A human, application, or agent can identify an agent endpoint, address one of its sessions, submit information, and observe the resulting activity through documented clients. Starting on one machine must not collapse host, endpoint, session, execution, and transport into one identity. Later access over Tailscale must preserve those distinctions.

Codex Router provides the initial integration. Codex app-server remains one runtime type, not the definition of every agent or host. ACP supplies portable conversations where the destination supports the negotiated operations. The complete native Codex protocol remains available to Codex-aware consumers.

The [Specification](./2026-09-05-agent-communication-system-specification.md) records observable domain boundaries and the remaining contract decisions. The [Program Design](./2026-09-05-agent-communication-system-program-design.md) records structural ownership.

## Consumers and journeys

**An agent with CLI access:** discover a target → establish observation where supported → explicitly send information → distinguish acceptance from completion → optionally send another explicit message back. Skill guidance uses the same public commands as a human; it cannot supply transport access or execution authority by itself. (U12–U14, U27, U28, U30)

**An application developer:** select an endpoint → establish an authenticated connection when required → negotiate actual capabilities → call through a Rust, Swift, TypeScript, or Python client → receive typed results, events and errors without depending on terminal UI. (U6–U8, U19–U21, U32)

**A terminal user:** discover/search existing Codex threads → choose new/resume/fork → launch the native TUI. Native TUI recovery remains upstream-owned. This project does not add an always-running Sessions supervisor. (U1, U3–U5, U17, U24)

**An operator:** expose the intended local service → inspect endpoint/backend availability → activate a replacement through the existing lifecycle owner → observe honest availability and connection loss. Production replacement is separately authorized. (U9, U10, U18, U23)

**A future remote caller:** select a host and endpoint → discover permitted session metadata → communicate with a session owned there. Contact does not imply session relocation, history replication, or local execution. Remote delivery timing remains a scope choice. (U29, U31, U34)

## Needs retained or revised

The U identities retain their relationship to the [prior requirements](../2026-09-04-agent-communication-foundation/2026-09-04-agent-communication-foundation-requirements.md). The current owner explicitly removes the supervisor, requires language-independent clients and well-defined domains, and permits a local-first delivery. Those decisions govern this replacement. Existing Must priorities remain for retained needs; new needs are owner-requested without a ranked delivery order.

| ID | Current need or boundary | Authority and applicability |
| --- | --- | --- |
| U1 | Preserve native interactive recovery when the backend returns; do not introduce a competing recovery owner. | Revised by native-recovery and no-supervisor decisions. Runtime limits require proof. |
| U2 | Deliberate CLI exit/cancellation must not secretly restart work. Managed-child restart/release machinery is removed. | Prior terminal-intent need retained; supervisor-specific behavior superseded by owner. |
| U3 | Preserve Sessions discovery formats, filters, sorting, limits and paging. | Retained. |
| U4 | Preserve exact/latest/new/resume/fork, local/hosted selection, dry-run and native argument fidelity. | Retained; final executable spelling remains a packaging decision. |
| U5 | Preserve search, previews and interactive picker behavior. | Retained. |
| U6 | Discover service endpoints and select the intended protocol unambiguously. | Retained; identity must allow multiple hosts/endpoints. |
| U7 | Support the negotiated ACP contract without pretending all optional capabilities exist. | Retained; ACPX excluded by owner. |
| U8 | Preserve complete native Codex operations, events and callbacks. | Retained. |
| U9 | Preserve explicit native connection loss/reconnect and never silently replay uncertain operations. | Retained. Native TUI may perform its own reconnect. |
| U10 | Inspect backend availability, endpoint capabilities and session inventories through the communication surface. | Retained coordination outcome; managed supervisor registry removed by owner. |
| U11 | Distinguish persisted session metadata, live runtime state and client connection state. | Retained; no mandatory managed-terminal inventory. |
| U12 | Explicitly send agent communication or human-user input to an exact target. Input kind and delivery are independent: agent communication and state-aware immediate delivery are the defaults. Ordinary send steers active work, starts idle loaded work, or resumes an existing stored thread before submission. Explicit queue requires a loaded thread; explicit steer requires an active turn. Human-user input is explicit. | Owner-confirmed SDK/CLI correction. Mapping to heterogeneous runtimes requires capability-specific contracts. |
| U13 | Distinguish queued/submitted input, completed execution, and a peer reply; report correlation limits and a native submission ID when available without inventing backend acknowledgement. | Retained and clarified by owner. |
| U14 | Interrupt exact work without conflating interruption with client closure or session deletion. | Retained where the destination supports an exact operation. |
| U15 | Preserve native Codex queue behavior; do not replace its queue. | Retained. Durable offline delivery is deferred to a fast follow. |
| U16 | Do not add automatic managed blank-identity replacement. Native blank recovery limits must remain visible. | Supervisor-owned replacement superseded by explicit no-supervisor/cut decisions. |
| U17 | Preserve native fork identity and behavior; do not substitute the source thread identity. | Retained; external managed TUI tracking removed. |
| U18 | Expose actual backend lifecycle/availability across service reconnection. | Retained outcome; managed-client intent persistence is no longer required. |
| U19 | Preserve each protocol's actual envelopes, framing, identifiers and version rules. | Retained. Native Codex is not forced into another JSON-RPC dialect. |
| U20 | Publish separate versioned protocol schemas and capability information. | Retained. |
| U21 | Supply explicit operation/result/error types usable by independent clients. | Retained. |
| U22 | Keep communication metadata separate from provider-routing state and native history/queues. | Retained boundary; it does not require a new database. |
| U23 | Restrict local access to the intended owner; remote access requires explicit identity/access rules. | Local boundary retained; owner permits remote design, not implicit network exposure. |
| U24 | Separate Sessions product, reusable clients, native integration and provider-router responsibilities. | Retained; no always-running Sessions supervisor. |
| U25 | Use two- or three-word responsibility names for important new/moved files and folders, with explicit package boundaries. | Retained and reinforced by current owner. |
| U26 | Make requirements, observable specification and structural design distinct and understandable. | Retained. |
| U27 | Allow one agent to explicitly send information to another independently addressed session. | Retained owner-requested outcome. |
| U28 | Distinguish content-bearing messages from notifications received by client software. | Retained; V1 notifications do not automatically start model turns. |
| U29 | Leave explicit online relocation to later work; no continuous replication required initially. | Retained owner boundary. |
| U30 | Provide descriptive CLI operations and agent skill guidance. | Retained. Raw RPC is not the ordinary agent workflow. |
| U31 | Define host, endpoint, session, execution and connection identities independently of initial local placement. | Current owner: correct abstractions and domains matter even when starting local. |
| U32 | Make usable clients available to Rust, Swift, TypeScript and Python consumers. | Current owner. Generated types alone are insufficient; Rust SDK and CLI first; other SDK implementations follow. |
| U33 | Keep domain contracts independent of UI, language bindings, runtime adapters and process ownership. | Current owner requests core package segregation; structural realization follows separately. |
| U34 | Enable later communication between separately owned hosts and heterogeneous runtimes without sharing one app-server. | Current owner. Local-first is acceptable; first-release remote delivery is not mandatory yet. |

## Address book and observation history

U35: Provide an endpoint-scoped address book of discovered Codex threads and an append-only journal at thread/server lifecycle and status granularity: observed thread creation/discovery, status changes, observed interruption/completion or unload, backend readiness/loss/replacement, and observer coverage loss/restoration. The current owner requests this domain explicitly. Journal entries describe what this service observed, not an authoritative global history of Codex. Reconciliation with native inventory remains necessary after disconnection or restart. No message bodies, transcripts, credentials, token deltas, individual tool activity or tool output enter this journal. A stopped/interrupted turn is not a stopped thread process; idle, unloaded, archived and deleted remain distinct native facts. Discovery of an existing thread is not logged as its creation. Server status means the Host-observed backend lifecycle or a client-observed connection condition, with the evidence source retained; a disconnected client cannot assert that the server died.

The address book distinguishes remembered addresses, current observation coverage and live runtime availability. Missing from one listing does not mean deleted. A disconnected observer cannot certify current state. Local append order is not a global runtime-event order or a distributed replication protocol. Lifecycle records use rolling 30-day retention. Expired history is removed only after preserving the address-book checkpoint; explicit reset/export commands are not part of V1.

## Scope and protected boundaries

The implementation home remains this Codex Router worktree. Other systems are future consumers or evidence; their product code is outside this design's implementation authority. Preserve provider routing, account/quota behavior, Codex-native execution and permissions, and normal Codex home for real session discovery. No upstream changes or production-process experiments are authorized.

The system does not implement a manager, worker selection, job engine, terminal supervisor, PTY proxy, client crash-restarter, or new agent loop. A built-in scheduler and durable mailbox are deferred fast-follow capabilities. V1 exposes callable operations for external schedulers and does not accept offline-delivery responsibility. It does not implement session relocation, continuous session replication, or a shared transcript. Hermes deployment, Tool Portal installation and Agent Studio UI implementation remain separate integrations.

Local-first does not mean one global session namespace, one protocol for all runtime operations, or an assumption that the session owner and tool execution machine are the same. Conversely, future readiness is not authority to implement a generic backend framework, gossip membership, a cloud registry or remote execution substrate without a concrete consumer contract.

## V1 delivery boundary

V1 runs on one machine. A local Host process composes communication and existing lifecycle responsibilities; the public SDK/CLI does not depend on a Sessions supervisor. Product renaming, remote deployment, scheduler and durable mailbox are deferred. An unavailable destination rejects submission before dispatch; loss after possible dispatch reports an unknown outcome without automatic replay.

Explicit sends submit through the selected native delivery operation; queued acceptance does not promise immediate execution. Notifications reach listening client software; the service does not automatically convert them into model turns. The accepted initial explicit-send/listen workflow is the basis for later wake-policy integrations.

The client target is Rust, Swift, TypeScript and Python, each exercising the same communication contract. V1 ships the Rust SDK and descriptive CLI. Swift, TypeScript and Python SDK implementations are later deliveries against the same language-independent protocol. Any authorized agent can use the CLI without a language-specific SDK. V1 backend coverage is the existing Host-managed Codex app-server, exposed through native access, ACP adaptation and Session Control. Hosting or connecting to non-Codex ACP runtimes is deferred; another harness may be a client of our Codex interfaces. No existing Hermes deployment is modified. ACP interoperability requires a real independent ACP client talking to our Codex adapter; it does not authorize hosting that client's agent runtime.

The service must not infer an agent's self-address. A caller supplies its endpoint/session reference when directing a return message. Automatic harness-specific self-address injection remains a later integration.

Scheduler and mailbox follow-up work use the public clients. They do not change native thread ownership. Scheduler design must settle missed runs and overlapping executions; mailbox design must settle durable acceptance, deduplication, retention, expiry and failure reconciliation before either capability is implemented.

## Agent declaration and client consistency

U36: SDK and CLI expose the same input-kind and delivery choices. Ordinary message send is agent communication with state-aware delivery, including resume of an existing unloaded thread. Explicit queue and exact steer remain separately selectable; neither implicitly resumes a target. Human-user input is explicitly named. An agent message carries the exact declaration below. Sender identity is self-declared, not authenticated, and the intended recipient is derived from the exact dispatch target. The adapter preserves that meaning through native representation where supported or a textual envelope where needed; a representation fallback never silently changes queue into steer. This is owner-authorized V1 behavior.

```text
Agent communication
Self-declared sender: <sender-address>
Intended recipient: <recipient-address>

<message body>
```

The caller supplies its self-address. The public clients construct the declaration consistently; callers need not manually prepend it. Explicit human-user input does not receive the agent declaration. Neither operation claims human or agent identity authentication merely from its spelling.

## Success evidence

All model-backed acceptance uses Luna explicitly through debug routing, with only fresh proof threads. The two agents must use the public CLI/SDK themselves to complete a small verifiable task and explicitly return its result; a harness forwarding their answers proves only transport. The core proof is a real discover → observe → send → receive result/events → explicit return-message workflow, with honest rejection and uncertain-outcome cases. Language interoperability requires usable client transcripts, not only schema compilation. Native and ACP conformance require their real protocol boundaries. Domain separation requires cases with multiple endpoints, colliding backend-local session IDs, backend replacement and unsupported capabilities. Remote delivery, when included, additionally requires real two-host allowed/denied access and disconnection evidence.

## Later shared message board

A follow-up may provide a SQLite-backed shared message board for agent-authored findings, theories, evidence references and discussion. This is separate from V1 lifecycle observations and from offline message-delivery receipts. Agents explicitly post and read through a public CLI/SDK; storage alone neither verifies claims nor establishes consensus. Board/session addressing, authorship provenance, edit history, subscriptions, access and retention need their own design. The board is not part of the two-PR V1 implementation and is not a new source of Codex conversation history.
