# Router selector admission and decision packet

Status as of 2026-10-05: design-only, `needs-revision`. This existing packet is the single status and decision entry point; the retained briefs below supply detail. No implementation or final Program Design binding is authorized.

## Current readiness

The same Selector Lead owns the lane in `codex-router.router-selector-design`, branch `router-selector-design`. At consolidation entry, HEAD was `fe20c63cb2ebc9a1e4a9ba1afbaf82736359d2b1` and the worktree was clean. The diff from source baseline `7a8cbd8943e6fb2dac23bcde7069203925003918` contains only selector documents and images: no product/configuration implementation, admitted canonical implementation plan, or Selector PR.

Tailnet communication and multiple-app-server configuration behavior are **unimplemented and untested in this lane**. Read-only CLI/source observations and document validation do not prove those behaviors. No live forwarding/exposure, native creation/fork, multiple-config isolation, reconnect/cancel/failure, remote auth/policy projection, or local/default implementation regression proof has run. The tailnet transport comparison is a proposal awaiting approval and qualification, not runtime readiness.

The [formal review disposition](2026-10-04-router-selector-formal-review-result.md) remains `needs-revision`. The retained distinct-lineage Claude reviewer reported model `claude-opus-5-5[1m]`; reasoning effort was unavailable. Its design/source inspection and focused verification closed A1, A3 and A4. A2 remains open. That coverage does not establish runtime proof, implementation review, or acceptance of the later tailnet proposal.

The latest source audit confirms that full routing identity already exists in `EndpointRef`, `SessionRef`, and `SessionPickerIdentity::{HostedCodex,HostedProvider}`. The unresolved A2 seam is the picker outcome/dispatch conversion to bare `String` IDs; final Program Design should carry the existing identity through actions rather than invent a second identity type.

## Admission map and retained evidence

| Gate | Current state and recommendation | Retained detail |
| --- | --- | --- |
| D1 source-view entry/return | **Next owner decision, already pending.** Recommend the existing F2 in-picker flow with explicit source header, visible Esc ladder and unchanged default NEW. Do not infer an answer or create a duplicate request. | [Source-view brief](2026-10-04-router-selector-source-view-decision-brief.md) |
| D2 default provenance | Queued owner choice. Recommend preserving default behavior while separating attribution from observation; configured-source actions retain observed source identity. | [Provenance brief](2026-10-04-router-selector-provenance-decision-brief.md) |
| D3 fork cwd/policy | Queued owner choice. Recommend preserving default cwd behavior, showing effective directory/origin, and qualifying configured-source cwd/policy before fork. | [Fork cwd/policy brief](2026-10-04-router-selector-fork-cwd-policy-decision-brief.md) |
| D4 delivery tolerance | Queued owner choice. The local/default and rejection-only slice is a proposed planning boundary; remote success remains obligatory and cannot be counted complete through rejection or stand-ins. | [Rejection-only planning brief](2026-10-04-router-selector-rejection-only-planning-decision-brief.md) |
| A2 structural contracts | Source-only preparation refined and focused-verified on 2026-10-05: opaque Stored/Runtime continuations, valid sparse pages, budget suspension, view-specific checks and full publication guard. Final Program Design bindings and their affected review remain open. These are Lead-owned design work, not additional blanket owner blockers. | [A2 preparation](2026-10-04-router-selector-a2-contract-preparation.md) |
| External transport qualification | Conventional OpenSSH and Serve are transport-capable candidates. Neither has been enabled or proved here. Identity binding, authorization/tool scope and native policy projection remain unverified; no new endpoint exposure is authorized. | [Tailnet proposal](2026-10-04-router-selector-tailnet-endpoint-brief.md) |
| Final design/review admission | Consume actual owner choices, bind A2, and have the retained reviewer verify affected contracts before planning admission. No new reviewer is commissioned by this packet. | [Integration preparation](2026-10-04-router-selector-program-design-integration-prep.md) |

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
