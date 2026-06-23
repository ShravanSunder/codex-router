# Revision Lane: Validation Proof

Lane: `validation-proof`
Status: `answered`
Mode: read-only planning
Confidence: high on proof shape, medium on final row numbering

## Evidence Inspected

- Full spec and current plan files.
- Accepted review receipt.
- Live test inventory via `rg` and exact `cargo test -- --exact --list`
  probes.
- Smoke harness: `tests/smoke/installed_codex_mock.sh`
- Installed smoke tests in `crates/codex-router-test-support/src/installed_codex.rs`
- Proxy route/header/local-auth tests, core local auth tests, selection
  turn-state tests, CLI profile/status tests.

## Accepted Candidate Evidence

- Matrix rows must use full libtest paths:
  `cargo test -p <pkg> <full::path> -- --exact --list`
  and
  `cargo nextest run -p <pkg> -- <full::path> --exact`.
- Ignored installed-smoke execution should use:
  `cargo nextest run -p codex-router-test-support --run-ignored ignored-only -- <full::path> --exact`.
- Bundled route, local-auth, affinity, and smoke rows must split into one row
  per behavior.
- Existing exact tests can be named for profile approval, local auth, route
  classifier, responses body preservation, models ETag, process selector state,
  turn-state baseline, and both installed smoke tests.
- New implementation tests are required for audit append failure, selector
  durable input, memories trace protocol, responses compact protocol,
  WebSocket `x-models-etag`, unsupported-before-selection, replay-scope
  affinity, and wrapper smoke enumeration.

## Parent Synthesis

Folded into:

- Plan 1A rows `1A-00`, `1A-04a`, `1A-04b`, `1A-06a`, `1A-14a`,
  `1A-14b`, and `1A-15`.
- Plan 1B rows `1B-02a`, `1B-07a`, `1B-13a`, `1B-14a`, `1B-16*`,
  `1B-17*`, and `1B-23` through `1B-25`.

Completion receipt: answered; read-only; parent wrote this lane artifact.
