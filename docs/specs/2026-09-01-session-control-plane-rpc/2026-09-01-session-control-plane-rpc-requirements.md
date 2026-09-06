# Session Control Plane — Requirements

> Historical design, superseded by the [Agent communication system Requirements](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-requirements.md), [Specification](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md), and [Program Design](../2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-program-design.md). Retained for requirement provenance; this document does not govern implementation.

## The outcome

A developer can keep using a managed Codex session while Codex Host replaces
its app-server. Live connections and active turns may end, but the service
remains discoverable at one owner-local address and `agent-sessions` returns to
the same materialized thread without manual recovery.

Applications receive three supported public interfaces without flattening
their meanings: standard Agent Client Protocol (ACP), the complete native Codex
app-server protocol, and Session Control JSON-RPC for Host generation,
managed-session coordination, truthful inventories, and explicit compositions.

This document owns why, for whom, and within what boundary. The separate
[Specification](./2026-09-01-session-control-plane-rpc-specification.md) owns
observable behavior. The separate
[Program Design](./2026-09-01-session-control-plane-rpc-program-design.md) owns
internal structure.

## People and jobs

### Developer using Sessions

```text
find, filter, search, resume, fork, or start a Codex thread
  -> work through a managed interactive child
  -> Host replaces app-server
  -> observe bounded recovery instead of permanent disconnection
  -> continue from the same materialized thread
```

The existing Sessions command is a product, not merely a launcher. Its scopes,
filters, formats, search language, picker, launch choices, and Codex argument
passthrough must survive the move to `agent-sessions`.

### Application or agent client

```text
discover one local service
  -> choose ACP, native Codex, or Session Control for this connection
  -> retain that protocol's exact methods, results, errors, and events
  -> reconnect explicitly when a generation-local native channel closes
```

Portable applications need ACP. Codex-aware applications need the entire
native protocol, not a copied subset. Control clients need Host and Sessions
state plus state-aware cross-thread send and interrupt behavior.

### Host operator

The operator replaces the managed app-server, observes generation readiness
and managed-session recovery, and never replays an unknown native request or
touches production by accident.

### SDK and adapter maintainer

The maintainer consumes versioned JSON Schemas to generate exact
TypeScript/Zod and Rust clients, verifies schema identity at runtime, and can
later add a curated MCP adapter without changing command semantics.

## Current facts that constrain the result

- Codex owns native thread identity, persistence, turns, approvals,
  subscriptions, callbacks, events, queue behavior, and app-server schemas.
- ACP owns its standard names, capabilities, request lifecycles, updates,
  errors, and extension rules.
- ACP and Codex both use JSON-RPC but are not aliases. ACP `session/prompt`
  completes after streamed updates; native `turn/start` returns an in-progress
  turn immediately.
- `thread/start` allocates a loaded ID, but a blank thread is not cold-resumable
  until Codex materializes persisted history.
- Replacement ends generation-local sockets, subscriptions, callbacks, pending
  requests, and active work. Persisted history and native queue rows may
  survive; live protocol state does not.
- Stored, loaded, active, subscribed, current, resumable, attached, and
  supervised are independent facts.
- Native `turn/start` is start-or-steer at dispatch time and does not reveal
  its branch. `turn/steer` addresses one exact active regular turn.
- Codex owns a bounded experimental SQLite FIFO queue and its pause, wake, and
  restart rules.
- Router account, quota, provider, affinity, and routing data has a separate
  database and reason to change.

## Authorized needs

The repository owner assigns every priority. Every row is authorized and
currently applicable.

| ID | Priority | Affected class | Need and intended outcome |
| --- | --- | --- | --- |
| U1 | Must | Sessions user | Recover an expected replacement onto the same materialized Codex thread and validated working directory. |
| U2 | Must | Sessions user | Distinguish normal exit, cancellation, release, recovery, and recovery failure; terminal intent never respawns. |
| U3 | Must | Existing Sessions user | Preserve table/JSON listing; cwd, checkout, repository, and all scopes; provider/source filters; created/updated sorting; limits and keyset paging. |
| U4 | Must | Existing Sessions user | Preserve exact-ID, latest, Start New, Resume, Fork, hosted/local, dry-run, and ordered lossless Codex argument passthrough. |
| U5 | Must | Existing Sessions user | Preserve qualified search, bounded previews, responsive layouts, keyboard/pointer interaction, loading, and distinct terminal failures. |
| U6 | Must | Local application | Discover one owner-local service and select exactly one public protocol channel per connection. |
| U7 | Must | Portable agent client | Use the complete negotiated ACP contract from the pinned official schema bundle with its names, capabilities, timing, notifications, and extension rules preserved. |
| U8 | Must | Codex-aware client | Use the complete negotiated native app-server protocol without a copied subset, renamed methods, weakened types, or semantic translation. |
| U9 | Must | Native client | Reconnect to the same address after replacement closes the old native connection; pending requests are never automatically replayed. |
| U10 | Must | Local control client | Use public Session Control for generation, managed-session lifecycle/inventory, reconnect coordination, schema discovery, and explicit compositions. |
| U11 | Must | Local control client | List stored threads, exact runtime-active threads, and managed clients without collapsing independent states. |
| U12 | Must | Local control client | Send input by native state: active steerable uses exact `turn/steer`; loaded non-active uses `turn/start`; unloaded resumable uses resume then `turn/start`. |
| U13 | Must | Local control client | Report the native operation accepted and exact correlation without claiming an unobservable start-versus-steer branch; completion remains event-driven. |
| U14 | Must | Local control client | Stop current work by exact interruption without releasing, unsubscribing, stopping background terminals, archiving, or deleting. |
| U15 | Must | Codex-aware client | Retain complete native queue persistence, FIFO, auto-dispatch, pause, paging, mutation, start, limits, and experimental semantics. |
| U16 | Must | Sessions user | Expose a blank Start New ID as non-resumable; on generation loss create a new blank ID and visibly report replacement without a synthetic turn. |
| U17 | Must | Sessions user | Fork attaches the new materialized fork identity; recovery never substitutes the source ID. |
| U18 | Must | Host operator | Publish ordered generations and recoverable managed-session state across control reconnect and Host re-exec. |
| U19 | Must | Client maintainer | All channels obey JSON-RPC 2.0 and their governing request-ID/batch rules; Session Control V1 additionally requires non-null connection-unique request IDs and accepts non-transactional batches. |
| U20 | Must | Client maintainer | Advertise separate versioned channel schemas, digests, capabilities, and selectors for SDK/Zod generation and verification. |
| U21 | Must | Maintainer | Session Control uses closed discriminated unions, validated domain types, exact method/params/result/error pairing, and deliberate absence/null semantics. |
| U22 | Must | Operator | Persist control metadata separately from router account/quota state and Codex history/queue state. |
| U23 | Must | Local owner | V1 has no protocol authentication, authorization, network ingress, remote deployment, or multi-user security claim. |
| U24 | Must | Repository maintainer | `agent-sessions` is separate; `codex-router` retains serve/host/accounts/quota; `codex-native-integration` replaces the legacy `codex-router-codex` name. |
| U25 | Must | Repository maintainer | New or moved responsibility-bearing file/folder names use at least two meaningful words; conventional structural entries are exempt. |
| U26 | Must | Human reader | Requirements, Specification, and Program Design remain distinct, linked, human-readable, and traceable to proof. |

## Decisions that define the product

- One advertised owner-local service provides public ACP, native Codex, and
  Session Control channels. Channel selection is immutable per connection;
  method families never mix on one connection.
- ACP and native Codex are first-class interfaces. Shared internal capabilities
  do not normalize their wire types, completion timing, errors, or events.
- The native channel relays the complete upstream protocol rather than cloning
  it. It closes on generation replacement. The client reconnects; the address
  remains stable and pending requests are not replayed.
- Session Control owns only Host generation, managed-session coordination,
  runtime projections, schema discovery, and named compositions. It is not ACP,
  a generic agent backend, or a shadow Codex protocol.
- `method` discriminates commands/notifications. Nested variants use one
  purpose-specific string discriminant: `kind`, `state`, `desiredState`, or
  `code`.
- Each channel publishes its own authoritative schema and digest. ACP and Codex
  retain upstream schema authority; Session Control generates from its typed
  registry. There is no combined authority schema.
- Generated SDKs are the primary application interface. A future curated MCP
  server may use the SDK; JSON-RPC alone is not MCP and MCP is outside V1.
- Recovery preserves a materialized thread, not a process, connection,
  subscription, callback, pending request, or active turn.
- `active` means exact Codex `ThreadStatus::Active`; no other state implies it.
- `SendMessage` uses exact steer when active, native `turn/start` otherwise,
  and resume then `turn/start` when unloaded. The latter two report submission
  acceptance because native dispatch may start or steer after a race.
- Queueing is explicit and Codex-owned. No second prompt queue exists.
- `StopThread` is exact `turn/interrupt` with response after abort. Release,
  unsubscribe, archive/delete, and terminal control remain separate.
- A blank ID is `allocatedNotMaterialized`. Generation loss produces and
  reports a replacement blank ID, never synthetic input. Fork produces a
  distinct materialized identity.
- Every existing Sessions capability is preserved. Incidental quirks remain
  non-normative debt and are not silently changed by this design.
- `codex-router sessions` is removed at hard cutover; `agentstudio-tui` is a
  later naming decision.

## Goal boundary

### In scope

- complete Sessions discovery, picker, launch, and failure behavior;
- supervised recovery after app-server replacement;
- one local service with ACP, complete native Codex, and Session Control
  JSON-RPC channels;
- exact per-channel lifecycle, capabilities, schema, and replacement behavior;
- generation, managed-session, and runtime-thread observation;
- Codex-aligned `SendMessage` and `StopThread` compositions;
- separate control persistence and generated SDK/schema readiness;
- the named executable/crate boundaries and debug-only proof.

### Protected foundations

- the negotiated ACP protocol schema shipped by the pinned ACP SDK bundle and
  the running Codex app-server's native schema;
- Codex history, queue, runtime, approval, callback, and event semantics;
- router account/quota/provider routing and every production process;
- Sessions reading normal Codex state, not a fake debug home.

### Non-goals

- no migration of sockets, pending requests, subscriptions, callbacks, or
  active turns; no automatic unknown-outcome replay;
- no synchronous send-and-wait-for-reply or generic `AgentBackend`;
- no Host-owned Codex history, queue, turn, approval, or event semantics;
- no mixed-protocol connection or combined schema;
- no authentication, authorization, remote/public listener, or cross-Mac use;
- no MCP implementation or complete Codex-to-MCP translation in V1;
- no resurrection after terminal intent, compatibility shim, final
  `agentstudio-tui` branding, or unrelated reorganization.

## What success must prove

- Every current Sessions feature has an inspectable preserved disposition.
- ACP conformance proves every method in the advertised negotiated ACP schema
  and its prompt/update/cancel lifecycle; native conformance proves every
  advertised Codex method/event.
- Replacement closes native connections; explicit reconnect reaches the same
  address and no unknown request is replayed.
- Discovery exposes three channel selectors with independent schemas, digests,
  versions, capabilities, and stability.
- State inventories never conflate stored, loaded, active, subscribed,
  attached, current, resumable, supervisor, or child state.
- Every SendMessage state/race/failure branch and native queue boundary behaves
  exactly as specified.
- Blank recovery visibly replaces identity; materialized resume and Fork retain
  their correct identities.
- Codec/schema evidence proves method pairing, closed variants, domain types,
  batch behavior, and pre-dispatch rejection.
- State/process inspection proves store separation, no second prompt queue, and
  untouched production processes.
- Source inspection proves executable/crate separation and meaningful names.
