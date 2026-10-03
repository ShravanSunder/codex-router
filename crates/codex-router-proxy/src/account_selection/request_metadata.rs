use super::*;

const ROUTING_METADATA_SCAN_LIMIT_BYTES: usize = 64 * 1024;

const ROUTING_METADATA_SCAN_MAX_TOP_LEVEL_KEYS: usize = 64;

pub(super) fn route_kind_for_request(
    request: &HttpProxyRequest,
) -> Result<crate::routes::RouteKind, HttpProxyError> {
    let classification_path = path_without_query(request.path());
    match classify_route(
        request.method(),
        classification_path,
        request.websocket_upgrade(),
    ) {
        RouteClass::Supported(route_kind) => Ok(route_kind),
        RouteClass::Rejected { reason } => Err(HttpProxyError::Rejected { reason }),
    }
}

pub(super) fn route_profile_for_kind(route_kind: RouteKind) -> RouteProfile {
    match route_kind {
        RouteKind::ResponsesWebSocket => RESPONSES_WEBSOCKET.clone(),
        RouteKind::ClaudeMessages => CLAUDE_MESSAGES.clone(),
        RouteKind::Responses
        | RouteKind::Models
        | RouteKind::MemoriesTraceSummarize
        | RouteKind::ResponsesCompact
        | RouteKind::ImageGenerations
        | RouteKind::ImageEdits => RESPONSES_HTTP.clone(),
    }
}

pub(super) const fn transport_label_for_request(request: &HttpProxyRequest) -> &'static str {
    if request.websocket_upgrade() {
        "websocket"
    } else {
        "http_sse"
    }
}

pub(super) fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map_or(path, |(path_without_query, _query)| path_without_query)
}

pub(super) fn previous_response_id(
    request: &HttpProxyRequest,
) -> Result<Option<PreviousResponseId>, HttpProxyError> {
    let Some(previous_response_id) =
        top_level_json_string_field(request.body(), b"previous_response_id")?
    else {
        return Ok(None);
    };
    if previous_response_id.is_empty() {
        return Ok(None);
    }

    Ok(PreviousResponseId::new(previous_response_id).ok())
}

pub(super) fn top_level_json_string_field(
    body: &[u8],
    field_name: &[u8],
) -> Result<Option<String>, HttpProxyError> {
    let mut cursor = skip_json_whitespace(body, 0);
    if body.get(cursor) != Some(&b'{') {
        return Ok(None);
    }
    cursor += 1;
    let mut depth = 1_u32;
    let scan_end = body.len().min(ROUTING_METADATA_SCAN_LIMIT_BYTES);
    let mut top_level_keys = 0_usize;

    while cursor < scan_end {
        cursor = skip_json_whitespace(body, cursor);
        let Some(byte) = body.get(cursor).copied() else {
            return Ok(None);
        };
        match byte {
            b'"' => {
                let Some((string_slice, after_string)) = json_string_slice(body, cursor) else {
                    return Ok(None);
                };
                if after_string > scan_end {
                    return Ok(None);
                }
                let after_key = skip_json_whitespace(body, after_string);
                if depth == 1 && body.get(after_key) == Some(&b':') {
                    top_level_keys += 1;
                    if top_level_keys > ROUTING_METADATA_SCAN_MAX_TOP_LEVEL_KEYS {
                        return Ok(None);
                    }
                    let Some(key) = serde_json::from_slice::<String>(string_slice).ok() else {
                        return Ok(None);
                    };
                    if key.as_bytes() == field_name {
                        let value_start = skip_json_whitespace(body, after_key + 1);
                        let Some((value_slice, _after_value)) =
                            json_string_slice(body, value_start)
                        else {
                            return Ok(None);
                        };
                        let Some(value) = serde_json::from_slice::<String>(value_slice).ok() else {
                            return Ok(None);
                        };
                        return Ok(Some(value));
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
                    return Ok(None);
                }
            }
            _ => {
                cursor += 1;
            }
        }
    }

    Ok(None)
}

fn json_string_slice(body: &[u8], start: usize) -> Option<(&[u8], usize)> {
    if body.get(start) != Some(&b'"') {
        return None;
    }
    let mut cursor = start + 1;
    while cursor < body.len() {
        match body.get(cursor).copied()? {
            b'\\' => {
                cursor = cursor.saturating_add(2);
            }
            b'"' => {
                let end = cursor + 1;
                return body.get(start..end).map(|slice| (slice, end));
            }
            _ => {
                cursor += 1;
            }
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
