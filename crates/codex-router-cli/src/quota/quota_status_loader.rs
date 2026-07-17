use super::*;

pub(super) async fn load_quota_status_report_async(
    router_root: &Path,
    all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let quota_history_state =
        AsyncSqliteStateStore::open_read_only(&router_root.join("state.sqlite")).await?;
    let accounts = quota_history_state.list_accounts().await?;
    let report = quota_status_report(
        &quota_history_state,
        &accounts,
        all_limits,
        now_unix_seconds,
        unicode_bars,
    )
    .await?;
    quota_history_state.close().await?;
    Ok(report)
}

pub(super) async fn quota_status_report(
    quota_history_state: &AsyncSqliteStateStore,
    accounts: &[AccountRecord],
    _all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let selector_inputs = quota_history_state
        .selector_inputs_for_route_band(USER_QUOTA_ROUTE_BAND, now_unix_seconds)
        .await?;
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
        let assessment_input = projection_account.cloned().unwrap_or_else(|| {
            burn_down_input_from_display_windows(account, &display_windows, now_unix_seconds)
        });
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
            weekly_pace,
        });
        assessment_inputs.push(assessment_input);
    }

    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
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
        rows,
    })
}
