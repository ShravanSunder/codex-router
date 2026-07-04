# Quota Status Reset-Pace UX Plan Ledger

Date: 2026-07-04
Plan: `docs/plans/2026-07-04-quota-status-reset-pace-ux/implementation-plan.md`
Reviews:

- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-revised-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-third-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-fourth-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-final-plan-review.md`

## Source Coverage

Accepted source is chat-only design plus accepted plan-review findings. No
source artifact file exists.

Covered requirements:

- 15-minute status/sample display freshness.
- Runtime selector authority remains unchanged unless separately specified.
- Stale data remains visible in quota status.
- Reset-pace language replaces safe-pace language.
- Center-origin burn meter with yellow/green/red state bands.
- Weekly bar plus percent remains.
- Repeated `needs refresh` and confusing pace prose removed from human-facing status.
- JSON compatibility is preserved unless new fields are explicitly named.
- `serve` remains DB writer; status/session commands remain read-only observers.
- Unsafe account labels, raw account IDs, provider errors, secrets, and high-cardinality telemetry remain out of status output/telemetry.

## Planning Inputs

Original plan-creation lanes:

| Lane | Reasoning effort | Agent | Artifact | Status |
| --- | --- | --- | --- | --- |
| codebase-boundary | medium | `019f2aba-4422-74f3-be98-8da57a5095a8` | `lanes/codebase-boundary.md` | answered |
| validation-proof | high | `019f2aba-7b1c-7eb2-aec4-f04d29505221` | `lanes/validation-proof.md` | answered |
| security-reliability | high | `019f2aba-bca6-7963-812f-0467bfe25f92` | `lanes/security-reliability.md` | answered |

Plan-review lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-plan-cohesion | `019f2ac2-95ce-7a32-ac17-4f19f65515c9` | needs revision |
| testability-validation | `019f2ac2-caf1-7fc3-ad36-730a02f3480b` | needs revision |
| architecture-assumptions + execution-scope | `019f2ac2-feeb-7191-aea8-b715b96667b0` | needs revision |
| security-reliability | `019f2ac3-4129-7d02-9c0d-709b6d8d3d8e` | needs revision |

Revised plan-review lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-plan-cohesion | `019f2ad2-2c79-7822-bb5f-91a95f563213` | needs revision |
| testability-validation | `019f2ad2-30f6-7b11-a941-de6cba03da09` | needs revision |
| architecture-assumptions + execution-scope | `019f2ad2-351f-7512-b2ff-5277994f2cac` | needs revision |
| security-reliability | `019f2ad2-3952-7610-b7ba-b51fd49b2db3` | ready |

Third plan-review lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-plan-cohesion | `019f2add-eaa4-7422-aaea-0f4081752083` | ready |
| testability-validation | `019f2add-ef98-7f52-83b8-01766aaea9ec` | needs revision |
| architecture-assumptions + execution-scope | `019f2add-f4dc-7700-a49b-ea41feecec0c` | ready |
| security-reliability | `019f2add-f8ee-7270-a9fb-b500d722dd31` | ready |

Fourth plan-review lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-plan-cohesion | `019f2ae6-3b58-7d81-b528-2b45f81f5ee2` | ready |
| testability-validation | `019f2ae6-820f-7aa3-b8ea-d017ebfd4a5d` | ready |
| architecture-assumptions + execution-scope | `019f2ae6-d44b-7b40-9bc7-60d4c99a9711` | ready |
| security-reliability | `019f2ae7-1ff4-73a1-b37a-cc66ec7ef372` | needs revision |

Final focused plan-review lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-plan-cohesion focused re-review | `019f2aec-b381-74a3-8733-5bb9d05bf36d` | ready |
| security-testability telemetry re-review | `019f2aec-f770-77f0-a31d-b2914d8cce4a` | ready |

No new subagents were dispatched for this revision. The revision used accepted
fourth plan-review findings as the bounded planning inputs.

## Accepted Review Findings and Disposition

| Finding | Disposition |
| --- | --- |
| 15-minute freshness can change runtime routing authority | Accepted. Plan now scopes 15 minutes to status display and preserves runtime selector authority. |
| Existing dirty product diffs collide with write surfaces | Accepted. Gate 0 now classifies the `crossterm`/terminal-width diff before overlapping edits. |
| Reset-pace meter needs typed view-model boundary | Accepted. Plan now requires a typed reset-pace model before presentation work. |
| Output contract too open | Accepted. Plan now defines human labels and JSON compatibility rules. |
| Stale-value proof not tied to no-provider-I/O | Accepted. Plan now requires a combined stale/no-provider fixture. |
| 15-minute projection boundary not directly proved | Accepted with reduction. Plan now proves status display threshold and separately proves selector authority remains 300 seconds. |
| Visual/manual gate underspecified | Accepted. Plan now names fresh, stale, degraded, and unavailable-burn capture cases plus checklist. |
| Full validation gate incomplete | Accepted. Plan now names fmt, clippy, nextest, deny, and audit. |
| Account-label safety assumption false | Accepted. Plan now requires safe labels or equivalent sanitization. |
| Degraded-read proof implicit | Accepted. Plan now requires degraded stale-value proof with no preferred authority. |
| Telemetry sample-age proof vague | Accepted. Plan now forbids exact sample age/text telemetry labels. |
| Post-implementation route points to plan review | Accepted. Plan now routes to `implementation-review-swarm`. |

## Accepted Revised Review Findings and Disposition

| Finding | Disposition |
| --- | --- |
| Runtime-authority guard is aimed at the wrong layer | Accepted. Plan now routes R2 proof through state/projection persisted stale-after behavior and names test-only state/projection write surfaces. |
| Sample confidence lacks an exact age source | Accepted. Plan now defines sample age from displayed value-bearing quota windows, using the oldest displayed observed sample for row-level metadata. |
| Slice 1 and Slice 2 are not parallel-safe around the shared DTO/view-model contract | Accepted. Plan now inserts a serialized shared DTO contract gate before parallel helper work and serializes row/view-model integration. |
| Typed reset-pace ownership is semantically defined but not module-defined | Accepted. Plan now names presentation-facing DTO ownership, `quota.rs` construction/classification ownership, and renderer boundaries. |
| CI-equivalent proof gate omits workflow lint | Accepted. Plan now adds `actionlint .github/workflows/ci.yml` to the CI-equivalent gate. |
| Visual/manual cases should name reset-pace state coverage | Accepted. Plan now names `fresh-healthy`, `stale-under`, `degraded-over`, and `unavailable-burn` capture cases with ANSI/non-ANSI inspection. |

## Accepted Third Plan Review Findings and Disposition

| Finding | Disposition |
| --- | --- |
| Shared DTO proof can still miss renderer string parsing | Accepted. Plan now requires an adversarial renderer/source-guard test proving typed reset-pace/sample fields drive rendering while labels or glyph strings contain sentinel conflicts, and explicitly forbids production renderer-side parsing of display strings. |
| Visual capture matrix names states but not per-state ANSI/non-ANSI artifacts | Accepted. Plan now requires a `case x width x style` capture matrix with paired `.txt` and `.ansi` artifacts for every named case at 48-column narrow width and 160-column sidecar width. |

## Accepted Fourth Plan Review Findings and Disposition

| Finding | Disposition |
| --- | --- |
| Telemetry proof must cover tracing/log attributes, not only OTel metrics | Accepted. Plan now defines telemetry as both tracing/log attributes and OTel metric labels/values for the status surface, and requires the telemetry contract guard to cover exact sample ages/text, raw provider errors, raw account IDs, raw account labels, and unsafe labels across both surfaces. |

## Rejected or Deferred Items

- Runtime routing freshness changes are rejected for this plan. A separate routing spec is required to change selector authority.
- Product code changes remain out of scope for plan creation.
- No new dependency change is planned; existing dependency diffs are current branch baseline to classify before implementation.
- Gate 0 still owns whether the current `crossterm` terminal-width diff is accepted branch baseline or an unrelated blocker.

## Accepted Vertical Slices

1. Status-only sample freshness and stale values.
2. Reset-pace burn model.
3. Quota status layout and copy cleanup.
4. Observer, redaction, and degraded-read guardrails.

Accepted execution correction:

- A shared status DTO/view-model contract gate precedes Slice 1 and Slice 2 integration.
- Slice 1 and Slice 2 pure helper tests may run in parallel after the contract gate.
- Integration through `QuotaStatusRow`, `QuotaStatusAccountViewModel`, and `QuotaSelectedAccountViewModel` is serial or single-owner.

## Completion Receipt

- Product code edited: no.
- Test/config files edited: no.
- Plan artifacts revised: yes.
- Review findings resolved in plan: yes, including accepted fourth-review telemetry proof-boundary finding.
- Final plan review verdict: ready.
- Recommended next workflow: `implementation-execute-plan`.

phase_result: complete
evidence: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-final-plan-review.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Final plan review found no remaining blocker or important findings after the telemetry proof revision.
