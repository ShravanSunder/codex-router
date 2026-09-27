//! Decode an ACP prompt result without the SDK's closed stop-reason enum.

use crate::agent_session_client::ExternalProviderRuntimeError;
use agent_client_protocol::schema::v1::StopReason;
use session_event_model::StopReason as ProviderPromptStopReason;

pub(crate) fn decode_prompt_result(
    result: &serde_json::Value,
) -> Result<ProviderPromptStopReason, ExternalProviderRuntimeError> {
    let reason = result
        .get("stopReason")
        .and_then(serde_json::Value::as_str)
        .ok_or(ExternalProviderRuntimeError::FrameDecodeFailure)?;
    match reason {
        "end_turn" => Ok(ProviderPromptStopReason::EndTurn),
        "max_tokens" => Ok(ProviderPromptStopReason::MaxTokens),
        "max_turn_requests" => Ok(ProviderPromptStopReason::MaxTurnRequests),
        "refusal" => Ok(ProviderPromptStopReason::Refusal),
        "cancelled" => Ok(ProviderPromptStopReason::Cancelled),
        unknown => Ok(ProviderPromptStopReason::Unknown(
            safe_unknown_stop_reason(unknown).to_owned(),
        )),
    }
}

pub(crate) fn decode_typed_stop_reason(
    reason: StopReason,
) -> Result<ProviderPromptStopReason, ExternalProviderRuntimeError> {
    match reason {
        StopReason::EndTurn => Ok(ProviderPromptStopReason::EndTurn),
        StopReason::MaxTokens => Ok(ProviderPromptStopReason::MaxTokens),
        StopReason::MaxTurnRequests => Ok(ProviderPromptStopReason::MaxTurnRequests),
        StopReason::Refusal => Ok(ProviderPromptStopReason::Refusal),
        StopReason::Cancelled => Ok(ProviderPromptStopReason::Cancelled),
        _ => Ok(ProviderPromptStopReason::Unknown("unrecognized".to_owned())),
    }
}

fn safe_unknown_stop_reason(value: &str) -> &str {
    if !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    {
        value
    } else {
        "unrecognized"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_stop_reason_is_typed_without_exposing_untrusted_text() {
        let private_payload = "../token=synthetic-secret";
        let reason = decode_prompt_result(&serde_json::json!({"stopReason": private_payload}))
            .expect("unknown stop reason keeps the turn");
        assert_eq!(
            reason,
            ProviderPromptStopReason::Unknown("unrecognized".to_owned())
        );
        assert!(!format!("{reason:?}").contains(private_payload));
        let ordinary = decode_prompt_result(&serde_json::json!({"stopReason": "future_reason"}))
            .expect("safe unknown value is retained");
        assert_eq!(
            ordinary,
            ProviderPromptStopReason::Unknown("future_reason".to_owned())
        );
    }
}
