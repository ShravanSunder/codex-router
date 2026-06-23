# Plan Revision Lane: Validation Proof Exactness

Status: answered
Security context: applicable

Accepted into plan:

- Matrix columns now include proof owner, preflight list command, execution command, expected observation, stale-proof guard, and red/green.
- Proof owner is task plus crate/module, not a person.
- Preflight standard is `cargo test -p <package> <exact_test_name> -- --exact --list` for ordinary tests and the same command with `--ignored` for installed-smoke tests.
- Missing tests are named as implementation deliverables instead of hidden behind broad filters.
- Installed-smoke proof must replace prefix-only dispatch with explicit scenario enumeration.

Key evidence:

- Spec proof requirements: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- Current smoke wrapper prefix dispatch: `tests/smoke/installed_codex_mock.sh`
- Current installed smoke inventory: `crates/codex-router-test-support/src/installed_codex.rs`
- Current test listing evidence from `cargo nextest list -p codex-router-cli --message-format oneline`, `codex-router-proxy`, `codex-router-auth`, and `codex-router-selection`.

Confidence: high
