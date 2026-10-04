# Router selector A2 source-contract preparation

Status: bounded Program Design preparation; not a final structural decision, implementation plan, or implementation authorization. D1 source-view entry/return and D2 default provenance remain owner decisions.

## Current source facts

- `SessionsPickerRecordLoader` accepts only `SessionsPickerDataQuery`: `crates/agent-collaboration/src/session_picker/picker_request.rs:25-41`.
- Dispatch constructs one loader closure that captures one repository context and one service directory: `crates/agent-collaboration/src/session_command_dispatch.rs:324-358`.
- The picker owns a watch-channel reload worker, a local reload generation, and a three-second refresh loop: `crates/agent-collaboration/src/session_picker/picker_component.rs:76-115,185-203`.
- Stored inventory results have `generation: Option<CodexGeneration>`, and Stored cursors are based on stored time/id rather than live generation/native cursors: `crates/collaboration-protocol/src/native_session_catalog.rs:42-97`; `crates/collaboration-service/src/session_inventory_dispatch.rs:218-232,360-372,412-418`.
- The client validates that an inventory result and every row target the requested endpoint, but does not provide a source-view contract: `crates/collaboration-client/src/control_connection.rs:294-317`.
- Local stored rows begin as `LocalCodex`; runtime refresh can stamp them with a default endpoint and produce `HostedCodex(SessionRef)`: `crates/agent-collaboration/src/session_commands/session_catalog_records.rs:95-149`; `session_commands/picker_runtime_inventory.rs:320-345`.
- Picker resume/fork outcomes currently carry bare `String` IDs: `crates/agent-collaboration/src/session_picker/picker_actions.rs:21-26`.

These facts establish the missing boundaries. They do not choose the F2 key/return behavior, default provenance qualification, fork cwd, or remote transport.

## Proposed contract seams (to bind after D1/D2)

### 1. Source-aware request/result boundary

**Owner:** source-view picker owner creates requests; dispatch loader owner resolves and queries the selected source.

The current query must be extended conceptually with an invocation-local source context and attempt generation:

```text
SourceInventoryRequest {
  sourceContext: PickerSourceContext,
  endpointSelector: ConfiguredOrDefaultEndpointSelector,
  view: Stored | Loaded | Active,
  scope: SourceOwnedScope,
  sourceFilter: Interactive | Subagents | All,
  query: SearchExpression,
  cursor: SourceCursor | None,
  requestGeneration: SourceViewGeneration,
  cancellation: ReadOnlyRequestCancellation
}
```

The loader returns a closed result owned by the source-view owner:

```text
SourceInventoryResult =
  Ready { sourceContext, endpoint, page, requestGeneration }
  | Rejected { sourceContext, requestGeneration, reason }
  | Canceled { sourceContext, requestGeneration }
```

`requestGeneration` is picker-local and prevents a stale source/query result from replacing the active view. A result cannot launch, change the default NEW target, or mutate a different source view. Cancellation is read-only best effort; stale-result rejection remains mandatory when cancellation races.

The existing three-second refresh loop must not be inherited automatically by a configured source. Refresh ownership and cadence remain a source-view design choice after D1; default/local refresh behavior remains unchanged.

### 2. Source-to-endpoint binding boundary

**Owner:** connection verifier; consumed by dispatch loader and source-view owner.

The registry's expected stable service identity is not itself an endpoint. The verifier must map one selected source to one currently advertised endpoint and return either:

```text
EndpointBinding =
  Bound { serviceIdentity, endpoint, generationOrStoredContext }
  | Rejected { reason: WrongService | EndpointUnavailable | EndpointUnqualified | StaleSnapshot }
```

The exact endpoint selector and whether a source may expose multiple eligible endpoints remain open. No machine label, UUID match, local endpoint, or remote health result substitutes for this binding. For Stored pages, `generationOrStoredContext` must carry the Stored consistency context rather than inventing a live generation.

### 3. Stored cursor and source consistency

**Owner:** inventory dispatch validates the cursor; source-view owner invalidates it on context changes.

A Stored cursor may carry:

```text
StoredSourceCursor {
  endpoint,
  view: Stored,
  source,
  scope,
  query,
  storedTime,
  storedSessionId
}
```

It must not carry a live `CodexGeneration` or native runtime cursor. A continuation is accepted only when endpoint, source, scope, query and view match the request. Source switches, query changes, cancellation, or failed return invalidate the cursor and any row/action snapshot. The picker-local `SourceViewGeneration` is separate from the Router's live generation and guards UI freshness; it does not become a Stored cursor field.

### 4. Observed versus attributed identity

**Owner:** inventory projection and fork/resume action owner.

The action boundary must distinguish the following closed source identity cases:

```text
SessionSourceIdentity =
  ObservedHosted { session: SessionRef, sourceContext, metadata }
  | LocalHome { codexHome, sessionId, metadata }
  | DefaultAttributed { sessionId, defaultEndpointAttribution, metadata }
```

`ObservedHosted` is permitted for configured-source actions and for rows returned by a verified source inventory. `DefaultAttributed` preserves today's default catalog behavior but is not positive historical Router provenance; it must not be reinterpreted on a named remote source. `LocalHome` retains the unhosted local source. A fork/resume selection carries this identity and source context instead of a bare `String` ID. The final choice between qualifying default-attributed rows and preserving today's default fork behavior is D2.

## Proof seams

- Request/result unit seam: current source context, endpoint, cursor, and request generation remain paired; stale and canceled results cannot replace the active view.
- Endpoint binding seam: wrong-service, unavailable, multiple/unclear endpoint and changed snapshot reject before source rows become actionable.
- Stored pagination seam: matching endpoint/source/scope/query continues; a mismatched or generation-bearing Stored cursor rejects; a source switch invalidates continuation.
- Provenance seam: configured-source actions require `ObservedHosted`; default-attributed rows cannot be sent to another Router; local rows retain `LocalHome`.
- Refresh seam: configured source does not inherit default three-second polling without an explicit source-view contract.
- Cross-machine fork seam remains disabled until portability/transport proof; no state copy or fallback is introduced.

## Deferred owner choices and dependencies

- **D1:** source-view entry/return and Esc precedence; the existing brief recommends F2.
- **D2:** default-attributed provenance versus preserving today's default fork behavior; see the provenance brief.
- **D3:** effective fork cwd/policy and visible origin.
- **D4:** whether a rejection-only remote slice can be planned before external exposure/binding/policy prerequisites.

A2 can be integrated into Program Design only after D1 and D2 settle the source-view owner and default identity rule. This packet intentionally leaves those choices open and does not authorize implementation.
