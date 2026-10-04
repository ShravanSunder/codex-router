# Router selector formal design review result

Status: `needs-revision`; planning admission and design acceptance are not established.

This digest is a public-safe, source-backed handoff for the reopened selector lane. It summarizes the formal cross-lineage review and affected correction verification without reproducing private Router transcripts. No implementation, PR, auth, network, settings, or production action is authorized by this artifact.

## Reviewer and scope

- Reviewer lineage: Claude Code, independent of the Lead's OpenAI session.
- Model: `claude-opus-5-5[1m]`, confirmed by the reviewer's harness metadata.
- Reasoning effort: unavailable; no effort value is inferred.
- Review context: no selector-author history was present. Prior unrelated Interaction SQLite review context was disclosed and excluded from selector evidence.
- Target: Requirements, Specification, Program Design, source-view decision brief, handoff/trace, and the current picker/catalog/dispatch/protocol source packet.
- Source head at packet: `3c78398d`; trace-only follow-up was later recorded in `20b4e2c6`.
- Review scope: three-artifact design review; no commands, runtime, tests, Cargo, network, auth, or file writes by the reviewer.

## Coverage and direct source anchors

The reviewer read the complete three artifacts, brief, handoff, and trace. Source checks covered:

- Picker input and effect boundary: `crates/agent-collaboration/src/session_picker/picker_component.rs:200-345`.
- Provider-row and action behavior: `crates/agent-collaboration/src/session_picker/picker_model.rs:280-300`; `picker_actions.rs:21-26`.
- Loader boundary: `crates/agent-collaboration/src/session_picker/picker_request.rs:25-41`; `crates/agent-collaboration/src/session_command_dispatch.rs:300-358`.
- Local-row attribution and runtime inventory: `session_commands/picker_runtime_inventory.rs:55-66,320-345`; `session_commands/session_catalog_records.rs:142-149`.
- Stored inventory semantics: `crates/collaboration-service/src/session_inventory_dispatch.rs:218-232,360-372,412-418`; `crates/collaboration-protocol/src/native_session_catalog.rs:42-97`.
- Loopback/identity feasibility: `mcp_http_listener.rs:40-60`; `control_initialization.rs:25-36`.

## Accepted findings and correction state

### A1 — closed

Specification R4 now makes the configured `defaultRemoteCwd` the only named-NEW cwd source. Missing configured cwd rejects before creation. The orphaned explicit destination-path branch was removed. The chooser and Program Design use the same configured-default semantics.

### A2 — open planning blocker

U7/R10 still need Program Design bindings for:

- a source-parameterized dispatch-to-picker request/result and cancellation boundary;
- configured source to native endpoint selection;
- Stored-page cursor/source consistency without reusing live generation checks;
- observed-vs-attributed identity and source-carrying resume/fork actions.

Current evidence: the loader accepts only `SessionsPickerDataQuery`; dispatch captures one service directory; picker outcomes carry bare string IDs; local rows can be stamped with a hosted-looking endpoint; Stored cursors explicitly have no live generation. These are structural How gaps, not planner choices.

### A3 — closed

The leftover “Direct noninteractive named NEW reports rejection and exits” sentence was removed. No `--router`, `--remote-cwd`, or named-NEW explicit-path surface remains. `ConflictingArguments` remains only for the valid named-route passthrough invariant.

### A4 — closed

The source-view decision brief now exposes the Esc ladder: close help, clear nonempty search, then return on empty search while preserving prior query/focus and the default NEW target. It names the cost that empty-search Esc changes from whole-picker exit inside source view. F2-unavailable fallback remains explicitly deferred.

## Owner decisions still pending

- **D1:** approve the F2 source-view entry/return contract in [the decision brief](2026-10-04-router-selector-source-view-decision-brief.md).
- **D2:** choose default-fork provenance qualification versus preserving today’s default fork behavior.
- **D3:** settle effective fork cwd/policy and visible origin.
- **D4:** decide whether a rejection-only remote slice is plannable before external exposure, binding, and policy prerequisites are available.

The owner-facing recommendation for D1 remains F2 with an explicit source header, the corrected Esc ladder, and no default NEW mutation.

## External feasibility boundary

Remote MCP exposure/tool visibility, attachment-time native binding, permitted caller-policy projection, and remote runtime/state-portability proof remain unverified prerequisites. The artifacts correctly classify these as feasibility gaps rather than silently inventing auth or transport mechanisms.

## Current verdict and next route

The formal result remains `needs-revision`. A2 stays open behind D1/D2; after the applicable owner decisions, the next route is Program Design to bind the source context, loader, Stored cursor, and provenance/action contracts, followed by focused verification from the same reviewer. No implementation plan, PR readiness, or acceptance is claimed.

Validation of the digest: `git diff --check` passed before checkpointing; the artifact contains no private Router transcript or credential material.
