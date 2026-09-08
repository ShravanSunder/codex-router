# Simplified automation design — review resolution

> Historical closeout only. The 2026-09-08 responsibility revision replaces stored active/waiting pointers with Run-derived admission and separates timer progress from schedule configuration. The ready result below applies to the earlier design and does not approve that revision or its implementation plan. See [current reconciliation](2026-09-08-schedule-responsibility-correction.md).

Result: ready for implementation planning within the local nine-table scope. No implementation or runtime proof is claimed.

Independent source: [Astra review](2026-09-07-automation-astra-review.md), receipt `scheduled-workflows-nine-table-2026-09-07-01/result-01`, three-artifact-design mode. Astra used fresh context and enforced read-only access, read all three artifacts and verified their hashes unchanged. Original verdict: needs-revision. The parent accepted F1–F7 and applied one bounded correction pass, then verified the corrected anchors. No second review was run or represented as having run.

Governing authority remains current Requirements S1–S42, including the owner's approved nine-table simplification and two-month schedule/wake edit history. No requirements were removed by remediation. Explicit agent replies remain the default; event wake-ups remain a follow-up.

## Findings and corrections

| Finding | Parent disposition and concrete correction | Current anchor |
| --- | --- | --- |
| F1 imported continuity | Accepted: imported text/provenance is current schedule state, captured into run inputs. Overwrite cannot alter active run inputs; no synthetic local source run. | Specification ContinuityInput and continuity capture; Program Design imported_continuity_json and Imported continuity |
| F2 scheduled readiness | Accepted: run coordinator checks generation-scoped readiness, defers observed busy targets, records eligible-start timing/effects, observes exact turn, then handles summary/release. Normal messages/wakes retain auto behavior. | Program Design Scheduled execution readiness and result flow |
| F3 partial effects | Accepted: one NativeEffectEvidence preserves allocation/resume/submission, known native IDs, generation/correlation and cessation through current, archived and error payloads. | Specification NativeEffectEvidence and DeliveryEvidence |
| F4 inspection | Accepted: AttemptHistoryPage exposes retention coverage; RunSnapshot exposes current RetainedSummary text/provenance. Latest attempt merged exactly once with retained events. | Specification AttemptHistoryPage, RetainedSummary and continuity/coverage contracts |
| F5 operation lookup | Accepted: show/reconcile share tagged OperationSnapshot with method-specific success and effect-bearing nonterminal/failure alternatives. No receipt-only conflicting shape remains. | Specification OperationSnapshot and operation/show |
| F6 one-shot pause | Accepted: an exhausted paused one-shot finishes without firing; wait returns wakeFinishedWithoutFiring. Historical firing and original expiry semantics remain. | Specification first-fire vocabulary and one-shot rule; Program Design Exhausted one-shot reminders |
| F7 mutable configuration | Accepted: explicitly add Host-composed AutomationConfigurationStore owning automation-settings.json; serialized receipt/file/sync/recovery ordering, not a claimed existing mutable Host store. No extra table. | Program Design Host-composed configuration adapter |

F7 adds the missing concrete realization of already-authorized mutable timeout configuration, not a settings engine or new product capability. F6 derives the terminal outcome from the approved skip-paused-ticks rule; it does not replay a missed one-shot. Unknown native outcomes still block automatic replay and slot release.

## Verification

- Thirteen in-memory SQLite schema/state checks passed, exit 0: first conditional claim wins; competing sequential claim loses; stale release cannot clear the slot; cross-schedule pointer and duplicate native address are rejected; imported continuity, current summary, first-fire evidence and instruction history survive event deletion; overwrite leaves captured continuity intact; nine tables, zero triggers, valid FKs; all 87 columns commented.
- Extracted five TypeScript wire-declaration blocks and ran `tsc --strict --noEmit --skipLibCheck --target es2022 /tmp/astra-remediated-wire-types.ts`: exit 0. Existing V1 external types were opaque placeholders; this verifies the new declarations, not native-schema compatibility.
- Relative document links and code-fence balance pass. All edits are under docs/specs or docs/wip; product source unchanged.
- Verified released Croner 4.0.0 source directly from crates.io: Seconds/Year::Disallowed and find_next_occurrence. Archive SHA-256 a59ed87528b2ee2b6d2b14d6767f6dbe41867dfc581433fc23a7c0fae847467a. This closes the review's exact-version evidence caveat without asserting runtime conformance.
- Parent checked the current native partial-effect and immutable Host configuration source anchors named by the reviewer. The proposed adapters now explicitly state what is new.

These checks do not prove concurrent SQLx workers, native fork/start races, file-update crash recovery, sandbox-callable CLI or Luna runtime behavior. Those remain named implementation proof gates. The review result plus this bounded parent verification is the design closeout; no independent post-correction verdict is claimed.

## Current artifact hashes

- 2026-09-07-scheduled-agent-workflows-program-design.md: `f8fa9ee1573df51b16adcee3438886632cddbd51240d89ec801fb462810ba319`
- 2026-09-07-scheduled-agent-workflows-requirements.md: `d0a1fbaf3ed4f57e8830743deb207869673e1e2bcd45b5ebd0ef002171c24283`
- 2026-09-07-scheduled-agent-workflows-specification.md: `1edcafa7722b8d4b096f32a28d9d75862a57583ef793518c36046c22decb681c`
