# Compact storyboard validation

The [HTML storyboard](storyboard.html) contains 93 actual compiled TUI screenshots at 120×36, 76×30 and 48×24. Its synthetic typed inventories exercise the real input and rendering paths. The images were captured with version 0.1.95; they are retained screenshots, not a fresh version-0.1.96 capture or live account/native-launch proof.

## Current version-0.1.96 checks

The centered weekly-floor editor moved its increment arrow from the old test coordinate. The old compiled PTY test reproduced the CI timeout. The full composed dialog exposed the arrow at zero-based column 32, row 12. Correcting only the test coordinate made the compiled journey pass; a permanent composition test pins the visible geometry, draft increment and Cancel without persistence. The editor behavior and semantic wait bounds are unchanged.

Machine/provider labels now respect terminal display columns, including wide CJK names and a zero-column budget. A permanent regression failed against the character-count implementation before the existing helper was corrected. ASCII compact-header, active-search and record-visibility checks remain green.

| Check | Result |
| --- | --- |
| Session library | 335 passed, 0 failed, 3 ignored |
| CLI library, including composed floor dialog | 461 passed, 0 failed, 4 ignored |
| Shared dialog library | 5 passed, 0 failed, 1 explicit benchmark ignored |
| Compiled terminal integration, final source | 17 passed, 0 failed, 0 ignored |
| Explicit performance benchmarks | 3 passed, 0 failed |
| Strict workspace/all-target Clippy | exit 0 |
| Formatting, whitespace, Rust file size | pass; 1,895 Rust files |

Commands used locked, offline dependencies, a private build target and one compiler job. The compiled terminal suite uses sealed loopback provider fixtures; it makes no real vendor model calls.

```sh
cargo test -p agent-collaboration -p codex-router-cli -p codex-router-tui --lib --locked --offline -j 1
cargo test -p codex-router-quota-reset-test-harness --test quota_reset_pty_test --locked --offline -j 1
cargo test -p agent-collaboration -p codex-router-cli -p codex-router-tui --lib performance_measurements --locked --offline -j 1 -- --ignored --nocapture
cargo clippy --workspace --all-targets --locked --offline -j 1 -- -D warnings
cargo fmt --all -- --check
python3 scripts/tooling/check-rust-file-size.py
```

## Measured performance

Debug-build worst p95 across the measured viewport/state combinations; these are observations, not timing-based correctness assertions or a before/after regression comparison. Component pipelines include input, state and rendering; dialog-only measurements render its composed children.

| Surface | 10 entries | 1,000 entries |
| --- | --- | --- |
| Sessions component pipeline | 66.2 ms | 102.0 ms |
| Quota component pipeline | 30.7 ms | 44.2 ms |
| Shared dialog render | 1.6 ms | 80.3 ms |

The shared dialog lays out all supplied children; the 1,000-row stress case is materially slower than ordinary dialog content. Sessions retain their existing bounded visible-row window.

## Delivery boundaries

Current-head hosted CI and independent review remain separate gates. Configured-machine browsing does not establish configured native NEW/resume/fork: those actions remain guarded pending the existing source-bound native attachment and permitted NEW-policy contract. Nothing here authorizes production replacement, merge or release.
