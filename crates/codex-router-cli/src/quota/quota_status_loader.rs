use super::*;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::route_profile::RESPONSES_HTTP;

#[derive(Clone)]
pub(super) struct QuotaCredentialResources {
    credential_store:
        Option<codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore>,
    availability: CredentialStoreAvailability,
}

impl QuotaCredentialResources {
    pub(super) async fn open(router_root: &Path) -> Self {
        match crate::secret_store_factory::open_cli_secret_store_async(router_root.join("secrets"))
            .await
        {
            Ok(credential_store) => Self::from_opened_store(credential_store),
            Err(_error) => Self {
                credential_store: None,
                availability: CredentialStoreAvailability::Unavailable,
            },
        }
    }

    fn from_opened_store(
        credential_store: codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore,
    ) -> Self {
        let availability = credential_store_availability(&credential_store);
        Self {
            credential_store: Some(credential_store),
            availability,
        }
    }

    pub(super) fn credential_store(
        &self,
    ) -> Option<codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore>
    {
        self.credential_store.clone()
    }

    pub(super) fn availability(&self) -> CredentialStoreAvailability {
        self.availability.clone()
    }
}

pub(super) async fn load_quota_status_report_async(
    router_root: &Path,
    all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let credential_resources = QuotaCredentialResources::open(router_root).await;
    load_quota_status_report_with_availability_async(
        router_root,
        all_limits,
        now_unix_seconds,
        unicode_bars,
        credential_resources.availability(),
    )
    .await
}

pub(super) async fn load_quota_status_report_with_availability_async(
    router_root: &Path,
    all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
    credential_store_availability: CredentialStoreAvailability,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let state_database_path = router_root.join("state.sqlite");
    let quota_history_state =
        match AsyncSqliteStateStore::open_read_only(&state_database_path).await {
            Ok(state) => state,
            Err(error) => {
                #[cfg(test)]
                report_quota_state_lock_diagnostic(&state_database_path, "open_read_only", &error);
                return Err(error.into());
            }
        };
    let accounts = match quota_history_state.list_accounts().await {
        Ok(accounts) => accounts,
        Err(error) => {
            #[cfg(test)]
            report_quota_state_lock_diagnostic(&state_database_path, "list_accounts", &error);
            return Err(error.into());
        }
    };
    let mut report = match quota_status_report(
        &quota_history_state,
        &accounts,
        all_limits,
        now_unix_seconds,
        unicode_bars,
    )
    .await
    {
        Ok(report) => report,
        Err(error) => {
            #[cfg(test)]
            report_quota_state_lock_diagnostic(&state_database_path, "quota_status_report", &error);
            return Err(error);
        }
    };
    if let Err(error) = quota_history_state.close().await {
        #[cfg(test)]
        report_quota_state_lock_diagnostic(&state_database_path, "close", &error);
        return Err(error.into());
    }
    report.credential_store_availability = credential_store_availability;
    Ok(report)
}

#[cfg(test)]
fn report_quota_state_lock_diagnostic(
    database_path: &Path,
    operation: &str,
    error: &dyn std::fmt::Display,
) {
    let mut wal_path = database_path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let mut shared_memory_path = database_path.as_os_str().to_os_string();
    shared_memory_path.push("-shm");
    let database_files = [
        database_path.to_path_buf(),
        std::path::PathBuf::from(wal_path),
        std::path::PathBuf::from(shared_memory_path),
    ];

    eprintln!(
        "quota_status_sqlite_operation_failed pid={} operation={operation} database={} error={error}",
        std::process::id(),
        database_path.display(),
    );
    for path in &database_files {
        let file_state = std::fs::metadata(path)
            .map(|metadata| format!("present, {} bytes", metadata.len()))
            .unwrap_or_else(|metadata_error| format!("absent or unavailable: {metadata_error}"));
        match std::process::Command::new("/usr/sbin/lsof")
            .args(["-nP", "-Fpcfn"])
            .arg(path)
            .output()
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let process_ids = stdout
                    .lines()
                    .filter_map(|line| line.strip_prefix('p'))
                    .collect::<Vec<_>>();
                let images = process_ids
                    .iter()
                    .map(|process_id| {
                        match std::process::Command::new("/usr/sbin/lsof")
                            .args(["-a", "-p", process_id, "-d", "txt", "-Fin"])
                            .output()
                        {
                            Ok(image) => format!(
                                "pid={process_id} exit={:?} image={:?}",
                                image.status.code(),
                                String::from_utf8_lossy(&image.stdout),
                            ),
                            Err(image_error) => {
                                format!("pid={process_id} image-inspection-error={image_error}")
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                eprintln!(
                    "quota_status_sqlite_file path={} state={file_state} lsof_exit={:?} open_processes={process_ids:?} images={images:?} output={stdout:?}",
                    path.display(),
                    output.status.code(),
                );
            }
            Err(lsof_error) => {
                eprintln!(
                    "quota_status_sqlite_file path={} state={file_state} lsof_error={lsof_error}",
                    path.display(),
                );
            }
        }
    }
}

fn credential_store_availability(
    credential_store: &codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore,
) -> CredentialStoreAvailability {
    match credential_store.status() {
        codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus::Ready => {
            CredentialStoreAvailability::Ready
        }
        codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus::KeyUnavailable => {
            CredentialStoreAvailability::KeychainLocked
        }
        codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus::MigrationIncomplete { accounts, failure } => {
            CredentialStoreAvailability::MigrationIncomplete { accounts, failure }
        }
    }
}

pub(super) async fn quota_status_report(
    quota_history_state: &AsyncSqliteStateStore,
    accounts: &[AccountRecord],
    _all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let accounts = accounts
        .iter()
        .filter(|account| account.provider() == codex_router_core::provider::Provider::Openai)
        .collect::<Vec<_>>();
    let selector_inputs = quota_history_state
        .selector_inputs_for_route_band(USER_QUOTA_ROUTE_BAND, now_unix_seconds)
        .await?;
    let weekly_quota_floors = quota_history_state
        .list_account_routing_policies()
        .await?
        .into_iter()
        .map(|policy| {
            (
                policy.account_id().clone(),
                u32::from(policy.weekly_quota_floor_basis_points().basis_points()),
            )
        })
        .collect::<HashMap<_, _>>();
    let refresh_statuses = quota_history_state
        .quota_refresh_statuses_for_route_band(USER_QUOTA_ROUTE_BAND)
        .await?;
    let refresh_statuses = refresh_statuses
        .into_iter()
        .map(|status| (status.account_id().clone(), status))
        .collect::<HashMap<_, _>>();
    let active_client_counts_result = quota_history_state
        .active_client_counts_for_route_band_read_only(
            USER_QUOTA_ROUTE_BAND,
            now_unix_seconds,
            ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS,
        )
        .await;
    let active_client_mirror_source = if active_client_counts_result.is_ok() {
        "sqlx_mirror"
    } else {
        "unavailable"
    };
    let active_client_counts = active_client_counts_result.as_ref().ok().map(|counts| {
        counts
            .iter()
            .map(|count| {
                (
                    count.account_id().clone(),
                    ActiveClientMirrorLoad {
                        count: count.active_clients(),
                        pressure: count.active_pressure(),
                    },
                )
            })
            .collect::<HashMap<_, _>>()
    });
    let selection_projection_result = project_route_band_selection_inputs_read_only(
        quota_history_state,
        USER_QUOTA_ROUTE_BAND,
        now_unix_seconds,
        ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS,
    )
    .await;
    let selection_projection_source = if selection_projection_result.is_ok() {
        SelectionProjectionSource::SqlxProjection
    } else {
        SelectionProjectionSource::DisplayWindowsFallback
    };
    let selection_projection = selection_projection_result.as_ref().ok();
    let mut status_inputs = Vec::new();
    let mut assessment_inputs = Vec::new();
    for account in accounts {
        let selector_input = selector_inputs
            .iter()
            .find(|input| input.account_id() == account.account_id());
        let snapshot = quota_history_state
            .load_quota_snapshot_for_route_band(account.account_id(), USER_QUOTA_ROUTE_BAND)
            .await?;
        let reset_credits_available = snapshot
            .as_ref()
            .and_then(PersistedQuotaSnapshot::reset_credits_available);
        let mut display_windows = if let Some(selector_input) = selector_input {
            display_windows_from_selector_input(selector_input)
        } else {
            snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
                vec![DisplayQuotaWindow::from_snapshot(snapshot)]
            })
        };
        attach_history_estimates_to_display_windows(
            quota_history_state,
            account.account_id(),
            USER_QUOTA_ROUTE_BAND,
            now_unix_seconds,
            &mut display_windows,
        )
        .await?;
        let projection_account = selection_projection.and_then(|projection| {
            projection
                .accounts()
                .iter()
                .find(|projected_account| projected_account.account_id() == account.account_id())
        });
        let projected_weekly_window = projection_account.and_then(|projected_account| {
            projected_account
                .windows()
                .iter()
                .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        });
        let weekly_pace =
            quota_pace_snapshot(&display_windows, projected_weekly_window, now_unix_seconds);
        let mut assessment_input = projection_account.cloned().unwrap_or_else(|| {
            burn_down_input_from_display_windows(account, &display_windows, now_unix_seconds)
        });
        let weekly_quota_floor_basis_points =
            weekly_quota_floors.get(account.account_id()).copied();
        if let Some(floor_basis_points) = weekly_quota_floor_basis_points {
            assessment_input =
                assessment_input.with_weekly_quota_floor_basis_points(floor_basis_points);
        }
        let active_clients =
            active_client_counts
                .as_ref()
                .map_or(ActiveClientMirrorStatus::Unavailable, |counts| {
                    let load = counts
                        .get(account.account_id())
                        .copied()
                        .unwrap_or(ActiveClientMirrorLoad::EMPTY);
                    ActiveClientMirrorStatus::MirrorFresh {
                        count: load.count,
                        pressure: load.pressure,
                        max_age_seconds: ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS,
                    }
                });
        status_inputs.push(QuotaStatusAccountInput {
            account_label: account.label().to_owned(),
            account_status: account.status().as_str().to_owned(),
            account_id: account.account_id().clone(),
            active_credential_generation: account.active_credential_generation(),
            reset_credits_available,
            updated: format_refresh_status(
                refresh_statuses.get(account.account_id()),
                now_unix_seconds,
            ),
            active_clients,
            windows: display_windows,
            credit_usage: credit_usage_status(
                selector_input,
                refresh_statuses.get(account.account_id()),
                account.active_credential_generation(),
                now_unix_seconds,
            ),
            weekly_pace,
            weekly_quota_floor_basis_points,
            oauth_maintenance: quota_history_state
                .load_credential_maintenance(account.account_id())
                .await?
                .filter(|record| {
                    Some(record.credential_generation) == account.active_credential_generation()
                }),
        });
        assessment_inputs.push(assessment_input);
    }

    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        RESPONSES_HTTP.clone(),
        assessment_inputs,
    ));
    let selected_pool = assessment.selected_pool();
    let authoritative_projection = selection_projection_source.is_authoritative();
    let preferred_next_account_id = authoritative_projection
        .then(|| assessment.preferred_next().cloned())
        .flatten();
    let preferred_next_hash = preferred_next_account_id
        .as_ref()
        .map(|account_id| telemetry_hash(account_id.as_str()))
        .unwrap_or_else(|| "none".to_owned());
    let preferred_selection_reason = preferred_next_account_id
        .as_ref()
        .and_then(|preferred_account_id| {
            assessment
                .accounts()
                .iter()
                .find(|account| account.account_id() == preferred_account_id)
        })
        .map_or("none", |account| {
            routing_reason_json(account.routing_reason())
        });
    tracing::info!(
        route_band = USER_QUOTA_ROUTE_BAND,
        selected_pool = selected_pool_json(selected_pool),
        selection.reason = preferred_selection_reason,
        preferred.account_hash = preferred_next_hash.as_str(),
        active_client.source = active_client_mirror_source,
        "codex_router.quota_status_selection"
    );
    let mut rows = status_inputs
        .iter()
        .filter_map(|input| {
            assessment
                .accounts()
                .iter()
                .find(|assessment| assessment.account_id() == &input.account_id)
                .map(|assessment| {
                    QuotaStatusRow::from_assessment(
                        input,
                        assessment,
                        now_unix_seconds,
                        unicode_bars,
                    )
                })
        })
        .collect::<Vec<_>>();
    if !authoritative_projection {
        for row in &mut rows {
            row.normalize_degraded_projection_authority();
        }
    }
    emit_quota_status_metrics(USER_QUOTA_ROUTE_BAND, &rows);

    Ok(QuotaStatusReport {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        route_band: USER_QUOTA_ROUTE_BAND.to_owned(),
        selected_pool,
        preferred_next_account_id,
        selection_projection_source,
        now_unix_seconds,
        credential_store_availability: CredentialStoreAvailability::Ready,
        rows,
    })
}

fn credit_usage_status(
    selector_input: Option<&SelectorQuotaInput>,
    refresh_status: Option<&QuotaRefreshStatusView>,
    active_credential_generation: Option<u64>,
    now_unix_seconds: u64,
) -> CreditUsageStatus {
    let policy = selector_input.map_or_default(SelectorQuotaInput::credit_usage_policy);
    let observation = selector_input.and_then(SelectorQuotaInput::credit_observation);
    let provider_observation = observation
        .map_or_else(CreditProviderObservation::missing, |value| {
            value.provider_observation().clone()
        });
    let age_label = observation
        .and_then(|value| value.observed_unix_seconds())
        .map(|observed_unix_seconds| {
            sample_metadata_from_observed_windows(&[observed_unix_seconds], now_unix_seconds)
                .age_label
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let freshness = observation.map_or(CreditUsageFreshness::Unknown, |value| {
        let observation_is_current = active_credential_generation
            == Some(value.credential_generation())
            && value.committed_attempt() == Some(value.latest_started_attempt())
            && value
                .observed_unix_seconds()
                .is_some_and(|observed| observed <= now_unix_seconds);
        if !observation_is_current {
            return CreditUsageFreshness::Unknown;
        }
        match refresh_status.and_then(QuotaRefreshStatusView::stale_after_unix_seconds) {
            Some(stale_after) if now_unix_seconds < stale_after => CreditUsageFreshness::Fresh,
            Some(_) => CreditUsageFreshness::Stale,
            None => CreditUsageFreshness::Unknown,
        }
    });

    CreditUsageStatus {
        policy,
        provider_observation,
        freshness,
        age_label,
    }
}

#[cfg(test)]
#[path = "quota_status_loader_tests.rs"]
mod tests;
