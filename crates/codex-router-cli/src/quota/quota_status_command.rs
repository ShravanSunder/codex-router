use super::*;

pub(super) async fn render_quota_status(
    stdout: &mut impl Write,
    router_root: PathBuf,
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let effective_format = effective_human_quota_format(format, stdout_is_terminal);
    let unicode_bars = effective_format != QuotaStatusFormat::Plain;
    let report =
        load_quota_status_report_async(&router_root, all_limits, now_unix_seconds, unicode_bars)
            .await?;
    match effective_format {
        QuotaStatusFormat::Table => write_quota_table_with_style(
            stdout,
            &report,
            stdout_terminal_width,
            QuotaTableStyle::TerminalColor,
        ),
        QuotaStatusFormat::Plain => write_quota_plain(stdout, &report),
        QuotaStatusFormat::Json => write_quota_json(stdout, &report),
    }
}

pub(super) async fn render_interactive_quota_status(
    router_root: PathBuf,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
    reset_session_factory: &dyn crate::quota_reset::InteractiveResetSessionFactory,
) -> Result<(), QuotaCommandError> {
    let width = stdout_terminal_width.unwrap_or(100).max(40);
    let report =
        load_quota_status_report_async(&router_root, all_limits, now_unix_seconds, true).await?;
    let view_model = quota_status_view_model(&report, report.rows(), width);
    let reload_view_model =
        quota_status_view_model_loader(router_root.clone(), all_limits, true, width);
    let reset_session = reset_session_factory.create(&router_root)?;
    let weekly_floor_saver = weekly_quota_floor_saver(router_root.join("state.sqlite"));
    let shutdown_sender = reset_session.ports.intent_sender.clone();
    let session_task = tokio::spawn(reset_session.runner);
    let render_result = run_quota_status_view(
        view_model,
        Some(reload_view_model),
        Some(reset_session.ports),
        Some(weekly_floor_saver),
    )
    .await;
    let _ = shutdown_sender
        .send(crate::quota_reset::reset_session_supervisor::ResetSessionIntent::Shutdown)
        .await;
    drop(shutdown_sender);
    let session_outcome = await_reset_session_task(session_task).await?;
    match session_outcome {
        crate::quota_reset::reset_session_supervisor::ResetSessionOutcome::Cancelled
        | crate::quota_reset::reset_session_supervisor::ResetSessionOutcome::Finished(_) => {}
    }
    render_result.map_err(QuotaCommandError::Stdout)
}

fn weekly_quota_floor_saver(database_path: PathBuf) -> WeeklyQuotaFloorSaver {
    Arc::new(move |account_id, percent| {
        let database_path = database_path.clone();
        Box::pin(async move {
            let floor = if percent == 0 {
                None
            } else {
                let basis_points = percent
                    .checked_mul(100)
                    .ok_or(WeeklyQuotaFloorSaveError::StateOperationFailed)?;
                Some(
                    WeeklyQuotaFloorBasisPoints::new(basis_points)
                        .map_err(|_| WeeklyQuotaFloorSaveError::StateOperationFailed)?,
                )
            };
            let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
                .await
                .map_err(render_safe_weekly_floor_error)?;
            let result = mutation
                .set_weekly_quota_floor_by_account_id(&account_id, floor)
                .await
                .map(|_| ())
                .map_err(render_safe_weekly_floor_error);
            mutation.close().await;
            result
        })
    })
}

fn render_safe_weekly_floor_error(error: StateStoreError) -> WeeklyQuotaFloorSaveError {
    match error {
        StateStoreError::WeeklyQuotaFloorDatabaseBusy => WeeklyQuotaFloorSaveError::DatabaseBusy,
        StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired => {
            WeeklyQuotaFloorSaveError::SchemaUpgradeRequired
        }
        StateStoreError::WeeklyQuotaFloorAccountNotFound => {
            WeeklyQuotaFloorSaveError::AccountNotFound
        }
        _ => WeeklyQuotaFloorSaveError::StateOperationFailed,
    }
}

pub(super) async fn await_reset_session_task(
    session_task: tokio::task::JoinHandle<
        crate::quota_reset::reset_session_supervisor::ResetSessionOutcome,
    >,
) -> Result<crate::quota_reset::reset_session_supervisor::ResetSessionOutcome, QuotaCommandError> {
    session_task
        .await
        .map_err(|_join_error| QuotaCommandError::ResetSessionTaskFailed)
}

pub(super) fn quota_status_view_model_loader(
    router_root: PathBuf,
    all_limits: bool,
    unicode_bars: bool,
    width: usize,
) -> QuotaStatusViewModelLoader {
    Arc::new(move || {
        let router_root = router_root.clone();
        Box::pin(async move {
            let report = load_quota_status_report_async(
                &router_root,
                all_limits,
                current_unix_seconds(),
                unicode_bars,
            )
            .await
            .ok()?;
            Some(quota_status_view_model(&report, report.rows(), width))
        })
    })
}

pub(super) fn effective_human_quota_format(
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
) -> QuotaStatusFormat {
    match format {
        QuotaStatusFormat::Json | QuotaStatusFormat::Plain => format,
        QuotaStatusFormat::Table if stdout_is_terminal => QuotaStatusFormat::Table,
        QuotaStatusFormat::Table => QuotaStatusFormat::Plain,
    }
}

#[cfg(test)]
mod weekly_floor_save_error_tests {
    use super::*;

    #[test]
    fn tui_weekly_floor_errors_are_closed_and_redacted() {
        let canaries = [
            "sensitive-account",
            "/private/router/state.sqlite",
            "internal sql diagnostic",
        ];
        for canary in canaries {
            let mapped = render_safe_weekly_floor_error(StateStoreError::Sqlite {
                message: canary.to_owned(),
            });
            assert_eq!(mapped, WeeklyQuotaFloorSaveError::StateOperationFailed);
            assert!(!format!("{mapped:?}").contains(canary));
        }
        assert_eq!(
            render_safe_weekly_floor_error(StateStoreError::WeeklyQuotaFloorDatabaseBusy),
            WeeklyQuotaFloorSaveError::DatabaseBusy
        );
        assert_eq!(
            render_safe_weekly_floor_error(StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired),
            WeeklyQuotaFloorSaveError::SchemaUpgradeRequired
        );
        assert_eq!(
            render_safe_weekly_floor_error(StateStoreError::WeeklyQuotaFloorAccountNotFound),
            WeeklyQuotaFloorSaveError::AccountNotFound
        );
    }

    #[tokio::test]
    async fn interactive_weekly_floor_saver_persists_by_stable_account_id() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-router-tui-weekly-floor-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("temporary router root should be created");
        let database_path = root.join("state.sqlite");
        let account_id = AccountId::new("tui-floor-account").expect("account id should be valid");
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state should open");
        state
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "duplicate",
                AccountStatus::Enabled,
            ))
            .await
            .expect("account should persist");
        state.close().await.expect("state should close");

        weekly_quota_floor_saver(database_path.clone())(account_id.clone(), 15)
            .await
            .expect("TUI saver should persist 15 percent");

        let state = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("state should reopen read-only");
        let policies = state
            .list_account_routing_policies()
            .await
            .expect("policy should read");
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].account_id(), &account_id);
        assert_eq!(
            policies[0].weekly_quota_floor_basis_points().basis_points(),
            1_500
        );
        state.close().await.expect("read-only state should close");
        std::fs::remove_dir_all(root).expect("temporary router root should be removed");
    }
}
