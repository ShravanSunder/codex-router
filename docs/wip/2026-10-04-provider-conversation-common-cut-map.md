# Provider conversation common-operation test cut map

Bounded mechanics-only integration-test split under the owner continuation instruction. The cut keeps the compiled CLI provider-operation assertions and their shared Unix-control fixtures unchanged while moving the first common-operation scenarios into a private child module.

## Write set

- `crates/agent-collaboration/tests/provider_conversation_cli.rs`
- new `crates/agent-collaboration/tests/provider_conversation_cli/common_operation_tests.rs`

## Exact map

Before the cut, `provider_conversation_cli.rs` had 1318 physical lines. Keep the shared constants and the remaining provider/Codex/unavailable/ignored-Cursor scenarios in the parent. Move the complete first six tests (old lines 18–480) into the private child, before `codex_create_missing_model_and_effort_fails_before_start_record`. The parent is 857 lines after the module declaration; the child is 465 lines and uses `use super::*` for shared constants, helpers, and subprocess fixtures declared in the parent.

Moved scenarios:

- `common_provider_cancel_allocates_and_prints_omitted_operation_id`
- `common_new_prompt_composes_provider_create_then_prompt`
- `common_load_settles_and_cancel_names_exact_operation`
- `common_create_waits_for_provider_target_and_prints_operation_first`
- `provider_settings_set_and_accept_use_immediate_control_methods`
- `provider_session_inspect_shows_capabilities_and_last_settings`

No provider CLI protocol/API, operation identity, fixture transport, persistence, auth/security, CI, or production behavior changes. The parent remains the Cargo integration-test target; only private module wiring and test ownership changed.

## Proof

- `cargo test -p agent-collaboration --test provider_conversation_cli`: exit 0; 11 passed, 0 failed, 1 ignored (the pre-existing authenticated Cursor test).
- `cargo fmt --all -- --check`: exit 0 after removing the formatter-only trailing blank line.
- `cargo check -p agent-collaboration --locked`: exit 0.
- `cargo clippy -p agent-collaboration --all-targets --locked -- -D warnings`: exit 0.
- Parent prefix/constants and remaining body comparison against `git show HEAD:crates/agent-collaboration/tests/provider_conversation_cli.rs`: exit 0; unchanged source retained except module declaration, with the moved six-test body preserved (the old terminal blank line is formatter-normalized).
- Moved test-name inventory: all six names preserved exactly once in the child and once in the pre-cut parent.
- Strict size checker: exit 1 with 14 remaining oversized Rust files; `provider_conversation_cli.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the two Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
