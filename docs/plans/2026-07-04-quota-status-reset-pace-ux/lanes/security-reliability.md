# Lane: security-reliability

Status: answered
Reasoning effort: high
Security context: applicable
Candidate evidence label: `read-only observer + stale-visible display + redaction/telemetry guardrails`
Agent: `019f2aba-bca6-7963-812f-0467bfe25f92`

## Constraints Accepted

- `quota status` must remain read-only.
- `sessions` must remain read-only/query-only.
- No status path may refresh quota, prune stale leases, refresh rollups, migrate schema, or call provider endpoints.
- Stale data may display but must be labeled with confidence/sample age.
- New fields must not leak tokens, raw account IDs, emails, absolute paths, provider auth details, or free-form provider errors.
- Telemetry additions must be low-cardinality.
- Terminal color must not be the only semantic carrier.

## Key Evidence Anchors

- `crates/codex-router-cli/src/quota.rs:1271-1347`
- `crates/codex-router-cli/src/sessions.rs:299-310`
- `crates/codex-router-state/src/sqlite.rs:594-617`
- `crates/codex-router-state/src/sqlite.rs:1943-2065`
- `crates/codex-router-state/src/selection_projection.rs:222-317`
- `crates/codex-router-cli/src/lib.rs:3168-3185`
- `crates/codex-router-cli/src/lib.rs:3976-4040`

## Rollback Notes

- Prefer presentation/read-model slices first.
- If threshold changes cause regressions, revert stale-after/projection threshold changes separately from harmless copy/layout improvements.

## Parent Disposition

Accepted into security assumptions, risks, and split/replan triggers.

