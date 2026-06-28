use std::borrow::Cow;

use serde::Deserialize;
use serde_json::Value;

use codex_router_core::ids::AccountId;
use codex_router_core::routes::RouteBand;
use codex_router_state::sqlite::AsyncQuotaExhaustionRepository;
use codex_router_state::sqlite::StateStoreError;
use futures_util::future::BoxFuture;
use thiserror::Error;

const PROVIDER_ERROR_ENVELOPE_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderErrorClassification {
    Unknown,
    AccountQuotaExhausted,
    WebSocketConnectionLimit,
}

#[derive(Debug, Error)]
pub enum ProviderErrorObservationError {
    #[error("state store unavailable while recording provider error")]
    State(#[from] StateStoreError),
}

pub trait AsyncProviderErrorObserver: Send + Sync {
    fn observe_provider_error<'a>(
        &'a self,
        account_id: AccountId,
        route_band: RouteBand,
        body: Vec<u8>,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>>;

    fn route_band_has_selectable_alternative_after_exhaustion<'a>(
        &'a self,
        _exhausted_account_id: AccountId,
        _route_band: RouteBand,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<bool, ProviderErrorObservationError>> {
        Box::pin(async { Ok(true) })
    }
}

pub fn classify_provider_error_envelope(body: &[u8]) -> ProviderErrorClassification {
    if body.len() > PROVIDER_ERROR_ENVELOPE_MAX_BYTES {
        return classify_provider_error_envelope_prefix(body);
    }

    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return ProviderErrorClassification::Unknown;
    };
    if !is_provider_error_envelope(&value) {
        return ProviderErrorClassification::Unknown;
    }

    let explicit_error_tokens = explicit_error_tokens(&value);
    if explicit_error_tokens.contains(&"websocket_connection_limit_reached") {
        return ProviderErrorClassification::WebSocketConnectionLimit;
    }
    if explicit_error_tokens
        .iter()
        .any(|token| is_quota_exhaustion_token(token))
    {
        return ProviderErrorClassification::AccountQuotaExhausted;
    }

    ProviderErrorClassification::Unknown
}

pub fn classify_responses_websocket_error_envelope(body: &[u8]) -> ProviderErrorClassification {
    if body.len() > PROVIDER_ERROR_ENVELOPE_MAX_BYTES {
        return classify_responses_websocket_error_envelope_prefix(body);
    }

    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return ProviderErrorClassification::Unknown;
    };
    if value.get("type").and_then(Value::as_str) != Some("error") {
        return ProviderErrorClassification::Unknown;
    }

    let Some(error) = value.get("error").and_then(Value::as_object) else {
        return ProviderErrorClassification::Unknown;
    };
    let mut tokens = Vec::new();
    if let Some(code) = error.get("code").and_then(Value::as_str) {
        tokens.push(code);
    }
    if let Some(error_type) = error.get("type").and_then(Value::as_str) {
        tokens.push(error_type);
    }

    if tokens.contains(&"websocket_connection_limit_reached") {
        return ProviderErrorClassification::WebSocketConnectionLimit;
    }
    if tokens.iter().any(|token| is_quota_exhaustion_token(token)) {
        return ProviderErrorClassification::AccountQuotaExhausted;
    }

    ProviderErrorClassification::Unknown
}

pub async fn record_provider_error_observation<R>(
    repository: &R,
    account_id: &AccountId,
    route_band: &str,
    body: &[u8],
    observed_unix_seconds: u64,
) -> Result<ProviderErrorClassification, StateStoreError>
where
    R: AsyncQuotaExhaustionRepository + Sync,
{
    let classification = classify_provider_error_envelope(body);
    if classification == ProviderErrorClassification::AccountQuotaExhausted {
        repository
            .mark_route_band_quota_exhausted(account_id, route_band, observed_unix_seconds)
            .await?;
    }

    Ok(classification)
}

fn is_provider_error_envelope(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("error")
        || value.get("error").is_some_and(Value::is_object)
}

fn explicit_error_tokens(value: &Value) -> Vec<&str> {
    let mut tokens = Vec::new();
    push_string_field(value, "code", &mut tokens);
    push_string_field(value, "type", &mut tokens);
    if let Some(error) = value.get("error").and_then(Value::as_object) {
        if let Some(code) = error.get("code").and_then(Value::as_str) {
            tokens.push(code);
        }
        if let Some(error_type) = error.get("type").and_then(Value::as_str) {
            tokens.push(error_type);
        }
    }

    tokens
}

fn push_string_field<'a>(value: &'a Value, field_name: &str, tokens: &mut Vec<&'a str>) {
    if let Some(token) = value.get(field_name).and_then(Value::as_str) {
        tokens.push(token);
    }
}

fn is_quota_exhaustion_token(token: &str) -> bool {
    matches!(
        token,
        "usage_limit_reached" | "quota_exceeded" | "insufficient_quota"
    )
}

fn classify_provider_error_envelope_prefix(body: &[u8]) -> ProviderErrorClassification {
    let Ok(body_text) = std::str::from_utf8(body) else {
        return ProviderErrorClassification::Unknown;
    };
    if !json_object_braces_are_balanced(body_text) {
        return ProviderErrorClassification::Unknown;
    }
    let Some(prefix) = body.get(..PROVIDER_ERROR_ENVELOPE_MAX_BYTES) else {
        return ProviderErrorClassification::Unknown;
    };
    let Ok(prefix) = std::str::from_utf8(prefix) else {
        return ProviderErrorClassification::Unknown;
    };
    let trimmed = prefix.trim_start();
    let Some(after_open_brace) = trimmed.strip_prefix('{') else {
        return ProviderErrorClassification::Unknown;
    };
    if !after_open_brace.trim_start().starts_with('"') {
        return ProviderErrorClassification::Unknown;
    }
    if !(prefix_top_level_string_field_equals(trimmed, "type", "error")
        || prefix_top_level_object_field_present(trimmed, "error"))
    {
        return ProviderErrorClassification::Unknown;
    }

    let mut tokens = Vec::new();
    push_prefix_json_string_field_values(trimmed, "code", &mut tokens);
    push_prefix_json_string_field_values(trimmed, "type", &mut tokens);

    if tokens
        .iter()
        .any(|token| token == "websocket_connection_limit_reached")
    {
        return ProviderErrorClassification::WebSocketConnectionLimit;
    }
    if tokens
        .iter()
        .any(|token| is_quota_exhaustion_token(token.as_str()))
    {
        return ProviderErrorClassification::AccountQuotaExhausted;
    }

    ProviderErrorClassification::Unknown
}

fn classify_responses_websocket_error_envelope_prefix(body: &[u8]) -> ProviderErrorClassification {
    let Ok(envelope) = serde_json::from_slice::<ResponsesWebSocketErrorEnvelopeProbe<'_>>(body)
    else {
        return ProviderErrorClassification::Unknown;
    };
    if envelope.message_type.as_deref() != Some("error") {
        return ProviderErrorClassification::Unknown;
    };
    let Some(error) = envelope.error else {
        return ProviderErrorClassification::Unknown;
    };

    if error.code.as_deref() == Some("websocket_connection_limit_reached")
        || error.error_type.as_deref() == Some("websocket_connection_limit_reached")
    {
        return ProviderErrorClassification::WebSocketConnectionLimit;
    }
    if error.code.as_deref().is_some_and(is_quota_exhaustion_token)
        || error
            .error_type
            .as_deref()
            .is_some_and(is_quota_exhaustion_token)
    {
        return ProviderErrorClassification::AccountQuotaExhausted;
    }

    ProviderErrorClassification::Unknown
}

#[derive(Debug, Deserialize)]
struct ResponsesWebSocketErrorEnvelopeProbe<'a> {
    #[serde(rename = "type", borrow)]
    message_type: Option<Cow<'a, str>>,
    #[serde(borrow)]
    error: Option<ResponsesWebSocketErrorObjectProbe<'a>>,
}

#[derive(Debug, Deserialize)]
struct ResponsesWebSocketErrorObjectProbe<'a> {
    #[serde(borrow)]
    code: Option<Cow<'a, str>>,
    #[serde(rename = "type", borrow)]
    error_type: Option<Cow<'a, str>>,
}

fn prefix_top_level_string_field_equals(
    prefix: &str,
    field_name: &str,
    expected_value: &str,
) -> bool {
    prefix_top_level_field_value(prefix, field_name)
        .is_some_and(|value| value == PrefixJsonFieldValue::String(expected_value))
}

fn prefix_top_level_object_field_present(prefix: &str, field_name: &str) -> bool {
    prefix_top_level_field_value(prefix, field_name)
        .is_some_and(|value| value == PrefixJsonFieldValue::Object)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrefixJsonFieldValue<'a> {
    String(&'a str),
    Object,
}

fn prefix_top_level_field_value<'a>(
    prefix: &'a str,
    field_name: &str,
) -> Option<PrefixJsonFieldValue<'a>> {
    let bytes = prefix.as_bytes();
    let mut index = 0;
    let mut depth = 0_u32;
    let mut expect_field = false;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                depth = depth.saturating_add(1);
                expect_field = depth == 1;
                index += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                expect_field = false;
                index += 1;
            }
            b',' if depth == 1 => {
                expect_field = true;
                index += 1;
            }
            b'"' if depth == 1 && expect_field => {
                let (field, after_field) = parse_json_string_token(prefix, index)?;
                let value_index = prefix_json_value_start(bytes, after_field)?;
                if field == field_name {
                    return match bytes.get(value_index).copied() {
                        Some(b'"') => parse_json_string_token(prefix, value_index)
                            .map(|(value, _)| PrefixJsonFieldValue::String(value)),
                        Some(b'{') => Some(PrefixJsonFieldValue::Object),
                        _ => None,
                    };
                }
                expect_field = false;
                index = after_field;
            }
            b'"' => {
                let (_, after_string) = parse_json_string_token(prefix, index)?;
                index = after_string;
            }
            b if b.is_ascii_whitespace() => {
                index += 1;
            }
            _ => {
                expect_field = false;
                index += 1;
            }
        }
    }

    None
}

fn push_prefix_json_string_field_values(prefix: &str, field_name: &str, tokens: &mut Vec<String>) {
    let bytes = prefix.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let Some((field, after_field)) = parse_json_string_token(prefix, index) else {
            return;
        };
        let Some(value_index) = prefix_json_value_start(bytes, after_field) else {
            index = after_field;
            continue;
        };
        if field == field_name
            && bytes.get(value_index).copied() == Some(b'"')
            && let Some((value, after_value)) = parse_json_string_token(prefix, value_index)
        {
            tokens.push(value.to_owned());
            index = after_value;
            continue;
        }
        index = after_field;
    }
}

fn json_object_braces_are_balanced(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut index = 0;
    while matches!(bytes.get(index), Some(byte) if byte.is_ascii_whitespace()) {
        index += 1;
    }
    if bytes.get(index).copied() != Some(b'{') {
        return false;
    }

    let mut depth = 0_u32;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                depth = depth.saturating_add(1);
                index += 1;
            }
            b'}' => {
                let Some(next_depth) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next_depth;
                index += 1;
                if depth == 0 {
                    return bytes[index..].iter().all(|byte| byte.is_ascii_whitespace());
                }
            }
            b'"' => {
                let Some((_, after_string)) = parse_json_string_token(input, index) else {
                    return false;
                };
                index = after_string;
            }
            _ => {
                index += 1;
            }
        }
    }

    false
}

fn prefix_json_value_start(bytes: &[u8], after_field: usize) -> Option<usize> {
    let mut index = after_field;
    while matches!(bytes.get(index), Some(byte) if byte.is_ascii_whitespace()) {
        index += 1;
    }
    if bytes.get(index).copied() != Some(b':') {
        return None;
    }
    index += 1;
    while matches!(bytes.get(index), Some(byte) if byte.is_ascii_whitespace()) {
        index += 1;
    }

    Some(index)
}

fn parse_json_string_token(input: &str, quote_index: usize) -> Option<(&str, usize)> {
    let bytes = input.as_bytes();
    if bytes.get(quote_index).copied() != Some(b'"') {
        return None;
    }
    let mut index = quote_index + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index = index.saturating_add(2);
            }
            b'"' => {
                let value = input.get(quote_index + 1..index)?;
                return Some((value, index + 1));
            }
            _ => {
                index += 1;
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::ProviderErrorClassification;
    use super::classify_provider_error_envelope;
    use super::classify_responses_websocket_error_envelope;

    #[test]
    fn websocket_connection_limit_is_transport_reconnect_not_quota() {
        let envelope = br#"{
            "type": "error",
            "status": 400,
            "error": {
                "type": "invalid_request_error",
                "code": "websocket_connection_limit_reached",
                "message": "Responses websocket connection limit reached"
            }
        }"#;

        let classification = classify_provider_error_envelope(envelope);

        assert_eq!(
            classification,
            ProviderErrorClassification::WebSocketConnectionLimit
        );
    }

    #[test]
    fn usage_limit_error_is_account_quota_exhaustion() {
        let envelope = br#"{
            "type": "error",
            "status": 429,
            "error": {
                "type": "usage_limit_reached",
                "code": "usage_limit_reached",
                "message": "You have reached your usage limit"
            }
        }"#;

        let classification = classify_provider_error_envelope(envelope);

        assert_eq!(
            classification,
            ProviderErrorClassification::AccountQuotaExhausted
        );
    }

    #[test]
    fn ambiguous_model_text_with_quota_words_is_not_classified() {
        let model_message = br#"{
            "type": "response.output_text.delta",
            "delta": "The phrase usage_limit_reached appears in this explanation."
        }"#;

        let classification = classify_provider_error_envelope(model_message);

        assert_eq!(classification, ProviderErrorClassification::Unknown);
    }

    #[test]
    fn oversized_provider_error_like_payload_is_classified_from_explicit_fields() {
        let padding = "x".repeat(128 * 1024);
        let envelope = format!(
            r#"{{
                "type":"error",
                "error":{{
                    "type":"usage_limit_reached",
                    "code":"usage_limit_reached",
                    "message":"{padding}"
                }}
            }}"#
        );

        let classification = classify_provider_error_envelope(envelope.as_bytes());

        assert_eq!(
            classification,
            ProviderErrorClassification::AccountQuotaExhausted
        );
    }

    #[test]
    fn oversized_websocket_error_with_json_whitespace_is_classified() {
        let padding = "x".repeat(128 * 1024);
        let envelope = format!(
            r#"{{
                "type" : "error",
                "error" :
                {{
                    "type" : "usage_limit_reached",
                    "code" : "usage_limit_reached",
                    "message" : "{padding}"
                }}
            }}"#
        );

        let classification = classify_responses_websocket_error_envelope(envelope.as_bytes());

        assert_eq!(
            classification,
            ProviderErrorClassification::AccountQuotaExhausted
        );
    }

    #[test]
    fn oversized_websocket_malformed_error_like_payload_is_not_classified() {
        let padding = "x".repeat(128 * 1024);
        let envelope = format!(
            r#"{{"type":"error","error":{{"code":"usage_limit_reached"}} bogus, "padding":"{padding}"}}"#
        );

        let classification = classify_responses_websocket_error_envelope(envelope.as_bytes());

        assert_eq!(classification, ProviderErrorClassification::Unknown);
    }

    #[test]
    fn oversized_websocket_quota_token_outside_error_object_is_not_classified() {
        let padding = "x".repeat(128 * 1024);
        let envelope = format!(
            r#"{{
                "type":"error",
                "debug":{{"code":"usage_limit_reached"}},
                "error":{{"type":"invalid_request_error","code":"bad_request","message":"{padding}"}}
            }}"#
        );

        let classification = classify_responses_websocket_error_envelope(envelope.as_bytes());

        assert_eq!(classification, ProviderErrorClassification::Unknown);
    }

    #[test]
    fn oversized_model_text_with_quota_words_is_not_classified() {
        let padding = "x".repeat(128 * 1024);
        let model_message = format!(
            r#"{{
                "type":"response.output_text.delta",
                "delta":"usage_limit_reached {padding}"
            }}"#
        );

        let classification = classify_provider_error_envelope(model_message.as_bytes());

        assert_eq!(classification, ProviderErrorClassification::Unknown);
    }
}
