# Lane: codebase-explorer

Status: candidate evidence, parent-verified in `swarm-ledger.md`

Core finding:

- Runtime selection already has a repository boundary for selector quota input.
- The current adapter collapses per-window state to minimum remaining headroom.
- Reset time exists in persisted selector windows but is not used by runtime selection.

Accepted source anchors:

- `crates/codex-router-proxy/src/account_selection.rs:189-210`
- `crates/codex-router-proxy/src/account_selection.rs:262-292`
- `crates/codex-router-state/src/quota_snapshot.rs:91-200`
- `crates/codex-router-state/src/repositories.rs:46-59`

Design implication:

The spec should define burn-down assessment against `SelectorQuotaInput` and `PersistedSelectorQuotaWindow`, not raw provider responses or CLI rows.
