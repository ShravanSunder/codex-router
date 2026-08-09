//! Privacy-bounded lifecycle telemetry emitted through the existing subscriber.

use std::sync::OnceLock;
use std::time::Duration;

use crate::ExecutableRelation;
use crate::HostOperation;
use crate::HostedReadiness;
use crate::RecoveryBudget;
use crate::RemoteControlCondition;
use crate::RemoteControlIdentity;
use crate::RouterCondition;
use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use opentelemetry::metrics::Histogram;

pub(crate) fn record_lifecycle(
    operation: HostOperation,
    result: &'static str,
    duration: Duration,
    router: RouterCondition,
    readiness: HostedReadiness,
    recovery_budget: RecoveryBudget,
    executable_relation: ExecutableRelation,
) {
    let attributes = [
        KeyValue::new("operation", operation_name(operation)),
        KeyValue::new("result", result),
        KeyValue::new("router.ownership", router_ownership(router)),
        KeyValue::new("host.readiness", readiness_name(readiness)),
        KeyValue::new("recovery.budget", recovery_budget_name(recovery_budget)),
        KeyValue::new(
            "executable.relation",
            executable_relation_name(executable_relation),
        ),
    ];
    lifecycle_counter().add(1, &attributes);
    lifecycle_duration().record(duration.as_secs_f64() * 1_000.0, &attributes);
    tracing::info!(
        event.name = "codex_router.host.lifecycle",
        operation = ?operation,
        result,
        duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        router_ownership = router_ownership(router),
        readiness = ?readiness,
        recovery_budget = ?recovery_budget,
        executable_relation = ?executable_relation,
    );
}

pub(crate) fn record_remote_control_observation(
    condition: RemoteControlCondition,
    identity: &RemoteControlIdentity,
) {
    tracing::info!(
        event.name = "codex_router.host.remote_control_observation",
        remote.condition = remote_control_condition_name(condition),
        remote.server.name = %bounded_identity(identity.server_name()),
        remote.environment.id = %bounded_identity(identity.environment_id().unwrap_or("unassigned")),
        "observed upstream Remote Control identity"
    );
}

fn lifecycle_counter() -> &'static Counter<u64> {
    static COUNTER: OnceLock<Counter<u64>> = OnceLock::new();
    COUNTER.get_or_init(|| {
        opentelemetry::global::meter("codex-router-host")
            .u64_counter("codex_router_host_lifecycle_total")
            .with_description("Count of completed host lifecycle operations")
            .build()
    })
}

fn lifecycle_duration() -> &'static Histogram<f64> {
    static HISTOGRAM: OnceLock<Histogram<f64>> = OnceLock::new();
    HISTOGRAM.get_or_init(|| {
        opentelemetry::global::meter("codex-router-host")
            .f64_histogram("codex_router_host_lifecycle_duration_ms")
            .with_unit("ms")
            .with_description("Duration of completed host lifecycle operations")
            .build()
    })
}

const fn operation_name(operation: HostOperation) -> &'static str {
    match operation {
        HostOperation::Start => "start",
        HostOperation::Status => "status",
        HostOperation::RestartAppServer => "restart_app_server",
        HostOperation::RestartRouter => "restart_router",
        HostOperation::UpdateCodex => "update_codex",
    }
}

const fn readiness_name(readiness: HostedReadiness) -> &'static str {
    match readiness {
        HostedReadiness::Ready => "ready",
        HostedReadiness::LocalReadyRemoteDegraded => "local_ready_remote_degraded",
        HostedReadiness::Unavailable => "unavailable",
    }
}

const fn recovery_budget_name(recovery_budget: RecoveryBudget) -> &'static str {
    match recovery_budget {
        RecoveryBudget::Available => "available",
        RecoveryBudget::Consumed => "consumed",
    }
}

const fn executable_relation_name(relation: ExecutableRelation) -> &'static str {
    match relation {
        ExecutableRelation::Match => "match",
        ExecutableRelation::Drift => "drift",
        ExecutableRelation::Unknown => "unknown",
    }
}

const fn remote_control_condition_name(condition: RemoteControlCondition) -> &'static str {
    match condition {
        RemoteControlCondition::Connected => "connected",
        RemoteControlCondition::Connecting => "connecting",
        RemoteControlCondition::Errored => "errored",
        RemoteControlCondition::Disabled => "disabled",
        RemoteControlCondition::Unavailable => "unavailable",
    }
}

fn bounded_identity(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect()
}

const fn router_ownership(router: RouterCondition) -> &'static str {
    match router {
        RouterCondition::ExternalReachable => "external",
        RouterCondition::OwnedReachable | RouterCondition::OwnedTransitioning => "owned",
        RouterCondition::Unavailable => "unavailable",
    }
}
