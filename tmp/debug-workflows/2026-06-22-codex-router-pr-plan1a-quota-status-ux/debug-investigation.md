# Quota status UX bug

Symptom: live quota refresh persisted nonzero selector windows, but quota status showed an internal route/window dump that made the product look broken.

Evidence:
- /tmp/codex-router-live-test/state.sqlite selector_quota_windows contains nonzero 5h and weekly rows for responses/models after refresh.
- quota status --all-limits renders effective duplicates, account_id, route_band, source, stale, and raw unix reset values.

Root cause: quota status renders persistence/debug rows directly instead of a user-facing account quota summary.

Fix direction: render at most responses 5h + weekly rows per account; hide account_id/source/stale; humanize reset and runout.

Implemented fix:
- `quota status` now renders the user-facing `responses` quota only.
- Each account shows at most two rows: `5h` and `weekly`.
- Hidden from the normal surface: account_id, route_band, source, stale, effective duplicates, raw Unix reset timestamps.
- Humanized: quota percent, relative reset time, pace, and projected runout.

Live evidence after `quota refresh`:
- askluna: 5h 100%, weekly 0%.
- matches: 5h 68%, weekly 81%.
- ssdev: 5h 100%, weekly 16%.

Proof:
- `cargo test -p codex-router-cli quota_status -- --nocapture`: 3 passed.
- `cargo clippy -p codex-router-cli --all-targets -- -D warnings`: passed.
- `cargo nextest run -p codex-router-cli`: 53 passed.
