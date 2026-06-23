# Reset-Aware Burn-Down Routing Plan Review Ledger

Date: 2026-06-23
Review workflow: `shravan-dev-workflow:plan-review-swarm`
Reviewed plan:
`tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/implementation-plan.md`
Source spec:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`

## Coverage

- Plan line count before revision: 584 lines.
- Source spec line count: 1971 lines.
- Review lanes completed:
  - whole-plan-cohesion
  - spec-compliance
  - testability-validation
  - execution-scope
  - security-reliability

## Verdict

`needs revision`

The spec was clear enough to plan from, but the first implementation plan did
not fully trace several high-risk requirements into owning tasks and proof
gates. Implementation must not begin from the pre-review plan.

## Accepted Blockers

- Previous-response owner-record writes were unowned.
- HTTP/SSE affinity-secret fail-closed ordering was missing.
- WebSocket local-auth proof missed manual header, mixed-carrier mismatch, and
  subprotocol token smuggling.
- T8 route-native harness was split in the DAG but not in task scope.
- Installed-Codex HTTP/SSE and WebSocket e2e proof was not transport-isolated.
- T2 mixed three different state/secret risks into one proof gate.

## Accepted Important Findings

- Status output proof did not force the account-centric UX contract.
- T5/T6 parallelism was unsafe because both touch shared auth/proxy surfaces.
- T7/T11 write scopes were too loose.
- Secret-loss/replacement recovery was only a note.
- Final validation omitted `cargo deny check`, `cargo audit`, and broader
  redaction artifact scanning.

## Revision Applied

Revision 1 updates the plan to:

- split T2 into T2a/T2b/T2c,
- add HTTP/SSE and WebSocket owner-record write tasks,
- add HTTP/SSE `affinity_secret_unavailable` fail-closed proof,
- make local auth a shared cross-transport contract,
- add WebSocket manual header, mixed-carrier, and subprotocol rejection proof,
- split T8 into harness scaffolding and route-native black-box proof,
- add exact transport-specific installed-Codex proof commands,
- tighten status output snapshots and wording constraints,
- add explicit T7/T11 write allowlists and stop/replan gates,
- add final supply-chain and redaction proof.

## Next Gate

Run a second focused `plan-review-swarm` pass before code implementation. The
second pass should verify that all accepted findings above are actually covered
by task ownership, DAG order, and proof commands.

