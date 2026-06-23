# Plan Revision Lane: Execution Order And Write Scope

Status: answered
Security context: applicable

Accepted into plan:

- Dirty-tree isolation is a gate before code work.
- Fresh worktree from the same branch tip is preferred.
- Dirty-worktree fallback requires path classification and hunk fingerprints for same-path overlaps.
- Plan 1A is serial: T1 -> T2 -> T3 -> A1 -> T4 -> T5 -> A2 -> validation/review.
- Plan 1B is serial: T6 -> B0 -> T7 -> T8 -> T9 -> T10 -> T11 -> B1 -> T12 -> review.
- Plan 1B cannot start before the Plan 1A completion receipt exists, even in a single PR stack.
- `crates/codex-router-proxy/src/websocket.rs` is in scope when WebSocket behavior is in scope.
- Manifests and `Cargo.lock` are closed unless a task-local amendment is approved.

Key evidence:

- Current dirty tree from `git status --short`.
- Existing Plan 1B early-start exception in the old umbrella and Plan 1A.
- Account import coupling in `crates/codex-router-cli/src/account.rs`.
- Serve/refresh/proxy/WebSocket coupling in `crates/codex-router-cli/src/lib.rs`, `crates/codex-router-cli/src/quota.rs`, `crates/codex-router-proxy/src/http_sse.rs`, and `crates/codex-router-proxy/src/websocket.rs`.

Confidence: high
