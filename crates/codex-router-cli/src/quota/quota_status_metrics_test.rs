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

#[test]
fn claude_post_renewal_auth_rejection_has_a_provider_scoped_counter() {
    let source = include_str!("quota_status_metrics.rs");
    let Some(after_metric) = source
        .split("fn record_claude_usage_auth_rejected_after_renewal")
        .nth(1)
    else {
        panic!("Claude post-renewal auth rejection metric helper should exist");
    };

    assert!(source.contains("codex_router_claude_usage_auth_rejected_after_renewal_total"));
    assert!(after_metric.contains("provider"));
    assert!(after_metric.contains("route_band"));
    assert!(!after_metric.contains("account_id"));
    assert!(!after_metric.contains("token"));
}

#[test]
fn credential_upkeep_refresh_counter_records_provider_outcomes_and_classes() {
    let source = include_str!("../credential_upkeep_worker/telemetry.rs");
    let Some(after_metric) = source
        .split("fn record_credential_upkeep_refresh_outcome")
        .nth(1)
    else {
        panic!("credential upkeep refresh counter helper should exist");
    };
    let Some(metric_helper_body) = after_metric.split("\n}\n").next() else {
        panic!("credential upkeep refresh counter helper body should be extractable");
    };

    assert!(source.contains("codex_router_credential_upkeep_refresh_total"));
    assert!(metric_helper_body.contains("provider"));
    assert!(metric_helper_body.contains("refresh.outcome"));
    assert!(metric_helper_body.contains("refresh.failure_class"));
    assert!(!metric_helper_body.contains("account_id"));
    assert!(!metric_helper_body.contains("token"));
}
