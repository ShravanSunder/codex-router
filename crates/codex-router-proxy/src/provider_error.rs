use serde_json::Value;

use codex_router_core::ids::AccountId;
use codex_router_core::routes::RouteBand;
use codex_router_state::sqlite::AsyncQuotaExhaustionRepository;
use codex_router_state::sqlite::StateStoreError;
use futures_util::future::BoxFuture;
use thiserror::Error;

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

#[cfg(test)]
mod tests {
    use super::ProviderErrorClassification;
    use super::classify_provider_error_envelope;

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
}
