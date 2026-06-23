# R5 Lane Receipt: Contract + Architecture + Spec Difference

Agent: Sagan
Status: answered
Verdict: needs revision

Coverage: read the full 1014-line spec, R4 ledger, and named code anchors.

Candidate findings:

- Blocker: all-unknown fallback is selectable but public status can contradict
  runtime behavior.
- Important: candidate tie ordering is still ambiguous, and
  `WeightedDeficitSelector` uses input order for equal weights.

Parent disposition: accepted. The spec now closes unknown fallback status and
the salvage tie key.
