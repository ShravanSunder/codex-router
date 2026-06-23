# R15 Lane: Status, Refresh, And Transcript Safety

Agent: Boyle
Status: accepted by parent

## Finding

The spec needed proofable refresh persistence and transcript redaction rules
that match the desired product behavior: startup/request routing/status never
wait on live refresh, failed refresh preserves last-known quota, and proof
artifacts never leak raw request or first-frame values.

## Accepted Spec Change

- Add repository-level refresh operations:
  `record_refresh_success_and_replace_selector_windows(...)` and
  `record_refresh_failure_preserving_selector_windows(...)`.
- Failed refresh preserves selector rows and records only a redacted error
  class/staleness marker.
- Status uses last-known persisted data and renders unknown/no-data without
  fake `0% left`.
- Smoke transcripts may include route band, safe label/hash, reason enum, call
  counts, and first-frame shape/type, but must not include raw or derived
  non-allowlisted body fields such as `first_frame_model`,
  `first_frame_has_input`, or `first_frame_stream`.

## Evidence Anchors

- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
