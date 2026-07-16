use super::*;

pub(super) fn quota_status_view_model(
    report: &QuotaStatusReport,
    rows: &[QuotaStatusRow],
    width: usize,
) -> QuotaStatusViewModel {
    let selected_row = rows.iter().find(|row| row.preferred_next);
    QuotaStatusViewModel {
        width,
        route_line: quota_status_route_line(report, rows),
        why_line: String::new(),
        serving_clients: quota_status_serving_clients(rows),
        rows: rows
            .iter()
            .map(|row| QuotaStatusAccountViewModel {
                account_id: row.account_id.clone(),
                active_credential_generation: row.active_credential_generation,
                selected: row.preferred_next,
                account: row.account_label.clone(),
                status: quota_state_text(row).to_owned(),
                active_clients: active_clients_label(row),
                reset_credits: reset_credits_account_list_label(row.reset_credits_available_value),
                reason: reason_summary(row),
                weekly_window: quota_account_list_window_summary(
                    &row.windows,
                    V1_WEEKLY_WINDOW_SECONDS,
                    "weekly",
                    report.now_unix_seconds,
                ),
                short_window: quota_account_list_window_summary(
                    &row.windows,
                    V1_SHORT_WINDOW_SECONDS,
                    "5h",
                    report.now_unix_seconds,
                ),
                burn_meter: quota_safe_pace_meter(row.weekly_pace, report.now_unix_seconds),
                sample_metadata: sample_metadata_from_display_window(
                    &row.windows,
                    V1_WEEKLY_WINDOW_SECONDS,
                    report.now_unix_seconds,
                ),
                reset_pace: reset_pace_view_model_from_snapshot(
                    row.weekly_pace,
                    report.now_unix_seconds,
                ),
                weekly_pace: quota_pace_summary(row.weekly_pace, report.now_unix_seconds),
                details: quota_selected_account_view_model(report, row),
            })
            .collect(),
        selected: selected_row.map(|row| quota_selected_account_view_model(report, row)),
    }
}

pub(super) fn quota_status_serving_clients(rows: &[QuotaStatusRow]) -> Option<u32> {
    let total = rows
        .iter()
        .filter_map(|row| row.active_clients_value)
        .fold(0_u32, u32::saturating_add);
    (total > 0).then_some(total)
}

pub(super) fn quota_status_route_line(
    report: &QuotaStatusReport,
    rows: &[QuotaStatusRow],
) -> String {
    let Some(selected_row) = rows.iter().find(|row| row.preferred_next) else {
        return format!(
            "{} -> none    {}",
            report.route_band,
            selector_summary(rows)
        );
    };
    let mut parts = vec![
        format!("{} -> {}", report.route_band, selected_row.account_label),
        compact_routing_summary(selected_row),
    ];
    if let Some(total_rate) = quota_compact_total_burn_rate(selected_row.weekly_pace) {
        parts.push(total_rate);
    }
    if let Some(limiting_window) = selected_row.limiting_window {
        parts.push(format!(
            "{} {} left",
            quota_window_label(limiting_window.window_seconds()),
            format_percent(limiting_window.remaining_headroom())
        ));
    }
    parts.join("    ")
}

pub(super) fn compact_routing_summary(row: &QuotaStatusRow) -> String {
    first_line(&row.routing)
        .strip_prefix("preferred by quota: ")
        .unwrap_or_else(|| first_line(&row.routing))
        .to_owned()
}

pub(super) fn quota_selected_account_view_model(
    report: &QuotaStatusReport,
    row: &QuotaStatusRow,
) -> QuotaSelectedAccountViewModel {
    QuotaSelectedAccountViewModel {
        account: row.account_label.clone(),
        status: quota_state_text(row).to_owned(),
        reason: first_line(&row.routing).to_owned(),
        short_window: quota_window_visual_summary(
            &row.windows,
            V1_SHORT_WINDOW_SECONDS,
            "",
            report.now_unix_seconds,
        )
        .trim()
        .to_owned(),
        weekly_window: quota_window_visual_summary(
            &row.windows,
            V1_WEEKLY_WINDOW_SECONDS,
            "",
            report.now_unix_seconds,
        )
        .trim()
        .to_owned(),
        burn_meter: quota_safe_pace_meter(row.weekly_pace, report.now_unix_seconds),
        burn_pace: quota_pace_summary(row.weekly_pace, report.now_unix_seconds).replace("  ", " "),
        sample_metadata: sample_metadata_from_display_windows(
            &row.windows,
            report.now_unix_seconds,
        ),
        reset_pace: reset_pace_view_model_from_snapshot(row.weekly_pace, report.now_unix_seconds),
        short_reset_pace: short_reset_pace_view_model_from_snapshot(
            quota_display_pace_snapshot(
                &row.windows,
                V1_SHORT_WINDOW_SECONDS,
                report.now_unix_seconds,
            ),
            report.now_unix_seconds,
        ),
        total_rate: quota_total_rate_summary(row.weekly_pace),
        connection_rate: quota_connection_rate_summary(row.weekly_pace),
        active_clients: active_clients_label(row),
        guards: format!("5h {}% / weekly {}%", row.short_pressure, row.long_pressure),
        reset: row.reset_credits_available.clone(),
        note: first_line(&row.routing).to_owned(),
    }
}
