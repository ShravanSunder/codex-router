//! Apply requested settings against each newly advertised ACP option set.

use agent_client_protocol::schema::v1::{
    SessionConfigOptionValue, SetSessionConfigOptionRequest, SetSessionModeRequest,
};

use super::*;
use crate::{
    AppliedProviderSetting, ProviderSettingKind, ProviderSettingsCatalog, RequestedProviderSettings,
};

pub(super) enum SettingSetupFailure {
    Invalid {
        kind: ProviderSettingKind,
        value: String,
        advertised: Vec<String>,
    },
    Agent {
        kind: ProviderSettingKind,
        value: String,
        reason: String,
    },
}

pub(super) async fn apply_initial_settings(
    session: &ActiveSession<'_, Agent>,
    requested: &RequestedProviderSettings,
    catalog: &mut ProviderSettingsCatalog,
) -> (Vec<AppliedProviderSetting>, Option<SettingSetupFailure>) {
    let mut applied = Vec::new();
    for (kind, requested_value) in [
        (ProviderSettingKind::Mode, requested.mode.as_ref()),
        (ProviderSettingKind::Model, requested.model.as_ref()),
        (ProviderSettingKind::Effort, requested.effort.as_ref()),
    ] {
        let Some(value) = requested_value else {
            continue;
        };
        let advertised = catalog.advertised_values(kind);
        if !advertised.iter().any(|offered| offered == value) {
            return (
                applied,
                Some(SettingSetupFailure::Invalid {
                    kind,
                    value: value.clone(),
                    advertised,
                }),
            );
        }

        let result = if let Some(option) = catalog.config_option(kind) {
            let request = SetSessionConfigOptionRequest::new(
                session.session_id().clone(),
                option.id.clone(),
                SessionConfigOptionValue::value_id(value.clone()),
            );
            match session
                .connection()
                .send_request_to(Agent, request)
                .block_task()
                .await
            {
                Ok(response) => {
                    crate::provider_settings_catalog_codec::replace_catalog_config_options(
                        catalog,
                        &response.config_options,
                    );
                    Ok(())
                }
                Err(error) => Err(error),
            }
        } else if kind == ProviderSettingKind::Mode && !catalog.modes.is_empty() {
            let result = session
                .connection()
                .send_request_to(
                    Agent,
                    SetSessionModeRequest::new(session.session_id().clone(), value.clone()),
                )
                .block_task()
                .await;
            if result.is_ok() {
                catalog.current_mode = Some(value.clone());
            }
            result.map(|_| ())
        } else {
            return (
                applied,
                Some(SettingSetupFailure::Invalid {
                    kind,
                    value: value.clone(),
                    advertised,
                }),
            );
        };
        if let Err(error) = result {
            return (
                applied,
                Some(SettingSetupFailure::Agent {
                    kind,
                    value: value.clone(),
                    reason: acp_operation_error(error).to_string(),
                }),
            );
        }
        applied.push(AppliedProviderSetting {
            kind,
            value: value.clone(),
        });
    }
    (applied, None)
}
