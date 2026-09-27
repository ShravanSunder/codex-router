//! Apply requested settings against each newly advertised ACP option set.

use agent_client_protocol::schema::v1::{
    SessionConfigOption, SessionConfigOptionValue, SetSessionConfigOptionRequest,
    SetSessionModeRequest,
};
use agent_client_protocol::{ConnectionTo, JsonRpcMessage, UntypedMessage};

use super::*;
use crate::{
    AppliedProviderSetting, ProviderSettingKind, ProviderSettingsCatalog, RequestedProviderSettings,
};

pub(crate) enum SettingSetupFailure {
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
    Uncertain {
        kind: ProviderSettingKind,
        value: String,
    },
}

enum SettingResponseFailure {
    Rejected(String),
    Unknown,
}

pub(crate) async fn apply_initial_settings(
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
            match request.to_untyped_message() {
                Ok(request) => match send_setting_request(session.connection(), request).await {
                    Ok(response) => apply_config_response(catalog, kind, value, response),
                    Err(failure) => Err(failure),
                },
                Err(_) => Err(SettingResponseFailure::Unknown),
            }
        } else if kind == ProviderSettingKind::Mode && !catalog.modes.is_empty() {
            let request = SetSessionModeRequest::new(session.session_id().clone(), value.clone());
            let result = match request.to_untyped_message() {
                Ok(request) => send_setting_request(session.connection(), request)
                    .await
                    .and_then(|response| {
                        response
                            .is_object()
                            .then_some(())
                            .ok_or(SettingResponseFailure::Unknown)
                    }),
                Err(_) => Err(SettingResponseFailure::Unknown),
            };
            if result.is_ok() {
                catalog.current_mode = Some(value.clone());
            }
            result
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
        match result {
            Ok(()) => {}
            Err(SettingResponseFailure::Rejected(reason)) => {
                return (
                    applied,
                    Some(SettingSetupFailure::Agent {
                        kind,
                        value: value.clone(),
                        reason,
                    }),
                );
            }
            Err(SettingResponseFailure::Unknown) => {
                return (
                    applied,
                    Some(SettingSetupFailure::Uncertain {
                        kind,
                        value: value.clone(),
                    }),
                );
            }
        }
        applied.push(AppliedProviderSetting {
            kind,
            value: value.clone(),
        });
    }
    (applied, None)
}

fn apply_config_response(
    catalog: &mut ProviderSettingsCatalog,
    kind: ProviderSettingKind,
    value: &str,
    response: serde_json::Value,
) -> Result<(), SettingResponseFailure> {
    let options = response
        .get("configOptions")
        .and_then(serde_json::Value::as_array)
        .ok_or(SettingResponseFailure::Unknown)?;
    let options = options
        .iter()
        .cloned()
        .map(serde_json::from_value::<SessionConfigOption>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| SettingResponseFailure::Unknown)?;
    let mut updated = catalog.clone();
    crate::provider_settings_catalog_codec::replace_catalog_config_options(&mut updated, &options);
    let effective = updated.effective_settings();
    let selected = match kind {
        ProviderSettingKind::Mode => effective.mode.as_deref(),
        ProviderSettingKind::Model => effective.model.as_deref(),
        ProviderSettingKind::Effort => effective.effort.as_deref(),
    };
    if selected != Some(value) {
        return Err(SettingResponseFailure::Unknown);
    }
    *catalog = updated;
    Ok(())
}

async fn send_setting_request(
    connection: &ConnectionTo<Agent>,
    request: UntypedMessage,
) -> Result<serde_json::Value, SettingResponseFailure> {
    let (reply, result) = tokio::sync::oneshot::channel();
    connection
        .send_request(request)
        .on_receiving_result(async move |response| {
            let _result = reply.send(response);
            Ok(())
        })
        .map_err(|_| SettingResponseFailure::Unknown)?;
    match result.await.map_err(|_| SettingResponseFailure::Unknown)? {
        Ok(response) => Ok(response),
        Err(error) if agent_client_protocol::is_incoming_transport_closed(&error) => {
            Err(SettingResponseFailure::Unknown)
        }
        Err(error) => Err(SettingResponseFailure::Rejected(
            acp_operation_error(error).to_string(),
        )),
    }
}
