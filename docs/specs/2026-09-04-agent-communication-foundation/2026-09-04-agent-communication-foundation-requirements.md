# Agent communication foundation — Requirements

> Historical design, superseded by the [Agent communication system Requirements](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-requirements.md), [Specification](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md), and [Program Design](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-program-design.md). Retained for requirement provenance; this document does not govern implementation.

## What this enables

A developer or another agent can find a Codex thread, send it information, observe what happens, and continue using it after the shared app-server is replaced. An ACP client can have a portable conversation with Codex. A Codex-aware client keeps access to the complete native protocol.

This is the communication foundation a future manager can use. It does not choose workers, assign goals, judge results, or become another agent execution loop.

The [Specification](./2026-09-04-agent-communication-foundation-specification.md) defines observable behavior. The [Program Design](./2026-09-04-agent-communication-foundation-program-design.md) explains its realization.

## The people and agents using it

**Developer at a terminal.** Find, search, start, resume, or fork a thread. Keep the existing picker and command behavior. When Host replaces app-server, see a bounded recovery attempt instead of having to reconstruct the session manually.

**Application or harness developer.** Connect through a documented transport and generated types. Know whether a request was rejected, accepted, interrupted, or left with an unknown outcome. Subscribe before sending when live output matters.

**Agent sending information to another agent.** Choose an exact target thread and explicitly send content. Sending does not establish a parent/child relationship or cause every subsequent assistant answer to return automatically.

**Client listening for activity.** Observe a thread or Host event without submitting another prompt. A protocol notification reaching client software and an LLM starting another turn are different outcomes.

**Host operator.** Retain process-lifecycle authority, distinguish installation from activation, and replace only the intended managed app-server. Investigation and acceptance proof must leave production processes untouched.

The owner remains responsible for scope and consequential operational decisions.

## The user journeys

```text
Another harness sends information
  discover local endpoint → select ACP or native/control client
  → select exact session/thread → establish event observation
  → send content → receive acceptance or terminal ACP result
  → observe subsequent output and failures on the documented stream

Agent B sends information back to A
  B explicitly chooses A → B submits a new message to A
  → A's native execution rules decide the admitted operation
  → clients observing A see the resulting events

A client observes without asking for work
  connect → subscribe → receive notifications
  → display/react in client software
  → no model turn begins merely because a notification was delivered

A developer survives replacement
  work in managed Sessions → old app-server generation ends
  → see reconnecting → newer generation becomes ready
  → resume the same materialized thread, or report why that is impossible
```

Native Codex additionally exposes `thread/inject_items` for model-visible context without starting a turn. Its idle-history and active-pending-input guarantees differ; the native channel preserves those semantics. This is not a protocol notification or an automatic wake mechanism.

The distinction in the third journey is a proposed interpretation of the owner's message-versus-notification clarification. Whether notifications should additionally wake an idle agent is an open owner choice below; no scheduler or wake policy is authorized by the diagram.

## Needs and their basis

U1–U26 retain the identities of the September 1 Requirements so existing needs cannot disappear during rewriting. Their current basis is that document's authorized rows, except U19's native-dialect correction follows the mandated native protocol itself. U27–U28 come from the current owner's communication request and retain their explicitly unassigned priority. U29 records the owner's later online-relocation boundary; U30 records the owner's required descriptive CLI/skill workflow. U29's boundary and U30's capability are required; that does not make relocation implementation a V1 task. Priorities for U1–U26 remain those assigned in the governing record.

| ID | Class | Need / outcome | Priority; authority and evidence |
| --- | --- | --- | --- |
| U1 | Sessions user | Recover expected replacement onto the same materialized thread and validated cwd. | Must; inherited authorized U1. |
| U2 | Sessions user | Normal exit, cancellation, release, and recovery failure are distinct; terminal intent never respawns. | Must; inherited authorized U2. |
| U3 | Sessions user | Preserve list formats, scopes, filters, sorting, limits, and keyset paging. | Must; inherited authorized U3 and current Sessions code. |
| U4 | Sessions user | Preserve exact/latest/new/resume/fork, hosted/local, dry-run, and ordered Codex arguments. | Must; inherited authorized U4 and launch source. |
| U5 | Sessions user | Preserve search, previews, responsive picker interaction, loading and distinct failures. | Must; inherited authorized U5. |
| U6 | Application | Discover one owner-local service and choose an unambiguous channel. | Must; inherited authorized U6. |
| U7 | ACP client | Use the negotiated ACP contract, preserving names, capabilities, lifecycle, and content rules. | Must; inherited authorized U7; pinned official schema is protocol authority. |
| U8 | Native client | Use the complete native app-server protocol without a renamed subset or altered wire behavior. | Must; inherited authorized U8; upstream protocol is authority. |
| U9 | Native client | Reconnect explicitly after replacement; never replay an unknown request automatically. | Must; inherited authorized U9. |
| U10 | Control client | Observe Host generation, managed sessions, inventories, schemas, and explicit compositions. | Must; inherited authorized U10. |
| U11 | Control client | Distinguish stored, loaded, active, subscribed, and managed state. | Must; inherited authorized U11. |
| U12 | Control client | Send by native state: exact active steer, loaded input submission, or resume then submission. | Must; inherited authorized U12. |
| U13 | Control client | Learn the accepted native operation and correlation limits; observe completion separately. | Must; inherited authorized U13. |
| U14 | Control client | Interrupt the exact turn without implicitly releasing, archiving, or deleting anything. | Must; inherited authorized U14. |
| U15 | Native client | Retain Codex's complete native queue semantics; no replacement prompt queue. | Must; inherited authorized U15. |
| U16 | Sessions user | A blank unmaterialized identity is not falsely called resumable; replacement is visible. | Must; inherited authorized U16. |
| U17 | Sessions user | Fork attaches its actual new identity; recovery uses that identity. | Must; inherited authorized U17. |
| U18 | Operator | Recover ordered generation and managed intent across Host/control reconnection. | Must; inherited authorized U18. |
| U19 | Client developer | Respect each channel's real envelope, ID, batching, and framing contract. | Must; inherited authorized interoperability need, corrected by native rpc.rs. |
| U20 | SDK developer | Obtain separate versioned schemas, exact digests and capability information. | Must; inherited authorized U20. |
| U21 | SDK developer | Use closed Control unions and exact method/params/result/error pairings. | Must; inherited authorized U21. |
| U22 | Operator | Keep coordination persistence separate from Router routing and Codex history/queue state. | Must; inherited authorized U22. |
| U23 | Local owner | Do not introduce unauthenticated network ingress or make unsupported security claims. | Must; inherited authorized local-only V1 limit; remote expansion is open below. |
| U24 | Maintainer | Separate Sessions product from provider Router and native Codex integration responsibilities. | Must; inherited authorized U24, including agent-sessions naming/cutover. |
| U25 | Maintainer | Use responsibility-specific multi-word names for new/moved modules. | Must; inherited authorized U25. |
| U26 | Human reader | Follow need → observable contract → structural explanation → proof without reading research notes. | Must; inherited authorized U26 and current request. |
| U27 | Sending agent | Explicitly send information to a separately addressed Codex thread; a reply is another explicit send. | Priority unassigned; authorized outcome from current owner, exact delivery policy subject to U12 and open choices. |
| U28 | Listening client/agent | Receive an observable event separately from a content-bearing message. | Priority unassigned; authorized distinction from current owner; idle-model wake behavior unresolved. |
| U29 | Session operator | Keep later explicit session relocation feasible, without requiring continuous replication in V1. Transfer assumes the source and destination computers are online. | Must preserve this boundary; relocation implementation is later. Authorized by the owner's relocation and online-transfer clarification. |
| U30 | Agent and CLI user | Discover targets, explicitly send information and listen using descriptive CLI commands and skill guidance, without constructing raw RPC frames. | Must; authorized by the owner's request for easy, descriptive agent communication. |

“Inherited authorized” preserves an existing need during reconciliation; it does not claim the owner has confirmed this entire replacement package. The original source is [September 1 Requirements](../2026-09-01-session-control-plane-rpc/2026-09-01-session-control-plane-rpc-requirements.md).

## Boundary of this proposal

The existing local RPC scope is the basis while the owner decides whether this package also includes cross-machine communication. Allowed design surfaces are Codex Router's Host, its native integration, the Sessions product, ACP adaptation, public Control protocol and generated clients. The proposed implementation is in this repository; other repositories are evidence and future consumers.

Preserve Codex's execution loop, thread/history/queue ownership, approvals, subscriptions and callback semantics. Preserve Router account/quota/provider routing. Preserve normal Codex home for real session discovery. Debug proof uses isolated Router state/endpoints without replacing production processes.

The current package does not design a manager, job engine, cron service, distributed registry, peer ACL service, durable shared mailbox, automatic assistant-result routing, remote execution substrate, Hermes deployment, Agent Studio UI, or Tool Portal/MCP implementation. Those are distinct possible consumers or later capabilities, not prerequisites for an ACP client to converse with Codex.

A model needs a harness-provided client/tool to initiate calls. An ACP agent/server endpoint alone does not give its model an outbound ACP client. The proposed CLI and SDK supply callable local client surfaces; harness-specific installation or automatic model wake integration remains outside this local foundation.

## Session relocation is later and explicit

Relocation will be a deliberate command in a later delivery. V1 does not require a continuous or streaming session-replication system. The initial transfer model assumes both source and destination computers are online; it does not promise to recover a session from an unavailable source.

Session checkpoint blobs and a metadata service are a future direction for recording transferable state and its location. They are not required V1 infrastructure. The checkpoint format, native-version compatibility, workspace transfer and exclusive ownership handoff still need design and proof before relocation can be promised. A replicated checkpoint alone does not authorize two active writers.

This boundary concerns session replication and relocation. It does not remove live RPC responses or event observation from the communication foundation. It also does not settle the separate policy for messages addressed to an offline agent.

## Native TUI recovery constraint

At the pinned Codex revision, the native TUI already reconnects to an out-of-process app-server. It can remain running with input paused when recovery or history restoration fails. A preallocated, unmaterialized thread still cannot be resumed, even while its creator is subscribed.

The foundation must reuse that native recovery owner and must not infer UI recovery from a live process. The inspected public CLI/Rust entrypoint does not expose a live thread-identity/reconnect observation feed to an external wrapper; its exit result is too late for managed live reporting. U16's automatic blank replacement and the associated live managed-identity/recovery evidence remain unsatisfied until a supported integration is established or the owner explicitly defers those outcomes. This is not authorization to weaken them, inject synthetic history, fork Codex, parse terminal text, or guess the newest thread.

## Choices still requiring the owner

1. **Notification delivery and model wake.** Proposed: message submission explicitly invokes native input; a notification reports an event to a listening client without starting a turn. Alternative: notification delivery itself must wake an idle model, requiring a harness integration and an explicit delivery/retry policy.
2. **Local or cross-machine scope.** Proposed: retain owner-local V1 and provide transport-ready client boundaries. Alternative: include authenticated Router-to-Router/Tailscale communication in this package. That needs endpoint identity, authorization, disconnection and credential rules before normative remote contracts can be written.
3. **New-need priority.** U27 and U28 are required outcomes in the current request, but no delivery ordering or must/should distinction has been assigned by the owner.

The local foundation can be specified independently of those later choices. A remote listener, automatic model wake policy or durable offline-message service is not implied by the relocation clarification.

## What success must demonstrate

- An independent ACP client creates or loads a Codex session, sends a prompt, receives ordered updates and the exact terminal response, and handles cancellation or backend loss.
- A native client sees the same application messages and reverse requests through the relay as through a direct app-server connection.
- A client subscribes before sending to another root, receives honest acceptance and correlated turn events, and can explicitly send another message back.
- Watching events alone creates no prompt or model request under the proposed notification behavior.
- Replacement ends the old connection; explicit reconnect resumes materialized history without replaying unknown work.
- The existing Sessions feature inventory is preserved, with genuine terminal interaction evidence.
- Schema validation, state inspection, error/race transcripts, and process/socket inspection prove the public contracts and protected boundaries.

Proof must distinguish protocol acceptance, model execution, receipt of an event, completion of a turn, and receipt of a peer reply. None is a substitute for another.
