# R15 Lane: Adversarial, Security, Harness, And Spec Difference

Agent: Singer
Verdict: needs revision

Accepted findings:

- WebSocket request-body token carrier rule contradicts the non-allowlisted
  first-frame parsing rule.
- Current-state WebSocket evidence is wrong; current code already does path
  preflight and bounded first-frame validation.
- Mixed-carrier auth mismatch requires preserving both accepted carriers through
  preflight/post-upgrade validation.
- Tail workflow wording uses `phase_result: complete`, but the review skill
  exposes parent-verified verdict readiness.

Completion receipt: answered with full spec coverage, R15 lane review, and live
implementation anchor inspection.
