use super::*;

const WEBSOCKET_METADATA_SCAN_LIMIT_BYTES: usize = 64 * 1024;
const WEBSOCKET_METADATA_SCAN_MAX_TOP_LEVEL_KEYS: usize = 64;
pub(super) async fn record_forwarded_websocket_metadata(
    metadata_text: tungstenite::Utf8Bytes,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
    async_affinity_owner_recorder: Option<Arc<dyn AsyncHttpAffinityOwnerRecorder>>,
    affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
) {
    let affinity_owner =
        websocket_affinity_owner_record_from_text(&metadata_text, affinity_owner_context);
    if let Some(owner) = affinity_owner {
        if let Some(recorder) = async_affinity_owner_recorder {
            let _result = recorder.record_affinity_owner(owner).await;
        } else if let Some(recorder) = affinity_owner_recorder {
            let _join_result = tokio::task::spawn_blocking(move || {
                let _result = recorder.record_affinity_owner(&owner);
            })
            .await;
        }
    }
}
#[cfg(test)]
pub(super) fn record_websocket_affinity_owner(
    upstream_message: &Message,
    affinity_owner_recorder: Option<&dyn HttpAffinityOwnerRecorder>,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) {
    let Some(recorder) = affinity_owner_recorder else {
        return;
    };
    let Some(owner) = websocket_affinity_owner_record(upstream_message, affinity_owner_context)
    else {
        return;
    };
    let _result = recorder.record_affinity_owner(&owner);
}

#[cfg(test)]
pub(super) fn websocket_affinity_owner_record(
    upstream_message: &Message,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) -> Option<PreviousResponseAffinityOwnerRecord> {
    let text = websocket_metadata_text_handle(upstream_message)?;
    websocket_affinity_owner_record_from_text(&text, affinity_owner_context)
}

fn websocket_affinity_owner_record_from_text(
    text: &str,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) -> Option<PreviousResponseAffinityOwnerRecord> {
    let context = affinity_owner_context?;
    let previous_response_id = extract_websocket_response_id_from_text(text)?;
    let Ok(affinity_key_hash) =
        hash_previous_response_id(&context.affinity_secret, &previous_response_id)
    else {
        return None;
    };
    Some(PreviousResponseAffinityOwnerRecord::new(
        affinity_key_hash,
        context.account_id.clone(),
        context.credential_generation,
        RouteBand::Responses,
        AffinitySourceTransport::WebSocket,
        current_unix_seconds(),
    ))
}

fn extract_websocket_response_id_from_text(text: &str) -> Option<PreviousResponseId> {
    if text.len() > WEBSOCKET_METADATA_SCAN_LIMIT_BYTES {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let response_id = value
        .get("response")
        .and_then(serde_json::Value::as_object)
        .and_then(|response| response.get("id"))
        .and_then(serde_json::Value::as_str)?;
    if response_id.is_empty() {
        return None;
    }
    PreviousResponseId::new(response_id.to_owned()).ok()
}

#[cfg(test)]
pub(super) fn is_response_completed(message: &Message) -> bool {
    let Some(text) = websocket_metadata_text_handle(message) else {
        return false;
    };
    is_response_completed_text(&text)
}

pub(super) fn is_response_create(message: &Message) -> bool {
    let Some(text) = websocket_metadata_text_handle(message) else {
        return false;
    };
    bounded_top_level_json_string_field_equals(
        text.as_str().as_bytes(),
        b"type",
        b"response.create",
    )
}

pub(super) fn is_response_completed_text(text: &str) -> bool {
    bounded_top_level_json_string_field_equals(text.as_bytes(), b"type", b"response.completed")
}

pub(super) fn is_response_failed_text(text: &str) -> bool {
    bounded_top_level_json_string_field_equals(text.as_bytes(), b"type", b"response.failed")
}

pub(super) fn is_response_incomplete_text(text: &str) -> bool {
    bounded_top_level_json_string_field_equals(text.as_bytes(), b"type", b"response.incomplete")
}

pub(super) fn is_response_terminal_error_text(text: &str) -> bool {
    if text.len() > WEBSOCKET_METADATA_SCAN_LIMIT_BYTES {
        return false;
    }
    let Ok(error_envelope) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    if error_envelope
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("error")
    {
        return false;
    }

    let has_non_success_status = error_envelope
        .get("status")
        .or_else(|| error_envelope.get("status_code"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| http::StatusCode::from_u16(status).ok())
        .is_some_and(|status| !status.is_success());
    let terminal_error_code = error_envelope
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|code| {
            matches!(
                code,
                "websocket_connection_limit_reached" | "previous_response_not_found"
            )
        });

    has_non_success_status || terminal_error_code
}

pub(super) fn has_forbidden_top_level_websocket_auth_carrier(body: &[u8]) -> bool {
    bounded_top_level_json_key_matches(body, |key| {
        let canonical = canonical_websocket_auth_field_name(key);
        matches!(
            canonical.as_deref(),
            Some("authorization" | "api-key" | "openai-api-key" | "x-codex-router-token")
        )
    })
}

fn bounded_top_level_json_string_field_equals(
    body: &[u8],
    field_name: &[u8],
    expected: &[u8],
) -> bool {
    let Some(value) = bounded_top_level_json_string_field(body, field_name) else {
        return false;
    };
    value.as_bytes() == expected
}

fn bounded_top_level_json_string_field(body: &[u8], field_name: &[u8]) -> Option<String> {
    let mut cursor = skip_json_whitespace(body, 0);
    if body.get(cursor) != Some(&b'{') {
        return None;
    }
    cursor += 1;
    let mut depth = 1_u32;
    let scan_end = body.len().min(WEBSOCKET_METADATA_SCAN_LIMIT_BYTES);
    let mut top_level_keys = 0_usize;
    while cursor < scan_end {
        cursor = skip_json_whitespace(body, cursor);
        let byte = body.get(cursor).copied()?;
        match byte {
            b'"' => {
                let (string_slice, after_string) = json_string_slice(body, cursor)?;
                if after_string > scan_end {
                    return None;
                }
                let after_key = skip_json_whitespace(body, after_string);
                if depth == 1 && body.get(after_key) == Some(&b':') {
                    top_level_keys += 1;
                    if top_level_keys > WEBSOCKET_METADATA_SCAN_MAX_TOP_LEVEL_KEYS {
                        return None;
                    }
                    let key = serde_json::from_slice::<String>(string_slice).ok()?;
                    if key.as_bytes() == field_name {
                        let value_start = skip_json_whitespace(body, after_key + 1);
                        let (value_slice, _after_value) = json_string_slice(body, value_start)?;
                        return serde_json::from_slice::<String>(value_slice).ok();
                    }
                    cursor = after_key + 1;
                } else {
                    cursor = after_string;
                }
            }
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                cursor += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                if depth == 0 {
                    return None;
                }
            }
            _ => {
                cursor += 1;
            }
        }
    }
    None
}

fn bounded_top_level_json_key_matches(
    body: &[u8],
    mut matches_key: impl FnMut(&str) -> bool,
) -> bool {
    let mut cursor = skip_json_whitespace(body, 0);
    if body.get(cursor) != Some(&b'{') {
        return false;
    }
    cursor += 1;
    let scan_end = body.len().min(WEBSOCKET_METADATA_SCAN_LIMIT_BYTES);
    let mut depth = 1_u32;
    let mut top_level_keys = 0_usize;
    while cursor < scan_end {
        cursor = skip_json_whitespace(body, cursor);
        let Some(byte) = body.get(cursor).copied() else {
            return false;
        };
        match byte {
            b'"' => {
                let Some((string_slice, after_string)) = json_string_slice(body, cursor) else {
                    return false;
                };
                if after_string > scan_end {
                    return false;
                }
                let after_key = skip_json_whitespace(body, after_string);
                if depth == 1 && body.get(after_key) == Some(&b':') {
                    top_level_keys += 1;
                    if top_level_keys > WEBSOCKET_METADATA_SCAN_MAX_TOP_LEVEL_KEYS {
                        return false;
                    }
                    if let Ok(key) = serde_json::from_slice::<String>(string_slice)
                        && matches_key(&key)
                    {
                        return true;
                    }
                    cursor = after_key + 1;
                } else {
                    cursor = after_string;
                }
            }
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                cursor += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                if depth == 0 {
                    return false;
                }
            }
            _ => cursor += 1,
        }
    }
    false
}

fn canonical_websocket_auth_field_name(value: &str) -> Option<String> {
    let decoded = percent_decode_ascii(value);
    let canonical: String = decoded
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    match canonical.as_str() {
        "authorization" => Some("authorization".to_owned()),
        "apikey" => Some("api-key".to_owned()),
        "openaiapikey" => Some("openai-api-key".to_owned()),
        "xcodexroutertoken" => Some("x-codex-router-token".to_owned()),
        _ => None,
    }
}

fn percent_decode_ascii(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    while index < bytes.len() {
        let Some(byte) = bytes.get(index).copied() else {
            break;
        };
        if byte == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(index + 1).copied().and_then(hex_value),
                bytes.get(index + 2).copied().and_then(hex_value),
            )
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn json_string_slice(body: &[u8], start: usize) -> Option<(&[u8], usize)> {
    if body.get(start) != Some(&b'"') {
        return None;
    }
    let mut cursor = start + 1;
    while cursor < body.len() {
        match body.get(cursor).copied()? {
            b'\\' => cursor = cursor.saturating_add(2),
            b'"' => {
                let end = cursor + 1;
                return body.get(start..end).map(|slice| (slice, end));
            }
            _ => cursor += 1,
        }
    }
    None
}

fn skip_json_whitespace(body: &[u8], mut cursor: usize) -> usize {
    while body
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        cursor += 1;
    }
    cursor
}
pub(super) fn websocket_metadata_text_handle(message: &Message) -> Option<tungstenite::Utf8Bytes> {
    let Message::Text(text) = message else {
        return None;
    };
    Some(text.clone())
}

pub(super) fn provider_error_body_from_message(message: &Message) -> Option<Vec<u8>> {
    let Message::Text(text) = message else {
        return None;
    };
    let body = text.as_str().as_bytes();
    if provider_error_classification_from_message(message) == ProviderErrorClassification::Unknown {
        return None;
    }

    Some(body.to_vec())
}

pub(super) fn provider_error_classification_from_message(
    message: &Message,
) -> ProviderErrorClassification {
    let Message::Text(text) = message else {
        return ProviderErrorClassification::Unknown;
    };
    classify_responses_websocket_error_envelope(text.as_str().as_bytes())
}
