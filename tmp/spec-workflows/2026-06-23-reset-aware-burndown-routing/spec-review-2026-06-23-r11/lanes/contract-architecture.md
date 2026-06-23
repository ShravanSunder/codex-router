# R11 Lane: Contract And Architecture

Status: answered
Verdict: needs revision
Agent: Confucius (`019ef563-02ef-7683-9e8a-75149cf6d9ca`)

Coverage:

- `reset-aware-burndown-routing-spec.md` was 1447 lines before R11 fixes.
- Read chunks: 1-300, 301-600, 601-900, 901-1200, 1201-1447.

Candidate findings:

1. Important: the spec moved previous-response affinity ownership to core but
   did not say what happens to existing `codex-router-selection::affinity` and
   raw `AffinityKey` surfaces.
2. Important: route-band policy was selection-owned, but the shared source of
   truth between proxy route classification and selection policy coverage was
   not specified.
3. Important: secret-store owned the affinity hash secret, but the concrete
   load/create API, stable key, entropy/encoding, and redacted error contract
   were missing.
4. Important: safe-label ownership was assigned, but detection predicates and
   output format were fuzzy.

Parent reducer result:

- Accepted all four.
- Added previous-response raw-key cutover language.
- Added core-owned `RouteBand` identity and drift-guard tests.
- Added secret-store API/key/encoding/redacted-error contract.
- Added `SafeAccountLabel` helper semantics, minimum unsafe predicates, and
  `acct-<12 lowercase hex chars>` tag format.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-creation-swarm
