//! Decode an ACP prompt result without the SDK's closed stop-reason enum.

use crate::external_provider_runtime::ExternalProviderRuntimeError;
use agent_client_protocol::schema::v1::StopReason;
use collaboration_protocol::ProviderPromptStopReason;

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
        unknown => Err(ExternalProviderRuntimeError::UnknownStopReason {
            suffix: safe_unknown_suffix(unknown),
        }),
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
        _ => Err(ExternalProviderRuntimeError::FrameDecodeFailure),
    }
}

fn safe_unknown_suffix(reason: &str) -> String {
    if !reason.is_empty()
        && reason.len() <= 32
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    {
        format!(" ({reason})")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_stop_reason_is_omitted_from_diagnostics() {
        let private_payload = "../token=synthetic-secret";
        let error = decode_prompt_result(&serde_json::json!({"stopReason": private_payload}))
            .expect_err("unknown stop reason");
        let diagnostic = error.to_string();
        assert_eq!(
            diagnostic,
            "agent ended the turn with an unrecognized stop reason"
        );
        assert!(!diagnostic.contains(private_payload));
    }
}
