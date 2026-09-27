//! Apply one advertised setting while its Session actor is idle.

use agent_client_protocol::{ActiveSession, Agent};

use crate::agent_session_client::ExternalProviderRuntimeError;
use crate::agent_session_client::provider_setting_application::{
    SettingSetupFailure, apply_initial_settings,
};
use crate::provider_session_actor::ProviderSessionRuntimeHandles;
use crate::{
    EffectiveProviderSettings, InvalidSettingSessionDisposition, ProviderSettingKind,
    RequestedProviderSettings,
};
use session_event_model::SessionEvent;

pub(crate) async fn apply_loaded_setting(
    session: &ActiveSession<'_, Agent>,
    kind: ProviderSettingKind,
    value: String,
    handles: &ProviderSessionRuntimeHandles,
) -> Result<EffectiveProviderSettings, ExternalProviderRuntimeError> {
    let provider_session_id = session.session_id().to_string();
    let mut catalog = handles
        .session_settings
        .read()
        .await
        .get(&provider_session_id)
        .cloned()
        .ok_or(ExternalProviderRuntimeError::LocalNotFound)?;
    let mut requested = RequestedProviderSettings::default();
    match kind {
        ProviderSettingKind::Mode => requested.mode = Some(value.clone()),
        ProviderSettingKind::Model => requested.model = Some(value.clone()),
        ProviderSettingKind::Effort => requested.effort = Some(value.clone()),
    }
    let (_, failure) = apply_initial_settings(session, &requested, &mut catalog).await;
    match failure {
        Some(SettingSetupFailure::Invalid { advertised, .. }) => {
            Err(ExternalProviderRuntimeError::InvalidSetting {
                setting: kind,
                value,
                advertised,
                provider_session_id,
                disposition: InvalidSettingSessionDisposition::RemainsCreated,
            })
        }
        Some(SettingSetupFailure::Agent { reason, .. }) => {
            Err(ExternalProviderRuntimeError::SettingFailed {
                setting: kind,
                value,
                reason,
            })
        }
        Some(SettingSetupFailure::Uncertain { .. }) => {
            handles
                .settings_unresolved
                .write()
                .await
                .insert(provider_session_id.clone(), kind);
            Err(ExternalProviderRuntimeError::SettingOutcomeUnknown {
                provider_session_id,
                setting: kind,
                value,
            })
        }
        None => {
            let effective = catalog.effective_settings();
            handles
                .event_sink
                .publish(
                    &provider_session_id,
                    SessionEvent::SettingsChanged {
                        settings: catalog.to_session_settings(),
                    },
                )
                .map_err(|_| ExternalProviderRuntimeError::SinkClosed)?;
            handles
                .session_settings
                .write()
                .await
                .insert(provider_session_id.clone(), catalog.clone());
            *handles.last_settings_catalog.write().await = Some(catalog);
            let mut unresolved = handles.settings_unresolved.write().await;
            if unresolved.get(&provider_session_id) == Some(&kind) {
                unresolved.remove(&provider_session_id);
            }
            drop(unresolved);
            if let Some(capabilities) = handles
                .session_capabilities
                .read()
                .await
                .get(&provider_session_id)
                .map(crate::ProviderCapabilityReport::to_session_model)
            {
                handles
                    .event_sink
                    .publish(
                        &provider_session_id,
                        SessionEvent::CapabilitiesChanged { capabilities },
                    )
                    .map_err(|_| ExternalProviderRuntimeError::SinkClosed)?;
            }
            Ok(effective)
        }
    }
}
