# Validation And Security Lane

Lane: testability-validation + security-reliability
Backend: Codex subagent
Initial verdict: needs revision

## Accepted Findings

- WebSocket proof did not yet prove no-probe, allowlist, and call-order
  contracts.
- Status UX had no real CLI smoke gate.
- Installed Codex e2e did not force reset-aware choice or connection-lifetime
  pinning.
- `probe_required` persistence needed failure and partial-data proof.
- Validation commands omitted exact smoke commands and relied on cargo tests
  that do not run ignored smoke coverage by default.

## Parent Resolution

The parent expanded T3/T5/T6 proof, added `tests/smoke/quota_status_fixture.sh`
as planned T4 smoke, kept `tests/smoke/installed_codex_mock.sh` in validation,
and required table/plain/json status output evidence plus WebSocket multi-turn
pinning.

Completion receipt: answered
Confidence: high
