# Router selector D1/D2 decision packet

Status: owner decision packet; no implementation or final Program Design binding.

Resolve these in order. D1 fixes the source-view owner and navigation boundary. D2 then fixes whether default-attributed rows may qualify for source-affine actions. A2's final source/loader/provenance binding depends on both.

## D1 — source-view entry and return

**Question:** Approve F2 as the source-view entry from the existing hosted picker, with an explicit source header and this Esc ladder: close help, clear nonempty search, then return on empty search while preserving prior query/focus and never changing default NEW?

**Evidence:** The current picker has Enter/resume, Alt+Enter/fork, Ctrl+N/Start-new and Esc handling but no F2 branch: `crates/agent-collaboration/src/session_picker/picker_component.rs:254-317`. U7/R10 require one-machine source inventory and source-affine fork without a global target write: Specification R9/R10 and Requirements U7. The source-aware loader/Stored consistency gaps are documented in the [A2 packet](2026-10-04-router-selector-a2-contract-preparation.md).

**Options:**

- **A — F2 in-picker source chooser (recommended):** smallest structure; preserves existing bindings and default affinity; adds one source context and loader boundary.
- B — pre-picker/global source selector: adds a second navigation surface and risks changing list/resume/default targeting.
- C — no configured source browsing: avoids the seam but fails U7/R10.

**Cost:** Empty-search Esc returns from source view instead of exiting the whole picker while that view is active. F2-unavailable fallback remains a separate follow-up; it is not invented here.

## D2 — default provenance and qualification

**Question after D1:** Approve preserving today's default/local fork behavior while separating default catalog attribution from observed source identity, and require full observed `SessionRef` context for configured-source actions?

**Evidence:** Local catalog and local launch target resolve the same `CODEX_HOME`/`$HOME/.codex` rule (`session_catalog_query.rs:83-119`; `session_launch_selection.rs:158-181`). Host startup receives an explicit Codex home and Stored inventory reads the backend home (`host_configuration.rs:132-139`; `lifecycle_owner/collaboration_lifecycle.rs:43-55`; `native_control_dispatch.rs:9-16`). Control initialization does not expose Codex home (`control_initialization.rs:25-36`). Runtime refresh can stamp a local row with a hosted-looking endpoint (`picker_runtime_inventory.rs:320-345`), which is attribution rather than historical observation.

**Options:**

- **A — preserve default behavior and separate attribution (recommended):** no new failure in the existing default journey; configured-source actions require observed identity; default-attributed rows never move to another Router by UUID guess.
- B — add an observable home/binding contract: strongest provenance but changes a control/protocol contract and proof surface.
- C — inspect and fail closed for every default fork: strongest fail-closed behavior but changes current default fork failure semantics.

**Dependency:** D2 cannot be finalized until D1 identifies which source-view actions need observed identity. D3 fork cwd/policy and D4 rejection-only planning remain separate decisions.

## Admission consequence

Until D1 and D2 are answered, the formal review remains `needs-revision`, A2 remains preparation rather than final Program Design, and implementation planning is not admitted. No source code, endpoint, credential, VPN, auth, or production change is implied by this packet.
