use super::*;

const RUNTIME_QUOTA_EXHAUSTION_MAX_AGE_SECONDS: u64 = 300;

impl RuntimeQuotaExhaustion {
    pub(super) fn new(account_id: AccountId, observed_unix_seconds: u64) -> Self {
        Self {
            account_id,
            expires_unix_seconds: observed_unix_seconds
                .saturating_add(RUNTIME_QUOTA_EXHAUSTION_MAX_AGE_SECONDS),
        }
    }
}

impl RouteBandQueueDegradedState {
    fn new(reason: RouteBandQueueDegradedReason, observed_unix_seconds: u64) -> Self {
        Self {
            reason,
            observed_unix_seconds,
        }
    }

    /// Returns the degraded reason.
    #[must_use]
    pub const fn reason(&self) -> RouteBandQueueDegradedReason {
        self.reason
    }
}

pub(crate) fn route_band_queue_health_key(route_band: RouteBand, queue_name: &str) -> String {
    format!("{}:{queue_name}", route_band.as_str())
}

pub(crate) fn route_band_queue_health_key_prefix(route_band: RouteBand) -> String {
    format!("{}:", route_band.as_str())
}

pub(super) fn projected_accounts_excluding_runtime_exhaustions(
    accounts: Vec<BurnDownAccountInput>,
    runtime_exhaustions: &RouteBandRuntimeExhaustions,
    route_band: &str,
    now_unix_seconds: u64,
) -> Result<Vec<BurnDownAccountInput>, StateStoreError> {
    let mut runtime_exhaustions =
        runtime_exhaustions
            .lock()
            .map_err(|_error| StateStoreError::Sqlite {
                message: "runtime quota exhaustion state unavailable".to_owned(),
            })?;
    let Some(route_band_exhaustions) = runtime_exhaustions.get_mut(route_band) else {
        return Ok(accounts);
    };
    route_band_exhaustions.retain(|exhaustion| now_unix_seconds < exhaustion.expires_unix_seconds);
    if route_band_exhaustions.is_empty() {
        runtime_exhaustions.remove(route_band);
        return Ok(accounts);
    }

    Ok(accounts
        .into_iter()
        .filter(|account| {
            !route_band_exhaustions
                .iter()
                .any(|exhaustion| &exhaustion.account_id == account.account_id())
        })
        .collect())
}

/// Marks an account exhausted in process-local runtime state before durable persistence catches up.
pub fn mark_runtime_quota_exhausted(
    runtime_exhaustions: &RouteBandRuntimeExhaustions,
    route_band: RouteBand,
    account_id: AccountId,
    observed_unix_seconds: u64,
) -> Result<(), StateStoreError> {
    let mut runtime_exhaustions =
        runtime_exhaustions
            .lock()
            .map_err(|_error| StateStoreError::Sqlite {
                message: "runtime quota exhaustion state unavailable".to_owned(),
            })?;
    let route_band_exhaustions = runtime_exhaustions
        .entry(route_band.as_str().to_owned())
        .or_insert_with(Vec::new);
    route_band_exhaustions.retain(|exhaustion| exhaustion.account_id != account_id);
    route_band_exhaustions.push(RuntimeQuotaExhaustion::new(
        account_id,
        observed_unix_seconds,
    ));

    Ok(())
}

/// Marks a route band degraded because queue-backed mirror/proof writes are unhealthy.
pub fn mark_route_band_queue_degraded(
    route_band_queue_health: &RouteBandQueueHealth,
    route_band: RouteBand,
    reason: RouteBandQueueDegradedReason,
    observed_unix_seconds: u64,
) -> Result<(), StateStoreError> {
    mark_route_band_queue_degraded_for_queue(
        route_band_queue_health,
        route_band,
        "route_band",
        reason,
        observed_unix_seconds,
    )
}

pub(crate) fn mark_route_band_queue_degraded_for_queue(
    route_band_queue_health: &RouteBandQueueHealth,
    route_band: RouteBand,
    queue_name: &'static str,
    reason: RouteBandQueueDegradedReason,
    observed_unix_seconds: u64,
) -> Result<(), StateStoreError> {
    let mut queue_health =
        route_band_queue_health
            .lock()
            .map_err(|_error| StateStoreError::Sqlite {
                message: "route-band queue health unavailable".to_owned(),
            })?;
    queue_health.insert(
        route_band_queue_health_key(route_band, queue_name),
        RouteBandQueueDegradedState::new(reason, observed_unix_seconds),
    );
    Ok(())
}

/// Clears route-band queue degraded state after an explicit successful health signal.
pub fn clear_route_band_queue_degraded(
    route_band_queue_health: &RouteBandQueueHealth,
    route_band: RouteBand,
) -> Result<(), StateStoreError> {
    let mut queue_health =
        route_band_queue_health
            .lock()
            .map_err(|_error| StateStoreError::Sqlite {
                message: "route-band queue health unavailable".to_owned(),
            })?;
    let prefix = route_band_queue_health_key_prefix(route_band);
    queue_health.retain(|key, _state| key != route_band.as_str() && !key.starts_with(&prefix));
    Ok(())
}

pub(crate) fn clear_route_band_queue_degraded_for_queue(
    route_band_queue_health: &RouteBandQueueHealth,
    route_band: RouteBand,
    queue_name: &'static str,
) -> Result<(), StateStoreError> {
    let mut queue_health =
        route_band_queue_health
            .lock()
            .map_err(|_error| StateStoreError::Sqlite {
                message: "route-band queue health unavailable".to_owned(),
            })?;
    queue_health.remove(&route_band_queue_health_key(route_band, queue_name));
    Ok(())
}

pub(crate) fn route_band_queue_health_allows_selection(
    route_band_queue_health: &RouteBandQueueHealth,
    route_band: RouteBand,
) -> Result<(), StateStoreError> {
    let queue_health = route_band_queue_health.lock().map_err(|_error| {
        crate::telemetry::emit_snapshot_freshness_observed(
            "queue_health",
            route_band.as_str(),
            "unavailable",
            "fail_closed",
            0,
        );
        StateStoreError::Sqlite {
            message: "route-band queue health unavailable".to_owned(),
        }
    })?;
    let prefix = route_band_queue_health_key_prefix(route_band);
    if queue_health.contains_key(route_band.as_str())
        || queue_health.keys().any(|key| key.starts_with(&prefix))
    {
        crate::telemetry::emit_snapshot_freshness_observed(
            "queue_health",
            route_band.as_str(),
            "unavailable",
            "fail_closed",
            0,
        );
        return Err(StateStoreError::Sqlite {
            message: "route-band queue health degraded".to_owned(),
        });
    }
    Ok(())
}
