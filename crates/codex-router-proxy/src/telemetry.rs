//! Scrubbed telemetry helpers for runtime routing metrics.

use opentelemetry::KeyValue;
use opentelemetry::global;

const METER_NAME: &str = "codex-router";

/// Queue degradation class safe for metrics and logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueDegradedReason {
    /// Queue has no immediate capacity.
    Full,
    /// Queue receiver is closed.
    Closed,
    /// Queue accepted work but durable storage failed.
    WriteFailed,
}

impl QueueDegradedReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Closed => "closed",
            Self::WriteFailed => "write_failed",
        }
    }
}

/// Scrubbed queue degraded event emitted by actor enqueue surfaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueDegradedEvent {
    queue_name: &'static str,
    route_band: &'static str,
    reason: QueueDegradedReason,
    queue_capacity: usize,
}

impl QueueDegradedEvent {
    /// Returns the safe queue name.
    #[must_use]
    pub const fn queue_name(&self) -> &'static str {
        self.queue_name
    }

    /// Returns the safe route-band name.
    #[must_use]
    pub const fn route_band(&self) -> &'static str {
        self.route_band
    }

    /// Returns the queue degradation class.
    #[must_use]
    pub const fn reason(&self) -> QueueDegradedReason {
        self.reason
    }
}

/// Scrubbed DB write queue lag event emitted when actor work starts processing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueLagEvent {
    queue_name: &'static str,
    route_band: &'static str,
    lag_millis: u64,
}

impl QueueLagEvent {
    /// Returns the safe queue name.
    #[must_use]
    pub const fn queue_name(&self) -> &'static str {
        self.queue_name
    }

    /// Returns the safe route-band name.
    #[must_use]
    pub const fn route_band(&self) -> &'static str {
        self.route_band
    }

    /// Returns measured queue lag in milliseconds.
    #[must_use]
    pub const fn lag_millis(&self) -> u64 {
        self.lag_millis
    }
}

/// Records a scrubbed account selection metric.
pub fn record_account_selected(
    account_hash: String,
    route_band: &str,
    transport: &'static str,
    selection_reason: &str,
) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_counter("codex_router_account_selections_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("account.slot", account_hash),
                KeyValue::new("route_band", route_band.to_owned()),
                KeyValue::new("transport", transport),
                KeyValue::new("selection.reason", selection_reason.to_owned()),
            ],
        );
}

/// Records a scrubbed account rejection metric.
pub fn record_account_rejected(route_band: &str, selection_reason: &'static str) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_counter("codex_router_account_rejections_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("account.slot", "none"),
                KeyValue::new("route_band", route_band.to_owned()),
                KeyValue::new("transport", "runtime"),
                KeyValue::new("selection.reason", selection_reason),
            ],
        );
}

/// Records a scrubbed active-client mirror gauge sample.
pub fn record_active_clients(
    account_hash: String,
    route_band: &str,
    transport: &'static str,
    active_clients: u64,
) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_gauge("codex_router_active_clients")
        .build()
        .record(
            active_clients,
            &[
                KeyValue::new("account.slot", account_hash),
                KeyValue::new("route_band", route_band.to_owned()),
                KeyValue::new("transport", transport),
            ],
        );
}

/// Records a scrubbed WebSocket lifecycle metric.
pub fn record_websocket_event(route_band: &'static str, event_kind: &'static str) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_counter("codex_router_websocket_events_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("route_band", route_band),
                KeyValue::new("event.kind", event_kind),
            ],
        );
}

/// Records a scrubbed DB write queue depth sample.
pub fn record_db_write_queue_depth(queue_name: &'static str, route_band: &'static str, depth: u64) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_gauge("codex_router_db_write_queue_depth")
        .build()
        .record(
            depth,
            &[
                KeyValue::new("queue.name", queue_name),
                KeyValue::new("route_band", route_band),
            ],
        );
}

/// Records a scrubbed DB write queue event.
pub fn record_db_write_queue_event(
    queue_name: &'static str,
    route_band: &'static str,
    queue_result: &'static str,
    degraded_reason: &'static str,
) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_counter("codex_router_db_write_queue_events_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("queue.name", queue_name),
                KeyValue::new("route_band", route_band),
                KeyValue::new("queue.result", queue_result),
                KeyValue::new("degraded.reason", degraded_reason),
            ],
        );
}

/// Records and emits a scrubbed DB write queue lag observation.
pub fn emit_db_write_queue_lag_observed(
    queue_name: &'static str,
    route_band: &'static str,
    queue_state: &'static str,
    lag_millis: u64,
) -> QueueLagEvent {
    let meter = global::meter(METER_NAME);
    meter
        .u64_gauge("codex_router_db_write_queue_lag_millis")
        .build()
        .record(
            lag_millis,
            &[
                KeyValue::new("queue.name", queue_name),
                KeyValue::new("route_band", route_band),
                KeyValue::new("queue.state", queue_state),
            ],
        );
    tracing::info!(
        queue.name = queue_name,
        route_band = route_band,
        queue.state = queue_state,
        queue.lag_millis = lag_millis,
        "codex_router.db_write_queue_lag_observed"
    );
    QueueLagEvent {
        queue_name,
        route_band,
        lag_millis,
    }
}

/// Emits a scrubbed DB write queue degraded event and records its metric.
#[must_use]
pub fn emit_db_write_queue_degraded(
    queue_name: &'static str,
    route_band: &'static str,
    reason: QueueDegradedReason,
    queue_capacity: usize,
) -> QueueDegradedEvent {
    record_db_write_queue_event(queue_name, route_band, "degraded", reason.as_str());
    tracing::warn!(
        queue.name = queue_name,
        route_band = route_band,
        degraded.reason = reason.as_str(),
        queue.capacity = queue_capacity,
        "codex_router.db_write_queue_degraded"
    );
    QueueDegradedEvent {
        queue_name,
        route_band,
        reason,
        queue_capacity,
    }
}

/// Records and emits a scrubbed snapshot freshness observation.
pub fn emit_snapshot_freshness_observed(
    snapshot_source: &'static str,
    route_band: &'static str,
    freshness_state: &'static str,
    fallback_policy: &'static str,
    age_seconds: u64,
) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_gauge("codex_router_snapshot_freshness_age_seconds")
        .build()
        .record(
            age_seconds,
            &[
                KeyValue::new("snapshot.source", snapshot_source),
                KeyValue::new("route_band", route_band),
                KeyValue::new("freshness.state", freshness_state),
                KeyValue::new("fallback.policy", fallback_policy),
            ],
        );
    tracing::info!(
        snapshot.source = snapshot_source,
        route_band = route_band,
        freshness.state = freshness_state,
        fallback.policy = fallback_policy,
        freshness.age_seconds = age_seconds,
        "codex_router.snapshot_freshness_observed"
    );
}

/// Records and emits a scrubbed maintenance lag observation.
pub fn emit_maintenance_lag_observed(
    maintenance_class: &'static str,
    route_band: &'static str,
    maintenance_state: &'static str,
    lag_millis: u64,
) {
    let meter = global::meter(METER_NAME);
    meter
        .u64_gauge("codex_router_maintenance_lag_millis")
        .build()
        .record(
            lag_millis,
            &[
                KeyValue::new("maintenance.class", maintenance_class),
                KeyValue::new("route_band", route_band),
                KeyValue::new("maintenance.state", maintenance_state),
            ],
        );
    tracing::warn!(
        maintenance.class = maintenance_class,
        route_band = route_band,
        maintenance.state = maintenance_state,
        maintenance.lag_millis = lag_millis,
        "codex_router.maintenance_lag_observed"
    );
}

#[cfg(test)]
mod tests {
    use crate::test_log_capture::capture_log_output;

    #[test]
    fn metric_names_match_plan3_contract() {
        let source = include_str!("telemetry.rs");
        for metric_name in [
            "codex_router_active_clients",
            "codex_router_account_selections_total",
            "codex_router_account_rejections_total",
            "codex_router_websocket_events_total",
        ] {
            assert!(source.contains(metric_name), "missing {metric_name}");
        }
    }

    #[test]
    fn metric_helpers_do_not_use_forbidden_label_keys() {
        let source = include_str!("telemetry.rs");
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);
        for forbidden in [
            "account.id",
            "account.label",
            "reservation.id",
            "route.path",
            "prompt",
            "payload",
            "token",
            "provider.body",
        ] {
            assert!(
                !production_source.contains(forbidden),
                "runtime metrics must not use forbidden label key {forbidden}"
            );
        }
    }

    #[test]
    fn account_rejection_metrics_include_scrubbed_routing_dimensions() {
        let source = include_str!("telemetry.rs");
        let Some(after_function_name) = source.split("pub fn record_account_rejected").nth(1)
        else {
            panic!("record_account_rejected helper should exist");
        };
        let Some(rejection_helper) = after_function_name
            .split("/// Records a scrubbed active-client")
            .next()
        else {
            panic!("record_account_rejected helper should precede active-client helper");
        };

        for required_label in [
            "account.slot",
            "transport",
            "route_band",
            "selection.reason",
        ] {
            assert!(
                rejection_helper.contains(required_label),
                "account rejection metrics must include {required_label}"
            );
        }
    }

    #[test]
    fn queue_metric_helpers_do_not_use_forbidden_label_keys() {
        let source = include_str!("telemetry.rs");
        let Some(after_function_name) = source.split("pub fn record_db_write_queue_depth").nth(1)
        else {
            panic!("record_db_write_queue_depth helper should exist");
        };
        let Some(queue_helpers) = after_function_name.split("#[cfg(test)]").next() else {
            panic!("queue metric helpers should be production helpers");
        };

        for metric_name in [
            "codex_router_db_write_queue_depth",
            "codex_router_db_write_queue_events_total",
            "codex_router_db_write_queue_lag_millis",
        ] {
            assert!(
                queue_helpers.contains(metric_name),
                "queue telemetry should emit {metric_name}"
            );
        }

        for required_label in [
            "queue.name",
            "route_band",
            "queue.result",
            "degraded.reason",
            "queue.state",
        ] {
            assert!(
                queue_helpers.contains(required_label),
                "queue telemetry should include scrubbed label {required_label}"
            );
        }

        for forbidden in [
            "account.id",
            "account.label",
            "reservation.id",
            "session.id",
            "route.path",
            "prompt",
            "payload",
            "token",
            "provider.body",
            "auth.header",
        ] {
            assert!(
                !queue_helpers.contains(forbidden),
                "queue telemetry must not use forbidden label key {forbidden}"
            );
        }
    }

    #[test]
    fn emitted_queue_degraded_event_excludes_forbidden_values() {
        let event = super::emit_db_write_queue_degraded(
            "provider_quota_exhaustion",
            "responses",
            super::QueueDegradedReason::Full,
            128,
        );
        let rendered_event = format!("{event:?}");

        assert!(rendered_event.contains("provider_quota_exhaustion"));
        assert!(rendered_event.contains("responses"));
        assert!(rendered_event.contains("Full"));
        assert!(!rendered_event.contains("raw-provider-body-canary"));
        assert!(!rendered_event.contains("sk-live-token-canary"));
        assert!(!rendered_event.contains("Authorization"));
        assert!(!rendered_event.contains("acct_raw_canary"));
        assert!(!rendered_event.contains("friendly account label"));
        assert!(!rendered_event.contains("reservation_raw_canary"));
        assert!(!rendered_event.contains("/Users/shravansunder"));
    }

    #[test]
    fn emitted_queue_degraded_log_excludes_forbidden_values() {
        let rendered_log = capture_log_output(|| {
            let _event = super::emit_db_write_queue_degraded(
                "provider_quota_exhaustion",
                "responses",
                super::QueueDegradedReason::Full,
                128,
            );
        });

        assert!(rendered_log.contains("codex_router.db_write_queue_degraded"));
        assert!(rendered_log.contains("provider_quota_exhaustion"));
        assert!(rendered_log.contains("responses"));
        assert!(rendered_log.contains("full"));
        assert!(!rendered_log.contains("raw-provider-body-canary"));
        assert!(!rendered_log.contains("sk-live-token-canary"));
        assert!(!rendered_log.contains("Authorization"));
        assert!(!rendered_log.contains("acct_raw_canary"));
        assert!(!rendered_log.contains("friendly account label"));
        assert!(!rendered_log.contains("reservation_raw_canary"));
        assert!(!rendered_log.contains("/Users/shravansunder"));
    }

    #[test]
    fn emitted_snapshot_freshness_log_excludes_forbidden_values() {
        let rendered_log = capture_log_output(|| {
            super::emit_snapshot_freshness_observed(
                "selection_projection",
                "responses",
                "stale",
                "last_known_good",
                30,
            );
        });

        assert!(rendered_log.contains("codex_router.snapshot_freshness_observed"));
        assert!(rendered_log.contains("selection_projection"));
        assert!(rendered_log.contains("responses"));
        assert!(rendered_log.contains("stale"));
        assert!(rendered_log.contains("last_known_good"));
        assert!(!rendered_log.contains("raw-provider-body-canary"));
        assert!(!rendered_log.contains("sk-live-token-canary"));
        assert!(!rendered_log.contains("Authorization"));
        assert!(!rendered_log.contains("acct_raw_canary"));
        assert!(!rendered_log.contains("friendly account label"));
        assert!(!rendered_log.contains("reservation_raw_canary"));
        assert!(!rendered_log.contains("/Users/shravansunder"));
    }

    #[test]
    fn emitted_maintenance_lag_log_excludes_forbidden_values() {
        let rendered_log = capture_log_output(|| {
            super::emit_maintenance_lag_observed(
                "active_session_rollup_refresh",
                "responses",
                "degraded",
                1_500,
            );
        });

        assert!(rendered_log.contains("codex_router.maintenance_lag_observed"));
        assert!(rendered_log.contains("active_session_rollup_refresh"));
        assert!(rendered_log.contains("responses"));
        assert!(rendered_log.contains("degraded"));
        assert!(!rendered_log.contains("raw-provider-body-canary"));
        assert!(!rendered_log.contains("sk-live-token-canary"));
        assert!(!rendered_log.contains("Authorization"));
        assert!(!rendered_log.contains("acct_raw_canary"));
        assert!(!rendered_log.contains("friendly account label"));
        assert!(!rendered_log.contains("reservation_raw_canary"));
        assert!(!rendered_log.contains("/Users/shravansunder"));
    }
}
