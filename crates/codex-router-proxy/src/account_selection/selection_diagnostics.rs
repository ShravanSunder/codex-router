use codex_router_core::routes::RouteBand;
use codex_router_state::sqlite::StateStoreError;

use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::http_sse::HttpProxyError;
use crate::session_account_affinity_cache::SessionAccountAffinityPublicationError;

#[derive(Clone, Copy)]
pub(crate) enum SelectionDiagnosticStage {
    QueueHealth,
    ActiveReservations,
    PersistedProjection,
    RuntimeQuarantine,
    PreviousResponseAffinity,
    SessionAffinity,
    WeightedSelector,
    AccountHold,
    Assessment,
    Selection,
    Reservation,
    Publication,
    HttpAccountAttemptLimit,
    HttpReplayUnavailable,
    HttpPrecommitObservation,
}

impl SelectionDiagnosticStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::QueueHealth => "queue_health",
            Self::ActiveReservations => "active_reservations",
            Self::PersistedProjection => "persisted_projection",
            Self::RuntimeQuarantine => "runtime_quarantine",
            Self::PreviousResponseAffinity => "previous_response_affinity",
            Self::SessionAffinity => "session_affinity",
            Self::WeightedSelector => "weighted_selector",
            Self::AccountHold => "account_hold",
            Self::Assessment => "assessment",
            Self::Selection => "selection",
            Self::Reservation => "reservation",
            Self::Publication => "publication",
            Self::HttpAccountAttemptLimit => "http_account_attempt_limit",
            Self::HttpReplayUnavailable => "http_replay_unavailable",
            Self::HttpPrecommitObservation => "http_precommit_observation",
        }
    }
}

pub(crate) fn record_selection_rejected(
    stage: SelectionDiagnosticStage,
    error_class: &'static str,
    route_band: Option<RouteBand>,
) {
    if let Some(route_band) = route_band {
        tracing::warn!(
            selection.stage = stage.as_str(),
            error.class = error_class,
            route.band = route_band.as_str(),
            "codex_router.selection_rejected"
        );
    } else {
        tracing::warn!(
            selection.stage = stage.as_str(),
            error.class = error_class,
            "codex_router.selection_rejected"
        );
    }
}

pub(crate) fn record_selection_error_on_failure<TOutput>(
    result: Result<TOutput, HttpProxyError>,
    stage: SelectionDiagnosticStage,
    route_band: RouteBand,
) -> Result<TOutput, HttpProxyError> {
    result.inspect_err(|error| {
        record_selection_rejected(stage, selection_error_class(error), Some(route_band));
    })
}

pub(crate) fn record_selection_error(
    error: HttpProxyError,
    stage: SelectionDiagnosticStage,
    route_band: RouteBand,
) -> HttpProxyError {
    record_selection_rejected(stage, selection_error_class(&error), Some(route_band));
    error
}

pub(crate) fn selection_error(
    stage: SelectionDiagnosticStage,
    error_class: &'static str,
    reason: QuotaAwareAccountSelectorError,
    route_band: RouteBand,
) -> HttpProxyError {
    record_selection_rejected(stage, error_class, Some(route_band));
    HttpProxyError::Selection { reason }
}

pub(crate) fn record_selection_error_with_class_on_failure<TOutput>(
    result: Result<TOutput, HttpProxyError>,
    stage: SelectionDiagnosticStage,
    error_class: &'static str,
    route_band: RouteBand,
) -> Result<TOutput, HttpProxyError> {
    result.inspect_err(|_error| record_selection_rejected(stage, error_class, Some(route_band)))
}

pub(crate) fn state_store_selection_error(
    error: &StateStoreError,
    stage: SelectionDiagnosticStage,
    route_band: RouteBand,
) -> HttpProxyError {
    record_selection_rejected(stage, state_store_error_class(error), Some(route_band));
    HttpProxyError::Selection {
        reason: QuotaAwareAccountSelectorError::StateUnavailable,
    }
}

pub(crate) fn state_store_result_on_failure<TOutput>(
    result: Result<TOutput, StateStoreError>,
    stage: SelectionDiagnosticStage,
    route_band: RouteBand,
) -> Result<TOutput, HttpProxyError> {
    result.map_err(|error| state_store_selection_error(&error, stage, route_band))
}

pub(crate) fn runtime_quarantine_selection_error(
    _error: StateStoreError,
    route_band: RouteBand,
) -> HttpProxyError {
    record_selection_rejected(
        SelectionDiagnosticStage::RuntimeQuarantine,
        "lock_poisoned",
        Some(route_band),
    );
    HttpProxyError::Selection {
        reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
    }
}

pub(crate) fn selector_mutex_selection_error<TError>(
    _error: TError,
    stage: SelectionDiagnosticStage,
    route_band: RouteBand,
) -> HttpProxyError {
    record_selection_rejected(stage, "lock_poisoned", Some(route_band));
    HttpProxyError::Selection {
        reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
    }
}

pub(crate) fn session_affinity_publication_selection_error(
    error: &SessionAccountAffinityPublicationError,
    route_band: RouteBand,
) -> HttpProxyError {
    record_selection_rejected(
        SelectionDiagnosticStage::SessionAffinity,
        session_affinity_publication_error_class(error),
        Some(route_band),
    );
    HttpProxyError::Selection {
        reason: QuotaAwareAccountSelectorError::StateUnavailable,
    }
}

pub(crate) fn session_affinity_publication_result_on_failure<TOutput>(
    result: Result<TOutput, SessionAccountAffinityPublicationError>,
    route_band: RouteBand,
) -> Result<TOutput, HttpProxyError> {
    result.map_err(|error| session_affinity_publication_selection_error(&error, route_band))
}

pub(crate) fn session_cache_selection_error(
    _error: crate::session_account_affinity_cache::SessionAccountAffinityCacheUnavailable,
    route_band: RouteBand,
) -> HttpProxyError {
    record_selection_rejected(
        SelectionDiagnosticStage::SessionAffinity,
        "lock_poisoned",
        Some(route_band),
    );
    HttpProxyError::Selection {
        reason: QuotaAwareAccountSelectorError::StateUnavailable,
    }
}

pub(crate) fn session_cache_result_on_failure<TOutput>(
    result: Result<
        TOutput,
        crate::session_account_affinity_cache::SessionAccountAffinityCacheUnavailable,
    >,
    route_band: RouteBand,
) -> Result<TOutput, HttpProxyError> {
    result.map_err(|error| session_cache_selection_error(error, route_band))
}

pub(crate) fn state_store_error_class(error: &StateStoreError) -> &'static str {
    match error {
        StateStoreError::Sqlite { .. } => "sqlite",
        StateStoreError::UnsupportedSchemaVersion { .. }
        | StateStoreError::MissingReadOnlySchemaObject { .. } => "unsupported_schema",
        StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
        | StateStoreError::AccountStatusSchemaUpgradeRequired
        | StateStoreError::CreditUsagePolicySchemaUpgradeRequired => "schema_upgrade_required",
        StateStoreError::WeeklyQuotaFloorDatabaseBusy
        | StateStoreError::AccountStatusDatabaseBusy
        | StateStoreError::CreditUsagePolicyDatabaseBusy => "database_busy",
        StateStoreError::WeeklyQuotaFloorAccountNotFound
        | StateStoreError::AccountStatusAccountNotFound
        | StateStoreError::CreditUsagePolicyAccountUnavailable
        | StateStoreError::AccountWindowStateAccountNotFound => "account_unavailable",
        StateStoreError::WeeklyQuotaFloorAccountLabelAmbiguous
        | StateStoreError::AccountStatusAccountLabelAmbiguous => "ambiguous_account_label",
        StateStoreError::AccountProviderImmutable => "provider_immutable",
        StateStoreError::CorruptAccountRoutingPolicy => "corrupt_account_routing_policy",
        StateStoreError::CorruptAccount { .. } => "corrupt_account",
        StateStoreError::CorruptSessionAccountAffinity { .. } => "corrupt_session_affinity",
        StateStoreError::CorruptQuotaSnapshot { .. } => "corrupt_quota_snapshot",
        StateStoreError::AccountConcurrentModification { .. }
        | StateStoreError::CreditUsagePolicyTargetChanged => "concurrent_modification",
        StateStoreError::CreditRefreshAttemptSequenceOverflow => "numeric_overflow",
        StateStoreError::InvalidCreditRefreshInput { .. }
        | StateStoreError::InvalidAccountWindowState { .. } => "invalid_state",
        StateStoreError::CorruptAccountWindowState { .. } => "corrupt_account_window_state",
        StateStoreError::AccountWindowStateRequiresClaudeAccount => "provider_mismatch",
    }
}

pub(crate) fn session_affinity_publication_error_class(
    error: &SessionAccountAffinityPublicationError,
) -> &'static str {
    match error {
        SessionAccountAffinityPublicationError::CacheUnavailable => "lock_poisoned",
        SessionAccountAffinityPublicationError::Persistence(error) => {
            state_store_error_class(error)
        }
    }
}

pub(crate) fn selection_error_class(error: &HttpProxyError) -> &'static str {
    match error {
        HttpProxyError::Selection { reason } => match reason {
            QuotaAwareAccountSelectorError::NoEligibleAccounts => "no_eligible_accounts",
            QuotaAwareAccountSelectorError::ShortQuotaExhausted { .. } => "short_quota_exhausted",
            QuotaAwareAccountSelectorError::SelectorStateUnavailable => "selector_state",
            QuotaAwareAccountSelectorError::StateUnavailable => "state_unavailable",
            QuotaAwareAccountSelectorError::SecretUnavailable => "affinity_secret_unavailable",
            QuotaAwareAccountSelectorError::MalformedAffinityKey => "malformed_affinity_key",
            QuotaAwareAccountSelectorError::AffinityOwnerMissing => "affinity_owner_missing",
            QuotaAwareAccountSelectorError::AffinityOwnerUnavailable => {
                "affinity_owner_unavailable"
            }
        },
        HttpProxyError::LocalAuth { .. } => "local_auth",
        HttpProxyError::Rejected { .. } => "request_rejected",
        HttpProxyError::Upstream { .. } => "upstream",
        HttpProxyError::ProviderCredential { .. } => "provider_credential",
    }
}
