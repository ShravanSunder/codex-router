Verdict
ready_with_fixes

Reason:
All accepted blocker/important findings that affected the current account/quota/background-refresh slice were fixed and re-verified. Remaining items are follow-ups because they require broader crate-boundary or filesystem-hardening work rather than a narrow patch to this implementation slice.

Accepted Findings Fixed

1. [blocker] Quota refresh did not populate route bands consumed by the proxy.
   Evidence: proxy selects `models`, `memories_trace_summarize`, and `responses_compact` separately from `responses`; refresh originally wrote only `responses` and `code_review`.
   Fix: response quota now fans out to `responses`, `models`, `memories_trace_summarize`, and `responses_compact` on success and failure.
   Proof: `quota_refresh_populates_models_route_band_used_by_serve` runs `quota refresh`, verifies the `models` snapshot, then serves `GET /v1/models` successfully.

2. [important] `quota status --format plain` was ambiguous for values containing spaces.
   Evidence: plain mode wrote raw `key=value` fields while account labels and notes could contain spaces.
   Fix: plain values are percent-encoded; table account labels are sanitized.
   Proof: `quota_status_plain_percent_encodes_space_containing_values`.

3. [important] Missing `code_review_rate_limit` left stale code-review state.
   Evidence: missing response quota failed closed; missing code-review quota was skipped.
   Fix: missing code-review quota writes a failed zero-headroom replacement route.
   Proof: `quota_refresh_missing_code_review_replaces_old_snapshot_with_failed_zero`.

4. [important] Serve-owned background refresh silently accepted invalid refresh endpoint config.
   Evidence: worker discarded refresh errors and invalid endpoint config was not checked before serving.
   Fix: `serve` validates quota refresh endpoint policy before listening; background cycle failures emit a redacted stderr line.
   Proof: `serve_command_rejects_disallowed_quota_refresh_base_url_before_listening`.

5. [important] Email/PII and crafted labels could be persisted and rendered unsafely.
   Evidence: labels were only checked for dotted email-like strings and were printed raw.
   Fix: import now accepts only local-safe labels (`A-Z`, `a-z`, `0-9`, `.`, `_`, `-`); account and quota display paths sanitize labels.
   Proof: `account_import_codex_auth_rejects_email_like_label` includes email-like, space, and newline cases.

6. [important] Duplicate-label imports silently overwrote credentials.
   Evidence: account id was derived from label and secrets were namespaced by account id.
   Fix: import rejects an existing label/account id instead of replacing secrets.
   Proof: `account_import_codex_auth_rejects_duplicate_label_without_overwriting_secrets`.

7. [important] Live quota table mode leaked profile directory names.
   Evidence: live quota used profile directory names as display labels.
   Fix: live quota display labels are stable aliases like `profile-1`.
   Proof: live quota tests now assert `profile-1` output and no raw profile label.

Follow-Ups Not Fixed In This Slice

1. [important] Per-account observed timestamps during long quota sweeps.
   Rationale: current refresh config carries a single observation timestamp for deterministic CLI/test runs. Fixing this cleanly needs a refresh clock contract that preserves deterministic tests while stamping real provider fetch completion per account.

2. [important] Background refresh shutdown cancellation during in-flight sweeps.
   Rationale: worker shutdown currently joins the in-flight blocking sweep. Bounding shutdown independent of account count needs a cancellation-aware refresh service contract, not only a CLI thread wrapper tweak.

3. [follow-up] Move refresh scheduling/normalization out of the CLI crate.
   Rationale: reviewers correctly noted the intended crate boundary is `codex-router-quota`. Moving this now would be a broad refactor after the behavior patch; it should be its own plan-backed cleanup.

4. [follow-up] Race-free filesystem nofollow/dirfd hardening.
   Rationale: existing path validation rejects `.codex`, `.prodex`, and symlink paths before open, but TOCTOU-resistant nofollow/dirfd opening is a deeper filesystem-hardening task across state and secret-store backends.

Review Proof

- Spec compliance reviewed: route-band mismatch accepted and fixed.
- Implementation proof reviewed: new regression coverage added for each accepted behavior bug.
- Security reviewed: no serving fallback to `auth.json`; no provider-token egress to non-provider quota endpoint without explicit test flag. Label/profile output issues accepted and fixed.
- Proof gaps remaining: live OAuth/quota proof is still approval-gated and not run for this changed revision.

Swarm Coverage

- Spec/proof lane: blocker route-band mismatch; follow-up slow in-flight background proof.
- Security/trust-boundary lane: validated label output injection and live profile label leakage; TOCTOU follow-up.
- Reliability lane: alias failure fan-out, timestamp-per-account, worker error visibility, shutdown cancellation.
- Contracts/tests lane: plain output ambiguity, missing code-review replacement, weak label validation, worker failure visibility.
- Adversarial/code-quality lane: route-band blocker, duplicate-label overwrite, worker visibility, crate-boundary follow-up.

Validation After Fixes

- `cargo fmt --all -- --check`: passed.
- `cargo test -p codex-router-state`: 14 passed.
- `cargo test -p codex-router-secret-store`: 10 passed.
- `cargo test -p codex-router-auth`: 17 passed.
- `cargo test -p codex-router-cli`: 47 passed.
- `cargo test -p codex-router-proxy`: 40 passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo nextest run --workspace`: 155 passed, 2 skipped.
- `cargo deny check`: passed; duplicate warnings only for `getrandom` and `windows-sys`.
- `cargo audit`: passed; 199 dependencies scanned.
- `actionlint .github/workflows/ci.yml`: passed.
- `tests/smoke/installed_codex_mock.sh`: 2 passed.

Artifact Links

- Shared packet: /Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/plan-workflows/2026-06-21-codex-router-feature-initial-codex-router-quota-output-account-onboarding/implementation-review-packet.md
- This report: /Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/plan-workflows/2026-06-21-codex-router-feature-initial-codex-router-quota-output-account-onboarding/implementation-review-report.md
