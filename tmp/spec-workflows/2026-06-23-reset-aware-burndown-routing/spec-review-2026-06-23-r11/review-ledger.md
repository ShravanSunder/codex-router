# R11 Spec Review Ledger

Date: 2026-06-23
Status: needs revision; accepted findings folded into spec in this checkpoint

## Source

- Baseline commit: `66dfe14 docs: resolve r10 quota spec findings`
- Review worktree: `/tmp/codex-router-r11-review.8XkrYg`
- Target spec:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- Coverage before R11 fixes: 1447 lines, read in chunks 1-300, 301-600,
  601-900, 901-1200, and 1201-1447.

## Lanes

| Lane | Agent | Verdict | Parent result |
| --- | --- | --- | --- |
| whole-spec-coverage | Anscombe | needs revision | accepted |
| contract-architecture | Confucius | needs revision | accepted |
| security-validation | Cicero | needs revision | accepted |
| planning-harness-disclosure | Curie | ready | accepted ready |

## Accepted Findings

1. Add a public routing reason for weekly reset imminence driven by
   long-window salvage.
2. Forbid individual non-allowlisted WebSocket first-frame/body field values in
   persisted or shared smoke transcripts.
3. Define the previous-response raw-key cutover away from
   `codex-router-selection::affinity` and raw `AffinityKey`.
4. Define a shared core `RouteBand` identity and drift guard between proxy route
   classification and selection policy lookup.
5. Define the secret-store affinity hash-secret API, stable key, entropy,
   persisted encoding, typed return, and redacted error contract.
6. Define the `SafeAccountLabel` helper contract, unsafe predicates, and
   deterministic redacted tag format.

## Revision Applied

The spec now defines:

- `preferred_weekly_reset_soon` enum, human phrase, JSON enum, precedence, and
  Scenario B status proof
- smoke transcript first-frame/body field allowlist and canary proof
- previous-response raw-key cutover tests
- core-owned `RouteBand` and policy drift tests
- `load_or_create_router_affinity_hash_secret(...)`,
  `router_affinity_hash_secret.v1`, 32-byte entropy, 64-lowercase-hex encoding,
  typed core return, and redacted errors
- `SafeAccountLabel` input/output, unsafe predicate minimums, and
  `acct-<12 lowercase hex chars>` tag format

## Parent Verdict

R11 did not pass the hard gate. Accepted findings were folded into the spec.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-review-swarm
recommended_transition_reason: R11 accepted findings have been folded into the spec; run another adversarial spec review before any plan creation.
