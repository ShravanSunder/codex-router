# Lane: Algorithm Prior Art And Crux

Status: answered
Verdict: needs revision

Key result:

- The algorithm family is broadly right: classifier/state machine + burn-down pressure assessment + weighted fairness.
- DRR/weighted deficit is a fairness primitive, not quota-risk math.
- Token bucket/GCRA/EDF are useful analogies, not correctness proofs for snapshot-based multi-window quota routing.

Candidate blocker:

- The assessment-to-weight contract is under-specified. A future plan could get the sign or normalization wrong and route toward danger rather than away from it.

Required revision:

- Add exact pressure sign semantics, cross-window reduction, clamp/rounding rules, and worked examples with deterministic winners.

Prior-art anchors:

- `https://web.stanford.edu/class/ee384x/EE384X/papers/DRR.pdf`
- `https://datatracker.ietf.org/doc/html/rfc2697`
- `https://www.rfc-editor.org/rfc/rfc3290.html`
- `https://www.cs.ru.nl/~hooman/DES/liu-layland.pdf`
- `https://brandur.org/rate-limiting`
