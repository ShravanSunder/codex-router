# Router selector default provenance decision brief

Status: queued after D1 source-view decision. This is source-grounded design input, not a second owner question while D1 is pending. It does not authorize implementation, protocol changes, inspection calls, auth, or publication.

## Decision

Choose how the selector treats a default-hosted stored row whose local catalog record is stamped with the current default endpoint, even though that stamp is attribution rather than observed historical Router provenance.

## Evidence

- The local stored catalog opens `StoredThreadCatalog` from the caller's resolved Codex home: `crates/agent-collaboration/src/session_commands/session_catalog_query.rs:83-101,104-119` and `crates/collaboration-client/src/session_catalog/query.rs:86-123`.
- The hosted launch target resolves the same `CODEX_HOME` or `$HOME/.codex` rule before selecting the Router service directory: `crates/agent-collaboration/src/session_commands/session_launch_selection.rs:158-181` and `:208-241`.
- Host startup receives an explicit collaboration Codex home and stores it in the lifecycle owner: `crates/codex-router-host/src/host_configuration.rs:132-139`; `crates/codex-router-host/src/lifecycle_owner/collaboration_lifecycle.rs:43-55`.
- Native control backends carry `codex_home` for Stored inventory reads: `crates/collaboration-service/src/native_control_dispatch.rs:9-16`; Stored inventory reads `backend.codex_home`: `crates/collaboration-service/src/session_inventory_dispatch.rs` in `stored_page`.
- Control initialization exposes service/epoch/schema information but no Codex-home field: `crates/collaboration-protocol/src/control_initialization.rs:25-36`.
- The current picker refresh stamps local rows as `HostedCodex(SessionRef)` when a native endpoint is present: `crates/agent-collaboration/src/session_commands/picker_runtime_inventory.rs:320-345`; the row projection itself starts as local: `session_catalog_records.rs:95-149`.

The local-source path therefore has a strong construction-time equality signal when the picker and the default Host share the same resolved home. A configured remote source cannot infer the caller's home or historical source from a matching UUID or label, and the endpoint handshake does not expose the Host's home for comparison.

## Options

### A. Add an observable source-home qualification contract

Expose a qualified source-home identity or equivalent binding in the default inventory/initialization contract, then treat a default row as observed only when the local catalog home and the default Host home match. This gives the strongest provenance result but changes a control/protocol contract and requires a compatibility and proof path.

### B. Preserve today's default behavior and separate attribution from observation (recommended)

Keep default resume/fork behavior unchanged. Model the stamped endpoint as default-catalog attribution, not historical observation. Require full observed `SessionRef` source context for configured-source rows and never reinterpret a default/local bare ID on another Router. This preserves U3 and avoids a new failure on existing default forks; its cost is that default historical provenance remains explicitly unproved.

### C. Qualify every default fork by inspection and fail closed

Inspect the default source before allowing a hosted fork; reject when the source cannot be qualified. This gives fail-closed provenance but adds a new failure to an existing default journey and may change current fork behavior.

## Recommendation and deferred question

Recommend **B** for the current bounded scope. It preserves the owner-authorized existing-session journey and keeps the provenance distinction truthful. After D1 is settled, present this as D2: approve preserving default behavior with attribution/observation separated, while requiring observed source identity for configured-source actions?

The choice remains separate from D3 fork cwd/policy and D4 rejection-only remote planning. No implementation plan is ready until the owner decisions and the A2 Program Design bindings are complete.
