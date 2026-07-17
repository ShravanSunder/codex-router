#[test]
fn quota_status_telemetry_contract_uses_scrubbed_low_cardinality_labels() {
    let source = concat!(
        include_str!("quota_status_loader.rs"),
        include_str!("quota_status_metrics.rs")
    );
    let Some(before_trace_event_name) = source
        .split("\"codex_router.quota_status_selection\"")
        .next()
    else {
        panic!("quota status tracing event should have a stable event name");
    };
    let Some(trace_event) = before_trace_event_name.rsplit("tracing::info!(").next() else {
        panic!("quota status tracing event should exist");
    };
    let Some(after_function_name) = source.split("fn emit_quota_status_metrics").nth(1) else {
        panic!("emit_quota_status_metrics helper should exist");
    };
    let Some(metrics_helper) = after_function_name
        .split("fn record_quota_refresh_metric")
        .next()
    else {
        panic!("quota status metrics helper should precede refresh metric helper");
    };

    for required_label in [
        "account.slot",
        "route_band",
        "transport",
        "selection.reason",
        "quota.window",
        "quota.remaining_bucket",
        "quota.guard_bucket",
    ] {
        assert!(
            metrics_helper.contains(required_label),
            "quota status telemetry must include {required_label}"
        );
    }
    for required_trace_label in [
        "route_band",
        "selected_pool",
        "selection.reason",
        "preferred.account_hash",
        "active_client.source",
    ] {
        assert!(
            trace_event.contains(required_trace_label),
            "quota status tracing attributes must include low-cardinality {required_trace_label}"
        );
    }
    for forbidden_label in [
        "account.id",
        "account.label",
        "reservation.id",
        "payload",
        "token",
        "sample.age_seconds",
        "sample.age_text",
        "provider.error",
    ] {
        assert!(
            !metrics_helper.contains(forbidden_label),
            "quota status telemetry must not include {forbidden_label}"
        );
        assert!(
            !trace_event.contains(forbidden_label),
            "quota status tracing attributes must not include {forbidden_label}"
        );
    }
}
