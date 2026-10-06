# Control connection exact cut map

This is a bounded mechanics-only slice at HEAD `96f57779`, not a ready whole-work plan. Fixes cleared pure reorganization of all oversized files; this owner has no active reservation. The MCP test reservation is unrelated and remains released.

## Write set

- `crates/collaboration-service/src/control_connection.rs`
- new `crates/collaboration-service/src/control_connection/admission_error_tests.rs`

No Control API changes, schema/serde changes, SQLx/migration changes, dispatch behavior, wake semantics, public reexports, or production process actions.

## Exact source map

Before this cut, `control_connection.rs` had 1031 lines. The production handler and helpers occupy old lines1–899; the final `initialize` helper closes at old line899/parent line901 after the new declaration block. Existing `wake_creation_crash_tests.rs` remains declared at parent lines9–11.

| Old source | Action |
| ---: | --- |
| 1–8 | keep imports and framing dependencies unchanged |
| 9–11 | keep existing `wake_creation_crash_tests` path child unchanged |
| new 12–14 | add private `#[cfg(test)] #[path = "control_connection/admission_error_tests.rs"] mod admission_error_tests;` |
| 12–899 old production | keep `Request`, `serve_control_connection`, dispatch, error/admission helpers and `initialize` unchanged |
| 900–1031 | remove inline `#[cfg(test)] mod admission_error_tests` wrapper and move its complete body to child |

Child contains the three existing tests and `use super::*`, with no renamed scenarios or changed assertions:

- `saturated_methods_preserve_their_published_error_contracts`
- `provider_methods_report_overload_before_target_resolution`
- `saturated_message_admission_stops_before_any_route_dispatch`

The child preserves `ControlAdmission`, `Value`, `json!`, admission helper visibility and all exact overload/error-contract assertions through the parent-child private-module boundary.

## Proof route

- `cargo fmt --all -- --check`.
- `cargo test -p collaboration-service control_connection::admission_error_tests:: -- --list` must discover all three tests.
- `cargo test -p collaboration-service control_connection::admission_error_tests::` must pass all three, including published error-schema and no-dispatch assertions.
- `cargo check -p collaboration-service --locked` and `cargo clippy -p collaboration-service --all-targets --locked -- -D warnings`.
- Compare old production prefix and child body against baseline after formatting; verify only the path declaration, child move and no other source paths changed.
- Re-run strict size checker; this source must disappear from violations while remaining oversized sources stay explicit.

Stop if the child needs a public visibility change, any dispatch/JSON/schema adjustment, or a collision with a renewed owner reservation.

## Applied result

Applied this map without changing the public handler or existing wake test child. Parent production source prefix remains byte-identical; the new private admission child contains the three existing tests and helpers. The parent is 902 lines and the child is 129 lines after formatting.

Validation completed after the final declaration-placement correction:

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p collaboration-service control_connection::admission_error_tests:: -- --list`: exit 0; all 3 names discovered.
- `cargo test -p collaboration-service control_connection::admission_error_tests::`: exit 0; 3 passed, 0 failed, 214 filtered, with all integration binaries reporting zero filtered matches.
- `cargo check -p collaboration-service --locked`: exit 0, 5.04 seconds.
- `cargo clippy -p collaboration-service --all-targets --locked -- -D warnings`: exit 0, 11.62 seconds.
- Strict size checker: exit 1 with 26 remaining oversized files; control_connection is no longer listed.

No public API, Control JSON/schema, dispatch, wake behavior, SQLx, migration, CI or production process changed.
