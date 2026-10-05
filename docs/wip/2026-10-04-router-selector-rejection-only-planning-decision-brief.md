# Router selector rejection-only planning decision brief

Status: queued after D1/D2 and formal review. This brief does not authorize implementation or claim remote feasibility.

## Evidence

- Requirements U2/U4/U5 require selected-machine execution and verified service/endpoint capability; rejection alone does not fulfill the remote success outcome: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-requirements.md:24-36`.
- Specification R4/R5/R7 require real remote execution, identity/attachment binding, and post-handoff uncertainty proof: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-specification.md:49-52`.
- Current source/design evidence leaves remote MCP exposure, native attachment binding, permitted policy projection and runtime proof unestablished: `docs/wip/2026-10-03-router-selector/bounded-design-handoff.md:35-41`.

## Options

### A. Plan local/default and preflight rejection slices only (recommended)

Plan registry validation, chooser behavior, default preservation, source-contract rejection and explicit unavailable states. Mark remote success and post-handoff proof as blocked external prerequisites with no implementation claim. This gives useful local delivery while preserving the remote gate.

### B. Wait for external prerequisites before any plan

Do not plan even local/default slices until a real remote exposure, binding, policy and runtime proof path exists. This avoids partial delivery but leaves validated local behavior unplanned.

### C. Plan the full remote outcome against stand-ins

Use substituted exposure/binding responses to plan and implement the remote success path. This risks treating stand-ins as proof and hides the exact external contract still missing; it is rejected by R4/R5/R7 proof obligations.

## Recommendation

Prefer A if the owner accepts a staged plan whose remote success remains a hard external gate. D4 is an owner delivery-tolerance decision: whether a rejection-only/local slice is valuable before remote prerequisites exist. No implementation plan is admitted until D1-D4 and the formal review corrections are settled.
