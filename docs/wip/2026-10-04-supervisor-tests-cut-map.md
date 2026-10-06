# External provider supervisor test cut map

This bounded mechanics-only slice is authorized by the owner continuation instruction and fixes pure-reorganization clearance. It preserves provider operation effects, typed failures, fixtures, test discovery and public supervisor APIs.

## Write set

- `crates/codex-router-host/src/external_provider_supervisor/tests.rs`
- new `crates/codex-router-host/src/external_provider_supervisor/failure_projection_tests.rs`

## Exact map

Before this cut, `tests.rs` had 1099 lines. Parent fixture imports/helpers and the first test group remain in the parent through old line 959. The final four typed-failure tests moved as complete items from old lines 960–1099:

- `lost_provider_prompt_has_terminal_unknown_effect_and_sanitized_reason`
- `unsupported_prompt_content_is_a_validation_failure_without_provider_effect`
- `unknown_agent_stop_reason_settles_with_typed_value`
- `provider_session_not_found_failure_has_typed_guidance_and_code`

The parent now has 960 lines and appends `#[path = "failure_projection_tests.rs"] mod failure_projection_tests;`. The child has 143 formatted lines, begins with `use super::*`, and retains all four test bodies and assertions. No fixture/helper was duplicated and no production supervisor source moved.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p codex-router-host external_provider_supervisor::tests::failure_projection_tests::`: exit 0; 4 passed, 0 failed, 140 filtered; package integration binaries had no matching filtered tests.
- Test discovery: all four child test names listed.
- `cargo check -p codex-router-host --locked`: exit 0, 22.21 seconds.
- `cargo clippy -p codex-router-host --all-targets --locked -- -D warnings`: exit 0.
- Parent fixture prefix and moved test-name inventory remain source-preserved; `git diff --check` passes.

No API, provider semantics, fixture process behavior, CI, Cargo, SQLx, migration, auth/security or production process changes. The full decomposition still has 25 oversized files after this cut; whole canonical plan, final CI, independent review and PR readiness remain incomplete.
