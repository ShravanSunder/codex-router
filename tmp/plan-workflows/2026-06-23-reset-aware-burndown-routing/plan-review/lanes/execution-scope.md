# Plan Review Lane: Execution Scope

Verdict: `needs revision`

## Accepted Findings

### Blocker: T8 was split in the DAG but not in the task list

- Problem: the DAG depended on harness scaffolding before route-native proof,
  but the task list had only one broad T8.
- Required edit: split into T8a harness contract and T8b route-native
  black-box proof.
- Folded into plan: T8a, T8b, Execution DAG.

### Blocker: parallel rules allowed overlapping ownership

- Problem: T3/T5/T6 shared proxy/auth files, and T8/T9/T10 shared transcript
  surfaces.
- Required edit: serialize T3 -> T5 -> T6; freeze T8a before T8b/T9/T10; keep
  T9/T10 transcript artifacts disjoint.
- Folded into plan: Execution DAG, Parallel Work Rules.

### Blocker: T2 mixed three risky migrations

- Problem: quota refresh status, affinity owner storage, and affinity secret
  lifecycle were one checkpoint with different failure modes.
- Required edit: split into T2a/T2b/T2c.
- Folded into plan: T2a, T2b, T2c.

### Important: conditional write scopes were too loose

- Problem: T7 and T11 could expand scope under validation pressure.
- Required edit: add explicit allowlists and stop/replan gates.
- Folded into plan: T7, T11.

