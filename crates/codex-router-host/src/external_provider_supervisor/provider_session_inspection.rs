//! Inspect one durable provider Session with its live connection observations.

use super::ExternalProviderSupervisor;
use collaboration_protocol::{
    ProviderConfigOptionView, ProviderConfigValueView, ProviderHistoryAvailability,
    ProviderInspectFailure, ProviderInspectFailureKind, ProviderSessionInspectRequest,
    ProviderSessionInspectResult, ProviderSessionState, ProviderSettingChoiceView,
    ProviderSettingsCatalogView, SessionRef,
};
use collaboration_service::SessionEventHub;
use session_event_model::SessionState;

#[allow(clippy::result_large_err)]
pub(super) async fn inspect(
    supervisor: &ExternalProviderSupervisor,
    request: ProviderSessionInspectRequest,
) -> Result<ProviderSessionInspectResult, ProviderInspectFailure> {
    let target = request.target;
    supervisor
        .inner
        .store
        .lock()
        .await
        .session_record(&target)
        .await
        .map_err(|_| failure(ProviderInspectFailureKind::Unavailable, &target))?
        .ok_or_else(|| failure(ProviderInspectFailureKind::NotFound, &target))?;
    let runtime = supervisor
        .runtime_for(&target.endpoint)
        .ok_or_else(|| failure(ProviderInspectFailureKind::Unavailable, &target))?;
    let session_id = String::from(target.session_id.clone());
    let capabilities = runtime
        .capability_report(&session_id)
        .await
        .to_session_model();
    let catalog = runtime.settings_catalog(&session_id).await;
    if let Some(catalog) = &catalog {
        supervisor
            .inner
            .settings_catalogs
            .lock()
            .await
            .insert(target.clone(), catalog.clone());
    }
    let catalog = match catalog {
        Some(catalog) => Some(catalog),
        None => supervisor
            .inner
            .settings_catalogs
            .lock()
            .await
            .get(&target)
            .cloned(),
    };
    let state = if let Some(hub) = supervisor.inner.hub.as_ref() {
        let hub_target = serde_json::to_value(&target)
            .ok()
            .and_then(|value| serde_json::from_value(value).ok())
            .ok_or_else(|| failure(ProviderInspectFailureKind::Unavailable, &target))?;
        hub.state(hub_target)
            .await
            .map_err(|_| failure(ProviderInspectFailureKind::Unavailable, &target))?
    } else {
        SessionState::Unloaded
    };
    let history = if matches!(state, SessionState::Unloaded)
        || supervisor
            .inner
            .history_unavailable
            .lock()
            .await
            .contains(&target)
    {
        ProviderHistoryAvailability::HistoryUnavailable
    } else {
        ProviderHistoryAvailability::Available
    };
    Ok(ProviderSessionInspectResult {
        target,
        state: state_tag(state),
        capabilities,
        history,
        settings_catalog: catalog.map(catalog_view),
    })
}

fn catalog_view(
    catalog: acp_client_runtime::ProviderSettingsCatalog,
) -> ProviderSettingsCatalogView {
    let current_mode = catalog.effective_settings().mode;
    ProviderSettingsCatalogView {
        current_mode,
        modes: catalog.modes.into_iter().map(choice_view).collect(),
        config_options: catalog
            .config_options
            .into_iter()
            .map(|option| ProviderConfigOptionView {
                id: option.id,
                name: option.name,
                category: option
                    .category
                    .map(crate::provider_operation_settlement::provider_setting_name),
                current_value: match option.current_value {
                    acp_client_runtime::ProviderConfigValue::Select(value) => {
                        ProviderConfigValueView::Select { value }
                    }
                    acp_client_runtime::ProviderConfigValue::Boolean(value) => {
                        ProviderConfigValueView::Boolean { value }
                    }
                },
                choices: option.choices.into_iter().map(choice_view).collect(),
            })
            .collect(),
    }
}

fn choice_view(choice: acp_client_runtime::ProviderSettingChoice) -> ProviderSettingChoiceView {
    ProviderSettingChoiceView {
        value: choice.value,
        label: choice.label,
    }
}

fn state_tag(state: SessionState) -> ProviderSessionState {
    match state {
        SessionState::Unloaded => ProviderSessionState::Unloaded,
        SessionState::Idle => ProviderSessionState::Idle,
        SessionState::Running => ProviderSessionState::Running,
        SessionState::RequiresAction { .. } => ProviderSessionState::RequiresAction,
        SessionState::AuthenticationRequired => ProviderSessionState::AuthenticationRequired,
        SessionState::Closed => ProviderSessionState::Closed,
    }
}

fn failure(kind: ProviderInspectFailureKind, target: &SessionRef) -> ProviderInspectFailure {
    ProviderInspectFailure {
        kind,
        target: target.clone(),
        message: match kind {
            ProviderInspectFailureKind::NotFound => "provider Session is unknown".into(),
            ProviderInspectFailureKind::Unavailable => {
                "provider Session inspection is unavailable".into()
            }
        },
    }
}
