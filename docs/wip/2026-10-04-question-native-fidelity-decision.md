# Native answer fidelity — contract boundary

U3 preserves literal Other answers and shared typed intent while declaring native intent-kind fidelity unavailable. The stronger native category-marker promise was an agent-added inference, with no independent owner authority in Requirements U3 or the owner’s shared-model extension. This correction is separate from the selected shared-broker ownership direction.

## Current native model

| User action | Router's native reply value | Native interpretation boundary |
| --- | --- | --- |
| Select offered label `Safe` | `["Safe"]` | Reply contains no selected-option marker. |
| Enter Other text `Safe` | `["Safe"]` | Reply contains no Other marker. |

Both paths converge on identical native answer strings. Optional retained-context enrichment then matches those strings against offered labels.

For example, selecting the offered label `Safe` and entering Other text `Safe` both become `{"answers":{"field":{"answers":["Safe"]}}}`. The native reply type has no selected-option or Other marker. The tagged handler serializes the strings normally. When GuardianApproval retained-context enrichment is enabled, matching an offered label also appends that option's description; this is not a universal typed native choice decoder.

Primary source: `tmp/practices-research/2026-10-03-native-question-lifetime/protocol-user-input.rs` reply types; `request-user-input-handler.rs:99,119-127`; `bespoke-event-handling.rs:1682-1707`. Current native contract is Codex0.160.0. Current RQ2 cannot promise native intent-kind distinction for equal strings. Router's shared input can remain unambiguous before native serialization.

## Preserve literal answers — recommended

```text
offered selection -> original offered label
Other entry       -> original entered text
shared intent     -> preserved through validation and broker history
native intent     -> not represented by the upstream reply contract
```

Gain: all owner-requested Other text remains usable, with no upstream change or invented encoding. Cost: category fidelity ends at the native string boundary; users and consumers must not infer native intent kind from the literal value. Native optional label-based retained-context enrichment remains upstream behavior. The shared Question subsystem documents and proves this boundary. RQ2 must state shared-input distinction and literal native delivery, rather than a native marker the protocol cannot carry.

## Considered restriction — not adopted

```text
answer would collapse distinct intent
  -> reject before broker resolution
  -> keep Question pending and explain the native limitation
otherwise
  -> deliver original label or text
```

Gain: accepted answers avoid known label/intent collisions. Cost: some legitimate Other text becomes unavailable; duplicate offered labels may also make an offered occurrence ambiguous. The approver bears the restriction and the shared Question validator must explain it before committing a decision. This narrows the currently intended Other support and still adds no native category marker.

## Recommendation and deferral

Recommend literal answers: preserve the user's value and the shared typed intent, then report the native boundary honestly. Do not add guessed JSON fields, alter native settings or claim native category consumption. The owner chose a shared-model extension, and this keeps its useful text support within the existing native contract.

The Specification now states shared typed distinction and literal native delivery. Rejecting equal Other text would introduce a restriction that the approved extension does not require; it is not adopted. Exact structural interfaces and independent validation remain open. No U3 implementation or design-ready claim follows from the ownership selection or this correction.

## Decision state

Full brief presented to the owner on 2026-10-04, with one answer request. Subsequent full Host handback prompted an authority check: Requirements U3 authorizes visible, answerable questions, and its shared-extension clause authorizes Other/free text and secret metadata; neither requires a native category marker. Under the repository’s unauthorized-assumption rule, the unsupported stronger promise is removed while approved outcomes are preserved. RQ2 is clarified transparently. The existing question remains available for optional owner steering and is not a necessary design blocker; no duplicate question is opened. This is the Lead’s disposition, not independent review acceptance.
