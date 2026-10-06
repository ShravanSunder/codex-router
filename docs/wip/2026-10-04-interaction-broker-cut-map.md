# Interaction broker test cut map

Bounded mechanics-only test split under the owner continuation instruction. Shared broker semantics, actor identity, approval/question history, cursor choices and the turn-cancellation sibling remain unchanged.

## Write set

- `crates/collaboration-service/src/interaction_broker_tests.rs`
- new `crates/collaboration-service/src/interaction_broker_tests/decision_history_tests.rs`
- new `crates/collaboration-service/src/interaction_broker_tests/question_lifecycle_tests.rs`

## Exact map

Before the cut, the parent test module had 2074 lines. Shared imports, fixtures, `pub(super) session`, `pub(super) fixture_broker`, `insert_pending`, and earlier tests remain in the parent through old line 899. The later scenarios move as complete groups:

- old lines 900–1512 to `decision_history_tests.rs`: 13 decision/history/typed approval tests (including the first decision test at old line 900).
- old lines 1513–2074 to `question_lifecycle_tests.rs`: 7 cursor/question/restart tests.

The parent now has 902 lines; children are 616 and 564 formatted lines. Each child uses `use super::*`; existing sibling `interaction_broker/turn_cancellation_tests.rs` still resolves `super::tests::session` and `super::tests::fixture_broker` from the parent.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p collaboration-service --lib interaction_broker::tests::`: exit 0; 27 passed, 0 failed, 190 filtered.
- `cargo check -p collaboration-service --locked`: exit 0.
- `cargo clippy -p collaboration-service --all-targets --locked -- -D warnings`: exit 0, 9.61 seconds.
- Strict size checker: exit 1 with 19 remaining oversized files; `interaction_broker_tests.rs` is no longer listed.

No approval/question wire/storage semantics, public API, SQLx/migrations, CI or production process changes. Sibling turn-cancellation coverage remains attached to the parent fixture helpers.
