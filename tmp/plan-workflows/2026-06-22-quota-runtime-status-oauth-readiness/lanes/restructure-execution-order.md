# Restructure Lane: Execution Order

Lane: `execution-order`
Agent: Cicero (`019eeee0-61b5-77e1-a331-b3933d38746b`)
Status: answered
Evidence label: `candidate-execution-order-dag-v1`
Security context: applicable
Confidence: medium

## Candidate Evidence

Use a mostly serial two-phase DAG:

```text
T1 -> {T2 || T3} -> T4 -> T5 -> Plan 1A gate
Plan 1A gate -> T6 -> T7 -> {T8 -> T9 || T10 -> T11} -> T12
```

Do not parallelize:

- T3 with T4.
- T4 with T5.
- T6 with T7.
- T8 with T9.
- T10 with T11.
- T12 with any writing lane.
- Plan 1 work with Plan 2 login/device-code work.

## Accepted Parent Changes

- Umbrella DAG now shows merge gate A1, merge gate A2, merge gate B0, and merge gate B1.
- Umbrella parallelism rules now name the only two safe fan-out points.
- Plan 1B still requires T6 before T7.
- Plan 1B owns final validation/smoke/review closeout after all writing lanes rejoin.

## Anchors

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:110-118`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:151-153`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:205-223`
- `crates/codex-router-cli/src/quota.rs:304-444`
- `crates/codex-router-cli/src/lib.rs:73-115`
- `crates/codex-router-cli/src/lib.rs:333-390`
- `crates/codex-router-proxy/src/http_sse.rs:552-825`

## Completion Receipt

Status: answered.
Parent wrote this lane artifact.
Remaining uncertainty: T5 might reuse existing `quota_status_rows` or require a schema bump; the order does not change.
