# Implementation Execute Plan Brief

Date: 2026-06-23
Workflow: `shravan-dev-workflow:implementation-execute-plan`
Plan:
`tmp/plan-workflows/2026-06-23-reset-aware-burndown-routing/implementation-plan.md`

## Coverage

- Implementation plan: 840 lines, loaded before execution.
- Focused plan review closure committed and pushed at `897df79`.
- Current execution starts at T0 because T0 is the serial gate for shared core
  primitives.

## T0 Core Contract Primitives

Plan rows:

- RP-09 previous-response affinity primitives and no raw key leakage.
- RP-12 shared safe labels for status/log/audit/smoke output.
- RouteBand support for `responses`, `responses_compact`, `models`, and
  `memories_trace_summarize`.

Files changed:

- `crates/codex-router-core/Cargo.toml`
- `crates/codex-router-core/src/lib.rs`
- `crates/codex-router-core/src/routes.rs`
- `crates/codex-router-core/src/affinity.rs`
- `crates/codex-router-core/src/redaction.rs`

Implemented:

- `RouteBand` enum with stable snake-case string, display, serde names.
- `PreviousResponseId`, `AffinityKeyHash`, `RouterAffinityHashSecret`, and
  `hash_previous_response_id`.
- `SafeAccountLabel`, `safe_account_label`, and unsafe-label predicate with
  deterministic `acct-<12 lowercase hex>` fallback from account id.

Proof:

- `cargo fmt --all -- --check` passed.
- `cargo test -p codex-router-core` passed: 15 tests.
- `cargo check --workspace` passed.

Notes:

- No downstream call sites were changed in T0.
- T1 will migrate selection to consume these core primitives.

