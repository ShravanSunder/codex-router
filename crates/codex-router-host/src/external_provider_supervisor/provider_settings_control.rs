//! Immediate settings actions use the durable Session inventory for authority.

use super::ExternalProviderSupervisor;
use crate::ExternalProviderRuntimeError;
use crate::provider_operation_settlement::effective_settings;
use collaboration_protocol::{
    ProviderSettingName, ProviderSettingsAcceptRequest, ProviderSettingsFailure,
    ProviderSettingsFailureKind, ProviderSettingsMappingStatus, ProviderSettingsResult,
    ProviderSettingsSetRequest, SessionRef,
};
use collaboration_service::ProviderSessionRecord;

#[allow(clippy::result_large_err)]
pub(super) async fn set(
    supervisor: &ExternalProviderSupervisor,
    request: ProviderSettingsSetRequest,
) -> Result<ProviderSettingsResult, ProviderSettingsFailure> {
    let record = authorized_session(supervisor, &request.target, &request.actor).await?;
    if request.value.trim().is_empty() {
        return Err(failure(
            ProviderSettingsFailureKind::InvalidSetting,
            &request.target,
            "setting value must be nonempty",
        ));
    }
    let runtime = supervisor
        .runtime_for(&request.target.endpoint)
        .ok_or_else(|| {
            failure(
                ProviderSettingsFailureKind::Unavailable,
                &request.target,
                "provider connection is unavailable",
            )
        })?;
    let kind = match request.setting {
        ProviderSettingName::Mode => acp_client_runtime::ProviderSettingKind::Mode,
        ProviderSettingName::Model => acp_client_runtime::ProviderSettingKind::Model,
        ProviderSettingName::Effort => acp_client_runtime::ProviderSettingKind::Effort,
    };
    let observed = runtime
        .set_setting(
            String::from(request.target.session_id.clone()),
            kind,
            request.value.clone(),
        )
        .await
        .map_err(|error| setting_error(&request, error))?;
    Ok(result(request.target, record, observed))
}

#[allow(clippy::result_large_err)]
pub(super) async fn accept(
    supervisor: &ExternalProviderSupervisor,
    request: ProviderSettingsAcceptRequest,
) -> Result<ProviderSettingsResult, ProviderSettingsFailure> {
    let record = authorized_session(supervisor, &request.target, &request.actor).await?;
    let runtime = supervisor
        .runtime_for(&request.target.endpoint)
        .ok_or_else(|| {
            failure(
                ProviderSettingsFailureKind::Unavailable,
                &request.target,
                "provider connection is unavailable",
            )
        })?;
    let observed = runtime
        .accept_session_settings(String::from(request.target.session_id.clone()))
        .await
        .map_err(|error| plain_error(&request.target, error))?;
    Ok(result(request.target, record, observed))
}

#[allow(clippy::result_large_err)]
async fn authorized_session(
    supervisor: &ExternalProviderSupervisor,
    target: &SessionRef,
    actor: &SessionRef,
) -> Result<ProviderSessionRecord, ProviderSettingsFailure> {
    let record = supervisor
        .inner
        .store
        .lock()
        .await
        .session_record(target)
        .await
        .map_err(|_| {
            failure(
                ProviderSettingsFailureKind::Unavailable,
                target,
                "provider Session inventory is unavailable",
            )
        })?
        .ok_or_else(|| {
            failure(
                ProviderSettingsFailureKind::NotFound,
                target,
                "provider Session is unknown",
            )
        })?;
    if actor != &record.created_by && actor != &record.approver {
        return Err(failure(
            ProviderSettingsFailureKind::WrongActor,
            target,
            "only the creator or Approver may change Session settings",
        ));
    }
    Ok(record)
}

fn result(
    target: SessionRef,
    record: ProviderSessionRecord,
    observed: acp_client_runtime::EffectiveProviderSettings,
) -> ProviderSettingsResult {
    let mut effective = effective_settings(record.requested_policy);
    effective.mapping_status = ProviderSettingsMappingStatus::Verified;
    effective.mode = observed.mode;
    effective.model = observed.model;
    effective.effort = observed.effort;
    ProviderSettingsResult {
        target,
        effective_settings: effective,
    }
}

fn setting_error(
    request: &ProviderSettingsSetRequest,
    error: ExternalProviderRuntimeError,
) -> ProviderSettingsFailure {
    match error {
        ExternalProviderRuntimeError::InvalidSetting { advertised, .. } => {
            ProviderSettingsFailure {
                kind: ProviderSettingsFailureKind::InvalidSetting,
                target: request.target.clone(),
                message: "setting value was not advertised by the provider".into(),
                setting: Some(request.setting),
                value: Some(request.value.clone()),
                advertised,
            }
        }
        ExternalProviderRuntimeError::SettingFailed { .. } => ProviderSettingsFailure {
            kind: ProviderSettingsFailureKind::ProviderRejected,
            target: request.target.clone(),
            message: "provider rejected the setting; current reported values remain effective"
                .into(),
            setting: Some(request.setting),
            value: Some(request.value.clone()),
            advertised: Vec::new(),
        },
        ExternalProviderRuntimeError::SettingOutcomeUnknown { .. } => ProviderSettingsFailure {
            kind: ProviderSettingsFailureKind::OutcomeUnknown,
            target: request.target.clone(),
            message: "provider setting outcome is unknown; Session remains gated".into(),
            setting: Some(request.setting),
            value: Some(request.value.clone()),
            advertised: Vec::new(),
        },
        other => {
            let mut failure = plain_error(&request.target, other);
            if failure.kind == ProviderSettingsFailureKind::OutcomeUnknown {
                failure.setting = Some(request.setting);
                failure.value = Some(request.value.clone());
            }
            failure
        }
    }
}

fn plain_error(
    target: &SessionRef,
    error: ExternalProviderRuntimeError,
) -> ProviderSettingsFailure {
    let (kind, message) = match error {
        ExternalProviderRuntimeError::LocalBusy => (
            ProviderSettingsFailureKind::Busy,
            "provider Session has running work",
        ),
        ExternalProviderRuntimeError::LocalNotFound => (
            ProviderSettingsFailureKind::NotFound,
            "provider Session is not loaded",
        ),
        ExternalProviderRuntimeError::TransportFailure
        | ExternalProviderRuntimeError::FrameDecodeFailure
        | ExternalProviderRuntimeError::FrameLimitExceeded => (
            ProviderSettingsFailureKind::OutcomeUnknown,
            "provider setting outcome is unknown; Session remains gated",
        ),
        _ => (
            ProviderSettingsFailureKind::Unavailable,
            "provider settings action is unavailable",
        ),
    };
    failure(kind, target, message)
}

fn failure(
    kind: ProviderSettingsFailureKind,
    target: &SessionRef,
    message: &str,
) -> ProviderSettingsFailure {
    ProviderSettingsFailure {
        kind,
        target: target.clone(),
        message: message.into(),
        setting: None,
        value: None,
        advertised: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn post_send_transport_loss_names_unknown_setting_state() {
        let request: ProviderSettingsSetRequest = serde_json::from_value(json!({
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"provider-session"},
            "actor":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
            "setting":"mode","value":"ask"
        })).expect("set request");
        let failure = setting_error(&request, ExternalProviderRuntimeError::TransportFailure);
        assert_eq!(failure.kind, ProviderSettingsFailureKind::OutcomeUnknown);
        assert_eq!(failure.setting, Some(ProviderSettingName::Mode));
        assert_eq!(failure.value.as_deref(), Some("ask"));
        let precise = setting_error(
            &request,
            ExternalProviderRuntimeError::SettingOutcomeUnknown {
                provider_session_id: "provider-session".into(),
                setting: acp_client_runtime::ProviderSettingKind::Mode,
                value: "ask".into(),
            },
        );
        assert_eq!(precise.kind, ProviderSettingsFailureKind::OutcomeUnknown);
        assert_eq!(precise.setting, Some(ProviderSettingName::Mode));
    }
}
