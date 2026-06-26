//! Scrubbed telemetry helpers for runtime routing metrics.

use opentelemetry::KeyValue;
use opentelemetry::global;

const METER_NAME: &str = "codex-router";

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
                KeyValue::new("route_band", route_band.to_owned()),
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

#[cfg(test)]
mod tests {
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
}
