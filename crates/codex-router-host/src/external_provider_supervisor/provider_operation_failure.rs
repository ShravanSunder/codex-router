//! Map provider admission and prompt loss into the operation failure contract.

use super::{failure, failure_with_provider_code};
use crate::ExternalProviderRuntimeError;
use collaboration_protocol::{
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, DeliveryCorrelationId, InvalidProviderSetting,
    InvalidSettingSessionDisposition, NonEmptyText, OperationId, ProviderOperationEffect,
    SessionRef,
};

const MAX_ADVERTISED_CHOICES: usize = 32;
const MAX_ADVERTISED_SUMMARY_BYTES: usize = 2048;
const MAX_SETTING_VALUE_DISPLAY_CHARS: usize = 120;

#[derive(Clone, Copy, Debug)]
struct ProviderErrorReference {
    provider_code: i64,
    correlation_id: acp_client_runtime::ProviderErrorCorrelationId,
}

struct CorrelatedProviderFailureProps {
    kind: ConversationOperationFailureKind,
    stage: ConversationOperationFailureStage,
    effect: ProviderOperationEffect,
    explanation: &'static str,
    operation_id: OperationId,
    target: Option<SessionRef>,
    provider_error: ProviderErrorReference,
}

pub(super) fn invalid_setting_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
    setting: acp_client_runtime::ProviderSettingKind,
    value: String,
    advertised: Vec<String>,
    disposition: acp_client_runtime::InvalidSettingSessionDisposition,
) -> ConversationOperationFailure {
    let (session_status, disposition, fallback_message) = match disposition {
        acp_client_runtime::InvalidSettingSessionDisposition::Closed => (
            "new Session was closed",
            InvalidSettingSessionDisposition::Closed,
            "invalid provider setting; see advertised values; new Session was closed",
        ),
        acp_client_runtime::InvalidSettingSessionDisposition::RemainsCreated => (
            "new Session remains created and idle",
            InvalidSettingSessionDisposition::RemainsCreated,
            "invalid provider setting; see advertised values; new Session remains created and idle",
        ),
    };
    let advertised_values = format_bounded_advertised_values(&advertised);
    let displayed_value = format_bounded_option_label(&value);
    let message = format!(
        "invalid provider setting {}={displayed_value}; advertised: {advertised_values}; {session_status}",
        setting.as_str()
    );
    let mut result = failure(
        ConversationOperationFailureKind::InvalidSetting,
        ConversationOperationFailureStage::Settlement,
        ProviderOperationEffect::Applied,
        fallback_message,
        operation_id,
        target,
    );
    if let Ok(message) = NonEmptyText::try_from(message) {
        result.message = message;
    }
    result.invalid_setting = Some(InvalidProviderSetting {
        setting: crate::provider_operation_settlement::provider_setting_name(setting),
        value,
        advertised,
        session_disposition: disposition,
    });
    result
}

fn format_bounded_option_label(label: &str) -> String {
    let mut label_characters = label.chars();
    let displayed_label = label_characters
        .by_ref()
        .take(MAX_SETTING_VALUE_DISPLAY_CHARS)
        .collect::<String>();
    let truncation_marker = if label_characters.next().is_some() {
        "…"
    } else {
        ""
    };
    format!("{displayed_label:?}{truncation_marker}")
}

fn format_bounded_advertised_values(advertised: &[String]) -> String {
    if advertised.is_empty() {
        return "none".to_owned();
    }

    let mut displayed_values = String::new();
    let mut displayed_count = 0;
    for option in advertised.iter().take(MAX_ADVERTISED_CHOICES) {
        let displayed_option = format_bounded_option_label(option);
        let separator = if displayed_count == 0 { "" } else { ", " };
        let omitted_after_option = advertised.len() - displayed_count - 1;
        let reserved_truncation_marker = if omitted_after_option == 0 {
            String::new()
        } else {
            format!(", … (+{omitted_after_option} more; see invalidSetting.advertised)")
        };
        let candidate_bytes = displayed_values.len()
            + separator.len()
            + displayed_option.len()
            + reserved_truncation_marker.len();
        if candidate_bytes > MAX_ADVERTISED_SUMMARY_BYTES {
            break;
        }

        displayed_values.push_str(separator);
        displayed_values.push_str(&displayed_option);
        displayed_count += 1;
    }

    let omitted_count = advertised.len() - displayed_count;
    if omitted_count > 0 {
        if displayed_count > 0 {
            displayed_values.push_str(", ");
        }
        displayed_values.push_str(&format!(
            "… (+{omitted_count} more; see invalidSetting.advertised)"
        ));
    }
    displayed_values
}

pub(super) fn prompt_runtime_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
    error: ExternalProviderRuntimeError,
) -> ConversationOperationFailure {
    if matches!(error, ExternalProviderRuntimeError::TransportFailure) {
        return failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider connection lost before the agent ended the turn (providerRetired)",
            operation_id,
            target,
        );
    }
    runtime_failure(operation_id, target, error)
}

pub(super) fn admission_failure(
    operation_id: OperationId,
    message: &'static str,
    target: Option<SessionRef>,
) -> ConversationOperationFailure {
    failure(
        ConversationOperationFailureKind::Unavailable,
        ConversationOperationFailureStage::Admission,
        ProviderOperationEffect::None,
        message,
        operation_id,
        target,
    )
}

pub(super) fn runtime_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
    error: ExternalProviderRuntimeError,
) -> ConversationOperationFailure {
    match error {
        ExternalProviderRuntimeError::LocalBusy => failure(
            ConversationOperationFailureKind::Busy,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider conversation already has active work",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::SettingsUnresolved => failure(
            ConversationOperationFailureKind::SettingsUnresolved,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider Session settings are unresolved; set a value or accept current settings before work",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::LocalNotFound
        | ExternalProviderRuntimeError::LocalCancelTargetMismatch => failure(
            ConversationOperationFailureKind::NotFound,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider conversation operation is not active",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::AuthenticationRequired {
            code,
            correlation_id,
        } => correlated_provider_failure(CorrelatedProviderFailureProps {
            kind: ConversationOperationFailureKind::AuthenticationRequired,
            stage: ConversationOperationFailureStage::Binding,
            effect: ProviderOperationEffect::None,
            explanation: "provider authentication is required",
            operation_id,
            target,
            provider_error: ProviderErrorReference {
                provider_code: code,
                correlation_id,
            },
        }),
        ExternalProviderRuntimeError::ProviderSessionNotFound {
            code,
            correlation_id,
        } => correlated_provider_failure(CorrelatedProviderFailureProps {
            kind: ConversationOperationFailureKind::ProviderSessionNotFound,
            stage: ConversationOperationFailureStage::Binding,
            effect: ProviderOperationEffect::None,
            explanation: "this session never started a turn and did not survive the provider restart; create a new conversation",
            operation_id,
            target,
            provider_error: ProviderErrorReference {
                provider_code: code,
                correlation_id,
            },
        }),
        ExternalProviderRuntimeError::ResourceNotFound {
            code,
            correlation_id,
        } => correlated_provider_failure(CorrelatedProviderFailureProps {
            kind: ConversationOperationFailureKind::NotFound,
            stage: ConversationOperationFailureStage::Binding,
            effect: ProviderOperationEffect::None,
            explanation: "provider resource was not found",
            operation_id,
            target,
            provider_error: ProviderErrorReference {
                provider_code: code,
                correlation_id,
            },
        }),
        ExternalProviderRuntimeError::UnsupportedMethod {
            code,
            correlation_id,
        } => correlated_provider_failure(CorrelatedProviderFailureProps {
            kind: ConversationOperationFailureKind::UnsupportedCapability,
            stage: ConversationOperationFailureStage::Validation,
            effect: ProviderOperationEffect::None,
            explanation: "provider ACP method is unsupported",
            operation_id,
            target,
            provider_error: ProviderErrorReference {
                provider_code: code,
                correlation_id,
            },
        }),
        ExternalProviderRuntimeError::InvalidParams {
            code,
            correlation_id,
        } => correlated_provider_failure(CorrelatedProviderFailureProps {
            kind: ConversationOperationFailureKind::InvalidRequest,
            stage: ConversationOperationFailureStage::Validation,
            effect: ProviderOperationEffect::None,
            explanation: "provider ACP parameters are invalid",
            operation_id,
            target,
            provider_error: ProviderErrorReference {
                provider_code: code,
                correlation_id,
            },
        }),
        ExternalProviderRuntimeError::RequestCancelled {
            code,
            correlation_id,
        } => correlated_provider_failure(CorrelatedProviderFailureProps {
            kind: ConversationOperationFailureKind::ProviderRejected,
            stage: ConversationOperationFailureStage::Settlement,
            effect: ProviderOperationEffect::Unknown,
            explanation: "provider ACP request was cancelled",
            operation_id,
            target,
            provider_error: ProviderErrorReference {
                provider_code: code,
                correlation_id,
            },
        }),
        ExternalProviderRuntimeError::ProviderRejected {
            code,
            correlation_id,
        } => correlated_provider_failure(CorrelatedProviderFailureProps {
            kind: ConversationOperationFailureKind::ProviderRejected,
            stage: ConversationOperationFailureStage::Settlement,
            effect: ProviderOperationEffect::Unknown,
            explanation: "provider rejected the operation after dispatch",
            operation_id,
            target,
            provider_error: ProviderErrorReference {
                provider_code: code,
                correlation_id,
            },
        }),
        ExternalProviderRuntimeError::PromptOutputLimitExceeded => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider prompt output exceeded the retained output limit; cancellation was requested and settled",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::FrameLimitExceeded => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider frame exceeded the configured transport limit",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::FrameDecodeFailure => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider frame could not be decoded or classified",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::UnsupportedContent { content_type } => failure(
            ConversationOperationFailureKind::UnsupportedCapability,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            match content_type {
                "image" => "unsupportedContent{image}",
                "audio" => "unsupportedContent{audio}",
                "embeddedResource" => "unsupportedContent{embeddedResource}",
                _ => "unsupportedContent{unknown}",
            },
            operation_id,
            target,
        ),
        _ => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider operation response was not available after dispatch",
            operation_id,
            target,
        ),
    }
}

fn correlated_provider_failure(
    props: CorrelatedProviderFailureProps,
) -> ConversationOperationFailure {
    let CorrelatedProviderFailureProps {
        kind,
        stage,
        effect,
        explanation,
        operation_id,
        target,
        provider_error,
    } = props;
    let correlation_id = match DeliveryCorrelationId::try_from(
        provider_error.correlation_id.to_string(),
    ) {
        Ok(correlation_id) => correlation_id,
        Err(_) => {
            return failure_with_provider_code(
                kind,
                stage,
                effect,
                "provider failure was classified, but its diagnostic reference could not be represented",
                operation_id,
                target,
                Some(provider_error.provider_code),
            );
        }
    };
    let mut failure = failure_with_provider_code(
        kind,
        stage,
        effect,
        explanation,
        operation_id,
        target,
        Some(provider_error.provider_code),
    );
    if let Ok(message) = NonEmptyText::try_from(format!(
        "{explanation} (provider code {}; reference {})",
        provider_error.provider_code,
        correlation_id.as_str()
    )) {
        failure.message = message;
    }
    failure
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client_runtime::ProviderSettingKind;
    use collaboration_protocol::ProviderSettingName;

    #[test]
    fn invalid_setting_failure_names_each_setting_and_advertised_values() {
        for (setting, name, expected_setting) in [
            (ProviderSettingKind::Mode, "mode", ProviderSettingName::Mode),
            (
                ProviderSettingKind::Model,
                "model",
                ProviderSettingName::Model,
            ),
            (
                ProviderSettingKind::Effort,
                "effort",
                ProviderSettingName::Effort,
            ),
        ] {
            let failure = invalid_setting_failure(
                OperationId::generate(),
                None,
                setting,
                "requested".to_owned(),
                vec!["first".to_owned(), "second".to_owned()],
                acp_client_runtime::InvalidSettingSessionDisposition::Closed,
            );

            assert_eq!(
                String::from(failure.message.clone()),
                format!(
                    "invalid provider setting {name}=\"requested\"; advertised: \"first\", \"second\"; new Session was closed"
                )
            );
            let invalid_setting = failure.invalid_setting.expect("invalid-setting detail");
            assert_eq!(invalid_setting.setting, expected_setting);
            assert_eq!(invalid_setting.value, "requested");
            assert_eq!(
                invalid_setting.advertised,
                vec!["first".to_owned(), "second".to_owned()]
            );
        }
    }

    #[test]
    fn invalid_setting_failure_bounds_large_advertised_values_and_echo() {
        let value = format!("requested\n{}", "v".repeat(500));
        let advertised = (0..500)
            .map(|choice_index| format!("choice-{choice_index}-{}", "🧪".repeat(120)))
            .collect::<Vec<_>>();
        let failure = invalid_setting_failure(
            OperationId::generate(),
            None,
            ProviderSettingKind::Model,
            value.clone(),
            advertised.clone(),
            acp_client_runtime::InvalidSettingSessionDisposition::Closed,
        );

        let message = String::from(failure.message.clone());
        let bounded_value = value
            .chars()
            .take(MAX_SETTING_VALUE_DISPLAY_CHARS)
            .collect::<String>();
        let expected_displayed_value = format!("{bounded_value:?}…");
        assert!(message.len() <= 4096);
        assert!(message.starts_with(&format!(
            "invalid provider setting model={expected_displayed_value}; advertised: "
        )));
        assert!(message.ends_with("; new Session was closed"));
        assert!(!message.contains(&"v".repeat(121)));

        let displayed_summary = message
            .split_once("; advertised: ")
            .and_then(|(_, after_advertised)| {
                after_advertised.split_once("; new Session was closed")
            })
            .map(|(summary, _)| summary)
            .expect("failure names the advertised choices and Session disposition");
        assert!(displayed_summary.len() <= MAX_ADVERTISED_SUMMARY_BYTES);
        assert!(displayed_summary.matches("\"choice-").count() <= MAX_ADVERTISED_CHOICES);
        let first_advertised_choice = advertised
            .first()
            .expect("the fixture advertises a first choice");
        let bounded_first_choice = first_advertised_choice
            .chars()
            .take(MAX_SETTING_VALUE_DISPLAY_CHARS)
            .collect::<String>();
        let expected_displayed_choice = format!("{bounded_first_choice:?}…");
        assert!(displayed_summary.starts_with(&expected_displayed_choice));

        let omitted_count = message
            .split("… (+")
            .nth(1)
            .and_then(|after_marker| {
                after_marker
                    .split_once(" more; see invalidSetting.advertised)")
                    .map(|(count, _)| count)
            })
            .and_then(|count| count.parse::<usize>().ok())
            .expect("truncation marker reports an omitted-choice count");
        assert!(omitted_count > 0);

        let invalid_setting = failure.invalid_setting.expect("invalid-setting detail");
        assert_eq!(invalid_setting.setting, ProviderSettingName::Model);
        assert_eq!(invalid_setting.value, value);
        assert_eq!(invalid_setting.advertised, advertised);
        assert_eq!(
            invalid_setting.session_disposition,
            InvalidSettingSessionDisposition::Closed
        );

        let short_advertised = (0..500)
            .map(|choice_index| format!("choice-{choice_index}"))
            .collect::<Vec<_>>();
        let count_limited_summary = format_bounded_advertised_values(&short_advertised);
        assert!(count_limited_summary.len() <= MAX_ADVERTISED_SUMMARY_BYTES);
        assert_eq!(
            count_limited_summary.matches("\"choice-").count(),
            MAX_ADVERTISED_CHOICES
        );
        assert!(count_limited_summary.contains("… (+468 more; see invalidSetting.advertised)"));
    }

    #[test]
    fn advertised_values_debug_escape_quotes_and_control_characters() {
        let advertised = vec!["line\n\"quoted\"".to_owned()];

        assert_eq!(
            format_bounded_advertised_values(&advertised),
            r#""line\n\"quoted\"""#
        );
    }
}
