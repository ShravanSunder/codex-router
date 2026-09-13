# Fable finding validation

Review source: `tmp/board-fable-current-review/review-findings.md`, Fable session `fbdf6e2f-97ae-4de9-87ca-3a98e51a0cba`. Reviewed snapshots matched current files before remediation. This record validates corrections; it is not a new independent review.

| Finding | Parent disposition and corrected anchor |
| --- | --- |
| F1 history range mismatch | Accepted. Specification Reading and Read examples use activity positions; returned scoped Range is directly accepted by messageList. |
| F2 cross-board order | Accepted. All scopes and all three modes use global activity order; Program Design Read flows agrees. |
| F3 cursor meanings | Accepted. U11/U16 distinguish bookmark, activity position and page cursor. Project inbox is unread catch-up; per-scope afterPosition also defined. |
| F4 failure enums | Accepted. Specification closes BoardFailureStage and BoardNextAction with mappings. |
| F5 guidance ownership | Accepted. Program Design assigns agent-communication SKILL/references and public CLI guidance, including permission and thread alternatives. |
| F6 repository derivation | Accepted. CLI owns Git discovery; pure normalization has a shared home; service validates without filesystem probing. |
| F7 receipt machinery | Removed per explicit owner direction. No operation ID, receipt table or inspection API. Existing resource IDs plus inspect-before-retry; no automatic replay. Reviewer claim of equivalent arbitrary retry safety rejected: intervening state changes can repeat effects, now explicitly disclosed. |
| F8 schema direction | Accepted. Only activity-to-message FK; response activity position obtained by join. |
| F9 acknowledge invalidation | Accepted. Neither acknowledgement nor watch changes invalidate page chains; the fixed upper bound plus future-only watches permits re-evaluating eligibility. |
| F10 closed shapes | Accepted. Human-only actingFor, removed optional messageCount, MessageSelection, unread inbox mode, actor-owned watch/ack and archived personal-reader-state behavior are explicit. |
| F11 residue | Accepted in affected anchors; lifecycle/review commentary kept in working trail. |
| F12 table | Accepted. U1-U31 are contiguous rows in one Requirements table. |
| F15 changed integration edges | Accepted. Service context store handle, dispatch, overload and Control schema registration named. |

No extra product scope was admitted. Durable receipts were removed, not replaced by a job engine. Existing implementation gates remain proof obligations; no runtime behavior is claimed implemented. This closes the listed mode-complete findings by parent verification within the user-authorized correction. The dispel lane and same-session schema follow-up are complete; their reductions below supersede this record's earlier incomplete-review status.

## Domain/schema follow-up reduction

Receipts: tmp/board-fable-current-review/domain-receipt-1.md and domain-receipt-2.md, same Fable session. Latest addendum covers boolean-only CHECK rule. Accepted: variant-free main-activity index, structural FK-parent index labeling, removal of enum defaults, migration-only FK-off ordering with precommit validation, baseline-owned seed, invalidRecord mapping, one-way activity FK and stale terminology. Simplified resulting-state duplication, redundant three-column message index and separate summary table (has_unread is now on project_reader_state). Kept payload uniqueness indexes as relational defense.

Rejected adding a message timestamp as a required owner decision: no current requirement asks for it and the withdrawn UI is not authority. UUID identity plus activity ordering remains the current contract; timestamp presentation can be separately requested. No new functionality was added for that observation.

Proof: revised embedded DDL parses in Python SQLite with 15 tables and only two boolean CHECKs. A populated parent/child SQLite probe verifies FK-mode ordering and data-preserving rebuild mechanics. These probes do not establish SQLx bundled-runtime migration correctness. Required permanent SQLx migration tests remain in the implementation proof plan. No additional reviewer dispatched.

## Final parent verification of schema receipts

Both complete schema receipts were read in full from the ACPX export. The initial advice to retain enum CHECKs is superseded by the explicit owner rule and the later Fable addendum. Current-file verification resolves each addendum item:

| Item | Disposition / current design section |
| --- | --- |
| Variant-specific index predicate | Removed; Query indexes uses root_id IS NULL. |
| FK-parent unique-index dependency | Retained and identified as structural in Migration connection and rebuild protocol. |
| Enum defaults | Removed; Rust supplies initial state. |
| Migration FK mode / precommit integrity | Explicit migration-only connection protocol and Migration proof contract. |
| Stored-row decode failure | invalidRecord/inspection/inspectResource in Specification. |
| Duplicated activity state | Removed; Rust derives state from activity kind. |
| Separate unread summary table | Removed; derived boolean stored on project_reader_state. |
| Redundant three-column message FK parent | Removed; two-column message/topic FK plus topic/board ancestry retained. |
| Seed ownership | Baseline migration owns exactly one checkpoint seed. |
| ER message/activity edge | Corrected, with transaction-guaranteed message activity explained. |
| Table rendering and obsolete sequence language | Current U1-U31 table and activity-position terminology checked. |
| Message-time suggestion | Rejected as unrequested extension; no timestamp feature added. |

Accepted findings are resolved in the current design by parent verification; no new reviewer is required for these bounded corrections. The domain/constraint/migration review is closed. Design is ready to enter implementation planning against the three distinct artifacts. This is not an implementation completion claim: permanent SQLx runtime, populated migration, concurrency, CLI/SDK and performance proof remain mandatory delivery gates.
