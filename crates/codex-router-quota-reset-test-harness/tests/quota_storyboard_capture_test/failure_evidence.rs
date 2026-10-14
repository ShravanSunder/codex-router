//! Safe evidence retained before a failed capture's synthetic fixture is removed.
use super::{QuotaResetFixture, TestResult, encode_json_string, ensure};
use crate::loopback_provider_test::SafeRequestRecord;
use std::{fs, path::Path};

pub(super) async fn write_failure_evidence(
    output: &Path,
    fixture: &QuotaResetFixture,
    requests: &[SafeRequestRecord],
) -> TestResult<()> {
    let records = requests
        .iter()
        .map(|request| {
            format!(
                "{{\"method\":{},\"path\":{},\"response_status\":{}}}",
                encode_json_string(&request.method),
                encode_json_string(&request.path),
                request.response_status,
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        output.join("failed-provider-ledger.json"),
        format!("[{records}]\n"),
    )?;
    let completion_diagnostic = fixture.root().join("quota-completion-diagnostic.log");
    if completion_diagnostic.exists() {
        fs::copy(
            completion_diagnostic,
            output.join("failed-completion-diagnostic.log"),
        )?;
    }
    let state = codex_router_state::sqlite::AsyncSqliteStateStore::open_read_only(
        &fixture.root().join("state.sqlite"),
    )
    .await?;
    let statuses = state
        .quota_refresh_statuses_for_route_band("responses")
        .await;
    let close_result = state.close().await;
    let statuses = statuses?;
    close_result?;
    // Only observation presence and the closed redacted error class leave the
    // fixture. No account identity, credential, raw error or database is copied.
    let records = statuses
        .iter()
        .map(|status| {
            let error_class = status.last_error_class().map_or_else(
                || "null".to_owned(),
                |class| encode_json_string(class.as_str()),
            );
            format!(
                "{{\"attempted\":{},\"has_success_status\":{},\"last_error_class\":{error_class}}}",
                status.last_attempt_unix_seconds().is_some(),
                status.last_success_unix_seconds().is_some(),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        output.join("failed-refresh-status.json"),
        format!("[{records}]\n"),
    )?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn failed_capture_retains_refresh_class_without_account_or_secret_data() -> TestResult<()> {
    use codex_router_core::ids::AccountId;
    use codex_router_state::{
        quota_snapshot::QuotaRefreshErrorClass, sqlite::AsyncSqliteStateStore,
    };
    let fixture = QuotaResetFixture::create().await?;
    let account = AccountId::new("acct_pty_alpha")?;
    let state = AsyncSqliteStateStore::open(&fixture.root().join("state.sqlite")).await?;
    state
        .record_refresh_failure_preserving_selector_windows(
            &account,
            "responses",
            123,
            QuotaRefreshErrorClass::NetworkError,
        )
        .await?;
    state.close().await?;
    let state_before = fs::read(fixture.root().join("state.sqlite"))?;
    let request = SafeRequestRecord {
        method: "GET".to_owned(),
        path: "/usage".to_owned(),
        routing_account: Some("routing-pty-alpha".to_owned()),
        response_status: 200,
    };
    write_failure_evidence(fixture.root(), &fixture, &[request]).await?;
    let snapshot =
        fs::read_to_string(fixture.root().join("failed-refresh-status.json")).unwrap_or_default();
    ensure(
        snapshot.contains("\"last_error_class\":\"network_error\""),
        "failed capture discarded the stored refresh cause",
    )?;
    ensure(
        snapshot.contains("\"attempted\":true"),
        "refresh attempt presence missing",
    )?;
    ensure(
        snapshot.contains("\"has_success_status\":false"),
        "failed refresh acquired a success status",
    )?;
    for filename in ["failed-provider-ledger.json", "failed-refresh-status.json"] {
        let evidence = fs::read_to_string(fixture.root().join(filename))?;
        ensure(
            !evidence.contains("acct_pty_alpha"),
            "evidence leaked an account identity",
        )?;
        ensure(
            !evidence.contains("routing-pty-alpha"),
            "evidence leaked a routing identity",
        )?;
        for canary in crate::isolated_fixture_test::FORBIDDEN_TERMINAL_CANARIES {
            ensure(
                !evidence.to_lowercase().contains(canary),
                "evidence leaked a credential canary",
            )?;
        }
    }
    fixture.assert_secrets_unchanged()?;
    ensure(
        state_before == fs::read(fixture.root().join("state.sqlite"))?,
        "evidence collection mutated state",
    )?;
    let fixture_root = fixture.root().to_path_buf();
    drop(fixture);
    ensure(
        !fixture_root.exists(),
        "diagnostic fixture survived cleanup",
    )?;
    Ok(())
}
