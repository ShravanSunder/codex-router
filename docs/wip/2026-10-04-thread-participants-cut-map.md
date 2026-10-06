# Thread participant integration-test cut map

Bounded test-only pure reorganization under the owner continuation instruction. Existing BoardStore SQLite fixture lifecycle, participant roles, subscription/watcher semantics and corruption assertions remain unchanged.

## Write set

- `crates/message-board-storage/tests/thread_participants.rs`
- new `crates/message-board-storage/tests/thread_participants/subscription_tests.rs`
- new `crates/message-board-storage/tests/thread_participants/corruption_tests.rs`

## Exact map

Before this cut, the integration target had 1442 lines. Keep shared identity/page/Fixture helpers and the first five participant lifecycle tests through old line 644. Move old lines 645–1062 into `subscription_tests.rs`: four subscription/join/handoff/concurrency tests. Move old lines 1063–1442 into `corruption_tests.rs`: four cursor/corruption/archived/unread tests. Parent is647 lines; children are420 and383 formatted lines. Both child modules use `use super::*` to retain the real temporary BoardStore fixture and helpers. No test renamed/removed; all13 target tests remain.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p message-board-storage --test thread_participants -- --list`: exit 0; child names discovered.
- Full integration target: exit 0; 13 passed, 0 failed, 0 filtered, 0.76 seconds.
- `cargo check -p message-board-storage --locked`: exit 0, 5.01 seconds.
- `cargo clippy -p message-board-storage --all-targets --locked -- -D warnings`: exit 0, 8.81 seconds.
- Strict size checker: exit 1 with 18 remaining oversized files; thread_participants.rs is no longer listed.

No BoardStore API, SQLite schema/SQLx/migration, participant/seat/SessionRef semantics, CI or production changes.
