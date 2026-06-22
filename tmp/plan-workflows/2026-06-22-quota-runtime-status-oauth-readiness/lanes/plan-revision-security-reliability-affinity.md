# Plan Revision Lane: Security Reliability Affinity

Status: answered
Security context: applicable

Accepted into plan:

- Plan 1B owns same-turn and previous-response affinity proof.
- Plan 1B owns local bearer-token lifecycle receipt.
- Plan 1A owns resolver bypass guard and structural search proof.
- Plan 1A owns audit JSONL allowlist proof.
- Plan 1A/1B explicitly define response-backed route-band invalidation and keep `code_review` status-only unless a later spec promotes it.
- Plan 1B owns cross-process quota-refresh one-writer behavior.

Key evidence:

- Spec routing, quota, secret storage, local auth, audit, proof, and security sections.
- Direct runtime secret reads in `crates/codex-router-cli/src/quota.rs` and `crates/codex-router-proxy/src/http_sse.rs`.
- Affinity substrate in `crates/codex-router-selection/src/turn_state.rs` and state repositories, but no proxy wiring yet.
- Audit sink in `crates/codex-router-core/src/audit.rs`.
- Local-token generation and WebSocket revocation tests in core/proxy/CLI.
- Per-route-band SQLite atomicity in `crates/codex-router-state/src/sqlite.rs`.

Confidence: medium-high
