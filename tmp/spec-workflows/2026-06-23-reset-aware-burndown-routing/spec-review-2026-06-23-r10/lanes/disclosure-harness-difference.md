# R10 Lane: Disclosure, Harness Fit, And Spec Difference

Status: answered
Verdict: needs revision
Agent: Mill (`019ef55a-2ab6-7df3-ae3a-b58a023e434b`)

Coverage:

- `reset-aware-burndown-routing-spec.md` was 1294 lines before R10 fixes.
- Read chunks: 1-260, 261-520, 521-780, 781-1040, 1041-1294.

Candidate finding:

1. Important: the installed-Codex e2e harness was required, but the spec did
   not pin the generated profile's local-auth header contract, leaving the
   current `env_key` versus intended `env_http_headers` behavior ambiguous.

Parent reducer result:

- Accepted.
- Added generated-profile proof requiring
  `env_http_headers = { "X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN" }`.
- Explicitly rejected `env_key = "CODEX_ROUTER_TOKEN"` and Authorization-bearer
  fallback as the generated profile contract for this goal.
- Required e2e proof that local auth reaches the router, is stripped before
  upstream HTTP/SSE and WebSocket opens, and is not printed.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-creation-swarm
