use opentelemetry::{KeyValue, global};
use sha2::{Digest, Sha256};
pub fn telemetry_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

pub(crate) fn record_quota_refresh_metric(
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

pub(crate) fn record_claude_usage_auth_rejected_after_renewal() {
    global::meter("codex-router")
        .u64_counter("codex_router_claude_usage_auth_rejected_after_renewal_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("provider", "claude"),
                KeyValue::new("route_band", "claude_messages"),
            ],
        );
}
