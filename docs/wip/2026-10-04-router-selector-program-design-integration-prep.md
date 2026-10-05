# Router selector final Program Design integration preparation

Status: queued behind D1/D2 owner decisions. This map identifies the exact structural cutover after decisions; it does not choose pending meaning or authorize implementation.

## Governing inputs

- Requirements: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-requirements.md`.
- Specification: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-specification.md`.
- Current Program Design: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-program-design.md`.
- D1/D2 packet: `docs/wip/2026-10-04-router-selector-d1-d2-decision-packet.md`.
- A2 source contract: `docs/wip/2026-10-04-router-selector-a2-contract-preparation.md`.

## Integration order

### Step 1 — consume D1

Record the owner-approved source-view entry/return contract in E8/R10 and the machine-scoped picker section. Bind the selected source-view state to the existing picker owner, including Esc/help/search precedence, prior query/focus restoration, failed/canceled return, and unchanged default NEW target. Keep F2-unavailable fallback explicitly deferred if D1 leaves it open.

### Step 2 — consume D2

Record the owner-approved default provenance rule in E5/R9 and the source-affine fork section. Select exactly one of the prepared `DefaultAttributed` paths: preserve today's default behavior with attribution separated, qualify through an observable home/binding contract, or fail closed after source inspection. Do not silently mix the options.

### Step 3 — bind A2 interfaces

Update the entity binding and ownership tables with these existing owners:

- **Picker source-view owner:** owns `PickerSourceContext`, source-view generation, query/focus snapshot, and source-switch cancellation/stale-result guard.
- **Dispatch loader owner:** owns source-aware request/result construction and source endpoint selection input; it cannot mutate picker outcomes or default NEW.
- **Inventory/control boundary:** owns endpoint/service consistency, Stored cursor validation, and closed rejection reasons. Stored cursors carry endpoint/view/source/scope/query/stored time/id and no live generation.
- **Action/fork owner:** owns `SessionSourceIdentity` and source-carrying resume/fork outcomes. `ObservedHosted`, `LocalHome`, and the D2-approved default path remain distinct.

### Step 4 — update flows and failure

Update the current/proposed call-path and state views to show source selection, endpoint binding, Stored pagination, cancellation, stale-result rejection, and source-affine fork capture. Preserve default/local paths and mark configured remote success as externally gated until X1 is supplied. Keep cross-machine fork disabled.

### Step 5 — update proof map

Fill R9/R10 interface cells with proof seams for source context, endpoint binding, Stored cursor mismatch, stale results, provenance, and configured-source refresh policy. Retain explicit gaps for remote exposure/auth/tool scope, native attachment binding, policy projection, runtime placement and state portability.

## Review gate

After the D1/D2-authorized Program Design correction, send only the affected anchors to the existing Claude reviewer for focused verification. Do not launch a new reviewer or reuse the Advisor as a reviewer. Planning admission remains closed until the reviewer verifies the corrected anchors and the orchestrator accepts the result.

## Explicit non-actions

No source code, persisted schema, registry format, endpoint exposure, SSH/Serve configuration, credentials, auth policy, VPN state, production process, PR, push, merge, or implementation plan is changed by this preparation artifact.
