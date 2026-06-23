# Revision Lane: Execution Order

Lane: `execution-order`
Status: `answered`
Mode: read-only planning
Confidence: high

## Accepted Candidate Evidence

Execution should be a hard serial chain:

```text
gate 0a: source freeze + dirty-tree inventory
gate 0b: fresh-worktree/tmp carry-forward receipt
  |
Plan 1A only
  |
T1 -> T2 -> T3 -> A1 -> T4 -> T5 -> A2
  |
Plan 1A validation + implementation review
  |
Plan 1B only
  |
T6 -> B0 -> T7 -> T8 -> T9 -> T10 -> T11 -> B1 -> T12
  |
Plan 1B final validation + implementation review
```

No task fan-out by default. Parallel implementation requires explicit replan
because accepted findings tie auth, credential writes, SQLite visibility,
alias-family publication, one-writer leases, local auth, and smoke proof
together.

Fresh-worktree execution must carry forward exactly:

- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/`
- `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/`

The carry-forward receipt must include source path, target path, source
commit/head, checksum or byte count, and `git status --short` before/after.

## Parent Synthesis

Folded into:

- Umbrella success rule, execution DAG, parallelism, and validation rule.
- Plan 1A Gate 0, A1, and A2.
- Plan 1B Gate 0, B0, B1, and final closeout.

Completion receipt: answered; read-only; parent wrote this lane artifact.
