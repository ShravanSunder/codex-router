# Adversarial Crux + Guardrail Codification + UX Lane

Status: answered
Verdict: needs revision
Coverage: full 837-line spec reviewed with goal details and current routing,
weighted deficit, quota snapshot, and CLI code anchors.

## Findings

- Blocker: `selected_next` overclaims because runtime selection also depends on
  proxy-owned affinity and live weighted-deficit state.
- Important: missing-reset text says pressure is used for ranking, but later
  unknown fallback gives all unknown accounts weight `1`.
- Important: non-blocking proof does not force delayed-refresh proof on the
  first `/v1/responses` WebSocket path.
- Important: default human status is still example-shaped; `status` is
  ambiguous and fixed `5h`/`weekly` slots need a v1 invariant.

## Guardrail Candidates

- Default status must not claim runtime-exact next selection unless it observes
  proxy fairness state and excludes affinity-bearing traffic.
- In a selected known pool with neutral fairness state, dominance should hold:
  an account no worse on every relevant window and better on at least one should
  not receive lower routing weight.
- First valid `/v1/responses` WebSocket request must route/open without waiting
  for live refresh.
- Default human table has exactly one displayed short slot and one displayed
  long slot, with explicit overflow/summarization.
- Unsafe label becomes deterministic safe hash/tag.

Completion receipt: answered.
