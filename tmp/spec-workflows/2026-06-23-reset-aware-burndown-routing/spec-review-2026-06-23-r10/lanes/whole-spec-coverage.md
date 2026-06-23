# R10 Lane: Whole Spec Coverage

Status: answered
Verdict: needs revision
Agent: Aristotle (`019ef55a-1509-7ad2-993d-93e2e42d147f`)

Coverage:

- `reset-aware-burndown-routing-spec.md` was 1294 lines before R10 fixes.
- Read chunks: 1-260, 261-520, 521-780, 781-1040, 1041-1294.

Candidate findings:

1. Important: `router_affinity_hash_secret` was defined in the affinity
   contract but not carried into the global security assets, forbidden emission
   surfaces, or redaction proof expectations.
2. Important: public `routing_reason` values were stable enums, but the spec did
   not define deterministic precedence when multiple preferred-account
   predicates were true.
3. Question: behavior for no-affinity/new owner creation when the affinity hash
   secret is missing or unreadable was not explicit.

Parent reducer result:

- Accepted all three as refinement inputs.
- Folded the secret into assets, redaction surfaces, forbidden emission surfaces,
  and proof rows.
- Added deterministic routing-reason precedence.
- Added `affinity_secret_unavailable` behavior for response-creating routes
  when the hash secret cannot be loaded or created.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-creation-swarm
