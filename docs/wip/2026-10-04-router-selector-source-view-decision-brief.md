# Router selector source-view entry and return decision brief

Status: owner decision needed before planning. This brief is a design-phase artifact; it does not authorize implementation, remote access, authentication, or publication.

## Decision

Choose the user-visible entry and return contract for browsing one configured Router's existing sessions so the source-affine fork requirement can be realized without changing the default target.

## Evidence

- U7 requires the fork journey to work in each selected machine context while preserving source/session affinity: [Requirements](../specs/2026-10-03-router-selector/2026-10-03-router-selector-requirements.md#authorized-needs).
- R10 leaves the source view and its key unaccepted, while requiring one machine's actual inventory, no merged catalog, and retention of the prior view after a failed or canceled switch: [Specification](../specs/2026-10-03-router-selector/2026-10-03-router-selector-specification.md#observable-obligations), especially R10 and the machine-scoped fork section.
- The current picker handles F1/help, search, reload, Enter, Alt+Enter, and Esc, but has no F2 source-view branch: `crates/agent-collaboration/src/session_picker/picker_component.rs:254-317`.
- The current picker loader accepts only query fields and captures one default service directory and repository context: `crates/agent-collaboration/src/session_picker/picker_request.rs:26-41`; `crates/agent-collaboration/src/session_command_dispatch.rs:324-357`.
- The Program Design records the resulting loader, provenance, Stored-page, and refresh contracts as unresolved: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-program-design.md:202-212`.

## Options

### A. F2 opens a source chooser inside the existing picker (recommended)

F2 opens a bounded machine chooser from the hosted session view. Enter selects one configured/current source, then the picker replaces its rows with that source's read-only inventory and shows an explicit source header. The source-view Esc ladder is explicit: close help first, clear a nonempty search second, and when search is empty return to the prior query/focus without changing the default NEW target. This changes empty-search Esc from whole-picker exit to source-view return while the source view is active; that cost is visible in this choice. The source context, endpoint, cursor, and request generation travel together; a failed or canceled switch retains the prior view. Alt+Enter then captures a full source identity from that view.

This preserves the current picker as the navigation owner, keeps existing Enter/resume and Alt+Enter/fork meanings, and matches the already proposed U7/R10 shape. It costs one new source-context state, a source-parameterized loader contract, and the source-view Esc precedence above, all already identified as required design work.

### B. Add a separate source-selection command or pre-picker mode

The command or an earlier global mode selects a Router before the existing picker opens. This can avoid adding F2 handling to the picker, but it introduces a second navigation surface and risks changing the default/list/resume target. It conflicts with the requirement that source browsing be explicit and not a global target write. The fallback key or access path when F2 is unavailable remains deferred; it is not silently invented here.

### C. Do not browse configured sources

Keep only the current/default source and its same-source fork. This avoids the loader and source-view seam, but fails U7's requirement to make the same-source fork usable in each selected machine context and leaves R10 unrealized.

## Recommendation and question

Adopt **A**. It is the smallest structure that satisfies U7 while preserving existing bindings and default affinity. The next design pass would specify only the source-context/loader contract, Stored cursor consistency, and return/focus behavior needed by this mode; it would not add a merged catalog, global Router switch, federation, or a new external transport.

**Owner question:** approve F2 as the source-view entry, with an explicit source header and Esc return that preserves the prior query/focus and never changes the default NEW target?

After this choice, the remaining provenance/default qualification and fork cwd/policy decisions stay separate and must be resolved before planning admission. The fallback access path when F2 is unavailable also remains a named follow-up. No implementation or PR readiness is claimed by this brief.
