# Obvious push truncation — Specification

Governing Requirements: [requirements.md](requirements.md), U6 and the operational limits. This bounded design changes push-preview presentation. It preserves the other push-format obligations in [the existing Specification](../2026-09-30-router-push-format/specification.md), particularly source-scalar counting, escaping, byte budget, complete links, stored bodies and display-only header parsing.

## The consumer's problem

A recipient sees up to 100 characters of a long message followed by a bare count, but the preview does not visibly end as truncated. The same rendered line appears across native Codex, ACP, Claude peer, inbox/history and scheduled or broker pushes. The unchanged `show` operation returns the full stored record and body; it is the expansion path, not another preview surface.

| Current long preview | Required long preview |
| --- | --- |
| `"first 100 source characters" (+1167)` | `"first 100 source characters…" (+1167 more chars)` |

## Domain entities

| ID | Term and same-instance rule | Relationships | Invariants / states | Basis |
| --- | --- | --- | --- | --- |
| E1 | Push: the existing Machine + push identity denotes one snapshot. | One target, one origin, zero or one body, one link. | Identity and full body survive display shortening; existing delivery states unchanged. | U6; existing push E2 |
| E2 | Preview: one rendering's visible prefix of that Push body. Two renderings may expose different lengths under their display budget. | Belongs to one E1; paired with E3 when truncated. | At most 100 source Unicode scalars; full or shortened; a shortened preview ends in one added ellipsis. | U6; existing push R1/R3 |
| E3 | Omitted count: the number of original body scalars outside E2's visible source prefix. | Derived from one E1 and E2. | Nonnegative; source prefix + omitted count equals full source-scalar count; escaping and the added ellipsis do not contribute. | U6; existing push R1 |
| E4 | Push link: the Machine + push identity points to E1. | Exactly one E1. | Never shortened; resolution/access rules unchanged. | U6; existing push E5/R3/R6 |

## Observable obligations

- **R1 (U6; E1–E3):** Whenever any body text is omitted, the quoted preview ends with `…` immediately before its closing quote. This applies both to the 100-scalar cap and further byte-budget shortening.
- **R2 (U6; E2–E3):** The suffix is exactly ` (+N more chars)` for positive N. N counts omitted Unicode scalar values in the original body, before escaping. Use `chars` for every positive count, including one. The ellipsis is an added display marker and does not change N.
- **R3 (U6; E1–E4):** An unshortened body has no added ellipsis and no omitted-count suffix. A body that already ends in an ellipsis retains its content. Subscription notices remain bodyless. Empty bodies remain unshortened.
- **R4 (U6; E1–E4):** The shared line remains at most 1,024 UTF-8 bytes with a complete push link and the existing escape/separator grammar. Added display text participates in budget fitting. The stored full body and push identity remain unchanged.
- **R5 (U6; E1–E4):** Every shared push surface uses this one format. Header parsing and delivery correlation continue to use the existing header and push identity rather than the preview or count. No alternate old/new rendering paths remain.

## Failure and proof

Renderer validation failures retain their existing outcomes. The complete link and fixed kind facts remain protected. Under the current valid-input limits and 1,024-byte budget, header fitting can always retain the initial 100-scalar preview; a shorter or zero preview is not a reachable production scenario. The marker/count rule still follows whatever source prefix is actually shown.

| Obligation | Independent observation |
| --- | --- |
| R1–R3 | Source bodies of 0, 99, 100, 101 and 1,340 scalars; exact quoted output and suffix; Unicode and escaping cases; all body-bearing kinds. |
| R4 | Escape-heavy bodies and large valid headers exercise actual machine/name fitting at the real 1,024-byte limit; assert byte bound, complete link, and exact source-derived prefix/count/marker. Full-body fetch remains exact. No synthetic lower-budget seam substitutes for the production limit. |
| R5 | Real producer/adapter boundary tests and the existing delivered-message matrix assert the literal long-preview grammar across its surfaces. Header-derived title excludes body/suffix; correlation uses push identity. |

The rule changes existing push-format R1 and scenario S2 only as to ellipsis and count wording; all other obligations remain in force.
