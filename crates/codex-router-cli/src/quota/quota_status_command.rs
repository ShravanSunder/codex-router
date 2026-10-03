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
    let credential_resources = QuotaCredentialResources::open(&router_root).await;
    let report = load_quota_status_report_with_availability_async(
        &router_root,
        all_limits,
        now_unix_seconds,
        true,
        credential_resources.availability(),
    )
    .await?;
    let view_model = quota_status_view_model(&report, report.rows(), width);
    let reload_view_model = quota_status_view_model_loader(
        router_root.clone(),
        all_limits,
        true,
        width,
        credential_resources.availability(),
    );
    let reset_session = credential_resources
        .credential_store()
        .map(|credential_store| reset_session_factory.create(&router_root, credential_store))
        .transpose()?;
    let weekly_floor_saver = weekly_quota_floor_saver(router_root.join("state.sqlite"));
    let credit_policy_saver = credit_usage_policy_saver(router_root.join("state.sqlite"));
    let credit_usage_refresher = reset_session_factory.credit_usage_refresher(&router_root);
    let (reset_session_ports, shutdown_sender, session_task) = match reset_session {
        Some(reset_session) => {
            let shutdown_sender = reset_session.ports.intent_sender.clone();
            let session_task = tokio::spawn(reset_session.runner);
            (
                Some(reset_session.ports),
                Some(shutdown_sender),
                Some(session_task),
            )
        }
        None => (None, None, None),
    };
    let render_result = run_quota_status_view(
        view_model,
        Some(reload_view_model),
        reset_session_ports,
        Some(weekly_floor_saver),
        Some(credit_policy_saver),
        Some(credit_usage_refresher),
    )
    .await;
    if let Some(shutdown_sender) = shutdown_sender {
        let _ = shutdown_sender
            .send(crate::quota_reset::reset_session_supervisor::ResetSessionIntent::Shutdown)
            .await;
        drop(shutdown_sender);
    }
    if let Some(session_task) = session_task {
        match await_reset_session_task(session_task).await? {
            crate::quota_reset::reset_session_supervisor::ResetSessionOutcome::Cancelled
            | crate::quota_reset::reset_session_supervisor::ResetSessionOutcome::Finished(_) => {}
        }
    }
    render_result.map_err(QuotaCommandError::Stdout)
}

fn credit_usage_policy_saver(database_path: PathBuf) -> CreditUsagePolicySaver {
    Arc::new(move |account_id, credential_generation, policy| {
        let database_path = database_path.clone();
        Box::pin(async move {
            let store = AsyncCreditUsagePolicyMutationStore::open(&database_path)
                .await
                .map_err(credit_policy_save_error)?;
            let result = store
                .save_account_credit_usage_policy(&account_id, credential_generation, policy)
                .await
                .map_err(credit_policy_save_error);
            store.close().await;
            result
        })
    })
}

fn credit_policy_save_error(error: StateStoreError) -> CreditUsagePolicySaveError {
    match error {
        StateStoreError::CreditUsagePolicyAccountUnavailable => {
            CreditUsagePolicySaveError::AccountUnavailable
        }
        StateStoreError::CreditUsagePolicyTargetChanged => {
            CreditUsagePolicySaveError::TargetChanged
        }
        StateStoreError::CreditUsagePolicyDatabaseBusy => CreditUsagePolicySaveError::DatabaseBusy,
        StateStoreError::CreditUsagePolicySchemaUpgradeRequired => {
            CreditUsagePolicySaveError::SchemaUpgradeRequired
        }
        _ => CreditUsagePolicySaveError::StateOperationFailed,
    }
}

pub(crate) fn interactive_credit_usage_refresher(
    router_root: PathBuf,
    base_url: String,
) -> CreditUsageRefresher {
    Arc::new(move |account_id, credential_generation| {
        let router_root = router_root.clone();
        let base_url = base_url.clone();
        Box::pin(async move {
            credit_usage_refresh_target_preflight(&router_root, &account_id, credential_generation)
                .await?;
            let state_db = router_root.join("state.sqlite");
            let resolver = crate::credential_runtime::AsyncCliCredentialResolver::open(
                &state_db,
                &router_root.join("secrets"),
            )
            .await
            .map_err(|_| CreditUsageRefreshError::Failed)?;
            let provider =
                HttpQuotaRefreshProvider::new().map_err(|_| CreditUsageRefreshError::Failed)?;
            let mut output_sink = std::io::sink();
            let report = refresh_quota_with_dependencies(
                &mut output_sink,
                router_root,
                base_url,
                &resolver,
                &provider,
                current_unix_seconds(),
            )
            .await
            .map_err(|_| CreditUsageRefreshError::Failed)?;
            if report.responses_observation_committed_for(&account_id, credential_generation) {
                Ok(())
            } else {
                Err(CreditUsageRefreshError::Failed)
            }
        })
    })
}

async fn credit_usage_refresh_target_preflight(
    router_root: &Path,
    account_id: &AccountId,
    expected_credential_generation: Option<u64>,
) -> Result<(), CreditUsageRefreshError> {
    let state_db = router_root.join("state.sqlite");
    let store = AsyncSqliteStateStore::open_read_only(&state_db)
        .await
        .map_err(|_| CreditUsageRefreshError::Failed)?;
    let accounts = store.list_accounts().await;
    let close_result = store.close().await;
    let accounts = match (accounts, close_result) {
        (Ok(accounts), Ok(())) => accounts,
        _ => return Err(CreditUsageRefreshError::Failed),
    };
    let Some(account) = accounts
        .into_iter()
        .find(|account| account.account_id() == account_id)
    else {
        return Err(CreditUsageRefreshError::ACCOUNT_UNAVAILABLE);
    };
    if account.status() != AccountStatus::Enabled {
        return Err(CreditUsageRefreshError::ACCOUNT_DISABLED);
    }
    if account.provider() != codex_router_core::provider::Provider::Openai {
        return Err(CreditUsageRefreshError::PROVIDER_UNSUPPORTED);
    }
    let Some(active_credential_generation) = account.active_credential_generation() else {
        return Err(CreditUsageRefreshError::CREDENTIALS_UNAVAILABLE);
    };
    if expected_credential_generation != Some(active_credential_generation) {
        return Err(CreditUsageRefreshError::TARGET_CHANGED);
    }
    Ok(())
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
    credential_store_availability: CredentialStoreAvailability,
) -> QuotaStatusViewModelLoader {
    Arc::new(move || {
        let router_root = router_root.clone();
        let credential_store_availability = credential_store_availability.clone();
        Box::pin(async move {
            let report = load_quota_status_report_with_availability_async(
                &router_root,
                all_limits,
                current_unix_seconds(),
                unicode_bars,
                credential_store_availability,
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
                codex_router_core::provider::Provider::Openai,
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

    #[tokio::test]
    async fn interactive_credit_policy_saver_reads_back_and_rejects_replaced_generation() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-router-tui-credit-policy-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("temporary router root should be created");
        let database_path = root.join("state.sqlite");
        let account_id = AccountId::new("tui-credit-policy").expect("account id should be valid");
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state should open and migrate before the mutation fixture");
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "credit policy",
                    AccountStatus::Disabled,
                )
                .with_active_credential_generation(7),
            )
            .await
            .expect("account should persist");
        state.close().await.expect("state should close");

        let saver = credit_usage_policy_saver(database_path.clone());
        assert_eq!(
            saver(
                account_id.clone(),
                Some(7),
                codex_router_core::credit_usage::CreditUsagePolicy::Allow,
            )
            .await
            .expect("disabled account policy can save and read back"),
            codex_router_core::credit_usage::CreditUsagePolicy::Allow
        );
        assert_eq!(
            saver(
                account_id.clone(),
                Some(6),
                codex_router_core::credit_usage::CreditUsagePolicy::Disallow,
            )
            .await
            .expect_err("old credential pane cannot overwrite the saved policy"),
            CreditUsagePolicySaveError::TargetChanged
        );

        let reopened = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("saved policy should reopen read-only");
        assert_eq!(
            reopened
                .load_account_credit_usage_policy(&account_id)
                .await
                .expect("saved policy should read"),
            codex_router_core::credit_usage::CreditUsagePolicy::Allow
        );
        reopened
            .close()
            .await
            .expect("read-only state should close");
        std::fs::remove_dir_all(root).expect("temporary router root should be removed");
    }
}

#[cfg(test)]
mod credit_usage_refresh_preflight_tests {
    use super::*;

    #[tokio::test]
    async fn unavailable_refresh_targets_are_rejected_before_secret_access() {
        let root = tempfile::tempdir().expect("temporary router root should exist");
        let database_path = root.path().join("state.sqlite");
        let disabled_id = AccountId::new("refresh-disabled").expect("account id");
        let credentialless_id = AccountId::new("refresh-credentialless").expect("account id");
        let claude_id = AccountId::new("refresh-claude").expect("account id");
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state should open");
        for account in [
            AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                disabled_id.clone(),
                "disabled",
                AccountStatus::Disabled,
            )
            .with_active_credential_generation(7),
            AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                credentialless_id.clone(),
                "credentialless",
                AccountStatus::Enabled,
            ),
            AccountRecord::new(
                codex_router_core::provider::Provider::Claude,
                claude_id.clone(),
                "Claude",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(11),
        ] {
            state
                .upsert_account(&account)
                .await
                .expect("account should persist");
        }
        state.close().await.expect("state should close");

        let refresher = interactive_credit_usage_refresher(
            root.path().to_path_buf(),
            "http://127.0.0.1:1".to_owned(),
        );
        assert_eq!(
            refresher(disabled_id, Some(7)).await,
            Err(CreditUsageRefreshError::ACCOUNT_DISABLED),
        );
        assert_eq!(
            refresher(credentialless_id, None).await,
            Err(CreditUsageRefreshError::CREDENTIALS_UNAVAILABLE),
        );
        assert_eq!(
            refresher(claude_id, Some(11)).await,
            Err(CreditUsageRefreshError::PROVIDER_UNSUPPORTED),
        );
        assert!(
            !root.path().join("secrets").exists(),
            "preflight must not create or open the secrets directory"
        );
    }
}
