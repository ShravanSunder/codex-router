//! Privacy-bounded lifecycle telemetry emitted through the existing subscriber.

use std::time::Duration;

use crate::ExecutableRelation;
use crate::HostOperation;
use crate::HostedReadiness;
use crate::RecoveryBudget;
use crate::RouterCondition;

pub(crate) fn record_lifecycle(
    operation: HostOperation,
    result: &'static str,
    duration: Duration,
    router: RouterCondition,
    readiness: HostedReadiness,
    recovery_budget: RecoveryBudget,
    executable_relation: ExecutableRelation,
) {
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

const fn router_ownership(router: RouterCondition) -> &'static str {
    match router {
        RouterCondition::ExternalReachable => "external",
        RouterCondition::OwnedReachable | RouterCondition::OwnedTransitioning => "owned",
        RouterCondition::Unavailable => "unavailable",
    }
}
