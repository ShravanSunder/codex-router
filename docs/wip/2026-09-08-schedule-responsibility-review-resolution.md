# Schedule responsibility revision — review resolution

Independent source: [Astra review](2026-09-08-schedule-responsibility-astra-review.md), identity 2026-09-08-schedule-responsibility-independent-stdout, three-artifact-design mode. Fresh context, enforced read-only, all three documents read fully. Original result needs-revision; configuration/timing/Run separation found coherent. No second independent reviewer was dispatched after correction.

## Parent dispositions

F1 accepted (S23, S16/S17, S42): CapturedRunInputs now contains FrozenExecutionConfiguration including destination mode/address/workspace and timeout override. Program Design maps it to captured_inputs_json and forbids rereading mutable definitions for admitted execution. Effective service-default timeout still resolves at eligible execution start.

F2 accepted (S17, S7/S9): same-Run summary recovery is distinct from successor admission. BEGIN IMMEDIATE verifies only the requested Run occupies execution, eligible phase and prior attempt cessation; it installs a new attempt without relinquishing occupancy. Duplicate retry and stale completion guards use operation/attempt identities. No additional Run or stored pointer is introduced.

F3 accepted (S28, S9/S19, S41): RunSnapshot now exposes RunExecutionEvidence with structured native partial effects, timing independent of native turn-ID availability and actual NativeSendReceipt when established. SQLite evidence and query projections update in the same transaction; conflicting duplicate views fail validation.

All corrections stay in the existing scope and use existing records. No new tables, event-wake behavior, retention change or upstream lock added. Finished-Run retention remains unchanged. The owner-facing timing separation remains the current structural proposal; do not infer approval of a different retention model from this review.

## Verification and limits

Parent reopened cited contract/source boundaries and reproduced the three inconsistencies before correction. Corrected extracted wire declarations pass tsc --strict --noEmit --skipLibCheck --target es2022; V1 referenced contracts treated as opaque, so no native-schema conformance claim. SQLite DDL parses with FKs enabled and zero triggers. Sequential SQL probes verify same-Run retry remains occupying, successor remains waiting, stale attempt completion and duplicate retry are rejected, and schedule edits do not change captured configuration. Links and fences pass.

These are bounded parent-verification checks, not SQLx concurrent-worker, process-crash or native-runtime evidence. Those remain required implementation gates. The correction of the review findings is complete; this record is not a new independent ready verdict. The earlier nine-table/pointer implementation plan remains inapplicable and must not be executed unchanged.
