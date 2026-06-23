# Lane: Planning Readiness And Adversarial Crux

Status: answered
Verdict: needs revision

Accepted candidate findings:

- Threshold constants are policy-by-magic-number unless the spec makes them fixed v1 constants or config defaults.
- Scenario B is not testable because it says `may outrank`.
- Official/refreshed status vs runtime selector state authority is unresolved.
- Auth-adjacent observability boundary is under-specified.

Required revision:

- Freeze or configure thresholds with rationale and proof boundaries.
- Rewrite Scenario B with exact expected winner or exact tie-band/tie-break rule.
- Define authoritative state when refreshed status and runtime selector state disagree.
- Add allowed/forbidden output fields for status and explanations.
