# Plan 1A A1 Receipt: Fail-Closed Credential Gate

## Gate

Merge Gate A1 required before T4 starts:

- T1-T3 matrix rows `1A-00` through `1A-06a`, plus `1A-00h`, pass.
- Dirty-tree isolation proves only T1-T3 owned paths were staged.
- Checkpoint path proof lists only T1-T3 owned paths and receipts.
- No baseline-only hunk is present.

## Evidence

Exact-one preflight for all A1 matrix tests:

```text
1A-00  tests::profile_write_command_requires_approval_flag  count=1
1A-00a tests::account_import_codex_auth_writes_router_owned_state_and_secrets count=1
1A-00b tests::quota_status_reads_sqlite_rows_without_provider_io count=1
1A-00c tests::serve_command_starts_runtime_and_forwards_one_loopback_request count=1
1A-00d tests::profile_print_emits_router_custom_provider_without_home_mutation count=1
1A-00e tests::token_export_and_profile_doctor_redact_router_token_value count=1
1A-00f tests::profile_write_dry_run_previews_named_profile_without_mutation count=1
1A-00g tests::profile_write_approved_writes_only_named_temp_profile_file count=1
1A-01  tests::router_credentials_debug_redacts_secret_fields count=1
1A-02  tests::account_import_codex_auth_redacts_refresh_token_in_error_paths count=1
1A-03  tests::quota_refresh_rejects_non_provider_base_url_before_token_egress count=1
1A-04  tests::assembled_loopback_router_runtime_redacts_http_and_websocket_audit_events count=1
1A-04a tests::audit_append_failure_reports_through_audit_failure_reporter_without_secret_leak count=1
1A-05  tests::account_import_codex_auth_partial_secret_write_disables_account_until_repair count=1
1A-06  tests::account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation count=1
1A-06a tests::credential_mutation_invalidates_response_backed_alias_family_atomically count=1
```

Exact-test helper row `1A-00h`:

```text
real_count=1 missing_count=0
```

Structural audit row `1A-04b`:

```text
bash -lc '! rg -n -e "let _result = audit_sink\\.append" -e "let _ = audit_sink\\.append" -e "AuditFailureReporter.*let _" crates/codex-router-proxy/src/http_sse.rs crates/codex-router-proxy/src/websocket.rs crates/codex-router-proxy/src/server.rs'
exit 0
```

Fresh package gates:

```text
cargo nextest run -p codex-router-cli -p codex-router-state -p codex-router-secret-store
53 tests run: 53 passed, 0 skipped

cargo nextest run -p codex-router-auth -p codex-router-proxy
49 tests run: 49 passed, 0 skipped
```

Workspace gates:

```text
cargo fmt --all --check
exit 0

cargo nextest run --workspace
129 tests run: 129 passed, 2 skipped

cargo clippy --workspace --all-targets -- -D warnings
exit 0

git diff --check
exit 0

git status --short
<empty>
```

Checkpoint path proof:

```text
git show --name-only --oneline --no-renames 16723fc
16723fc feat: fail closed account credential activation
Cargo.lock
crates/codex-router-cli/src/account.rs
crates/codex-router-cli/src/lib.rs
crates/codex-router-secret-store/Cargo.toml
crates/codex-router-secret-store/src/account_tokens.rs
crates/codex-router-secret-store/src/model.rs
crates/codex-router-state/src/account.rs
crates/codex-router-state/src/lib.rs
crates/codex-router-state/src/quota_snapshot.rs
crates/codex-router-state/src/sqlite.rs
tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/implementation-t3-fail-closed-credential-receipt.md
```

## Decision

A1 is satisfied. T4 may start from `16723fc` plus this receipt checkpoint.

No live OAuth, live provider quota proof, or browser/account login flow was run.
