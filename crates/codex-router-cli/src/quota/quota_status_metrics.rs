use super::*;

pub(super) fn telemetry_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

pub(super) fn emit_quota_status_metrics(route_band: &str, rows: &[QuotaStatusRow]) {
    if !rows.iter().any(|row| row.preferred_next) {
        global::meter("codex-router")
            .u64_counter("codex_router_account_rejections_total")
            .build()
            .add(
                1,
                &[
                    KeyValue::new("account.slot", "none"),
                    KeyValue::new("route_band", route_band.to_owned()),
                    KeyValue::new("transport", "cli"),
                    KeyValue::new("selection.reason", "no_quota_candidate"),
                ],
            );
    }

    for row in rows {
        let account_hash = telemetry_hash(row.account_id.as_str());
        if row.preferred_next {
            global::meter("codex-router")
                .u64_counter("codex_router_account_selections_total")
                .build()
                .add(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("transport", "cli"),
                        KeyValue::new("selection.reason", routing_reason_json(row.routing_reason)),
                    ],
                );
        }
        if !row.preferred_next {
            global::meter("codex-router")
                .u64_counter("codex_router_account_rejections_total")
                .build()
                .add(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("transport", "cli"),
                        KeyValue::new("selection.reason", routing_reason_json(row.routing_reason)),
                    ],
                );
        }
        if let Some(active_clients) = row.active_clients_value {
            global::meter("codex-router")
                .u64_gauge("codex_router_active_clients")
                .build()
                .record(
                    u64::from(active_clients),
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("transport", "sqlx_mirror"),
                    ],
                );
        }
        for window in &row.windows {
            global::meter("codex-router")
                .u64_gauge("codex_router_quota_remaining_bucket")
                .build()
                .record(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new(
                            "quota.window",
                            quota_window_label(window.window_seconds).to_owned(),
                        ),
                        KeyValue::new(
                            "quota.remaining_bucket",
                            quota_remaining_bucket(window.remaining_headroom),
                        ),
                    ],
                );
        }
        for (window_label, guard_deficit) in
            [("5h", row.short_pressure), ("weekly", row.long_pressure)]
        {
            global::meter("codex-router")
                .u64_gauge("codex_router_quota_guard_bucket")
                .build()
                .record(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("quota.window", window_label),
                        KeyValue::new("quota.guard_bucket", quota_guard_bucket(guard_deficit)),
                    ],
                );
        }
    }
}

pub(super) fn record_quota_refresh_metric(
    route_band: &str,
    refresh_outcome: &'static str,
    refresh_error_class: &'static str,
) {
    global::meter("codex-router")
        .u64_counter("codex_router_quota_refresh_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("route_band", route_band.to_owned()),
                KeyValue::new("refresh.outcome", refresh_outcome),
                KeyValue::new("refresh.error_class", refresh_error_class),
            ],
        );
}

pub(super) fn quota_remaining_bucket(remaining_headroom: u32) -> &'static str {
    match remaining_headroom {
        0 => "empty",
        1..=4 => "lt_5",
        5..=24 => "lt_25",
        25..=49 => "lt_50",
        50..=74 => "lt_75",
        _ => "gte_75",
    }
}

pub(super) fn quota_guard_bucket(guard_deficit: u32) -> &'static str {
    match guard_deficit {
        0 => "none",
        1..=24 => "low",
        25..=49 => "medium",
        50..=74 => "high",
        _ => "critical",
    }
}
