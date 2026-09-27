//! Test-only observations retained across ACP client extraction.
use super::*;
pub(super) use agent_client_protocol::schema::v1::ToolKind;

pub(super) use acp_client_runtime::MAX_ACP_FRAME_BYTES;

pub(crate) use acp_client_runtime::sanitized_initialization_error_for_test as sanitized_initialization_error;

pub(super) fn has_completed_router_endpoints_call(tool_calls: &[ExternalProviderToolCall]) -> bool {
    tool_calls.iter().any(|tool_call| {
        (tool_call.name.as_deref() == Some("router-collaboration-endpoints_list")
            || tool_call.title == "router-collaboration: endpoints_list")
            && tool_call.kind == ToolKind::Other
            && tool_call.status == agent_client_protocol::schema::v1::ToolCallStatus::Completed
            && tool_call.outcome == ExternalProviderToolOutcome::Success
    })
}

pub(super) fn accepts_native_router_result(
    tool_calls: &[ExternalProviderToolCall],
    response: &str,
    expected_service_id: &collaboration_protocol::UuidIdentity,
) -> bool {
    let output = classify_provider_output(response, expected_service_id);
    has_completed_router_endpoints_call(tool_calls)
        && output.returned_uuid_count == 1
        && output.sole_uuid_matches_expected
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ProviderOutputClassification {
    pub(super) output_empty: bool,
    pub(super) exact_identity_equal: bool,
    pub(super) expected_identity_token_present: bool,
    pub(super) returned_uuid_count: usize,
    pub(super) sole_uuid_matches_expected: bool,
    pub(super) output_byte_count: usize,
}

pub(super) fn classify_provider_output(
    output: &str,
    expected_service_id: &collaboration_protocol::UuidIdentity,
) -> ProviderOutputClassification {
    fn is_uuid_shape(candidate: &[u8]) -> bool {
        candidate.len() == 36
            && candidate.iter().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
    }

    fn continues_token(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
    }

    let expected = String::from(expected_service_id.clone());
    let bytes = output.as_bytes();
    let mut returned_uuids = Vec::new();
    let mut start = 0_usize;
    while start.saturating_add(36) <= bytes.len() {
        let end = start + 36;
        let bounded_before = start == 0 || !continues_token(bytes[start - 1]);
        let bounded_after = end == bytes.len() || !continues_token(bytes[end]);
        if bounded_before
            && bounded_after
            && is_uuid_shape(&bytes[start..end])
            && let Ok(candidate) = std::str::from_utf8(&bytes[start..end])
            && let Ok(identity) =
                collaboration_protocol::UuidIdentity::try_from(candidate.to_owned())
        {
            returned_uuids.push(identity);
            start = end;
            continue;
        }
        start += 1;
    }
    let expected_identity_token_present = returned_uuids
        .iter()
        .any(|identity| identity == expected_service_id);
    ProviderOutputClassification {
        output_empty: output.is_empty(),
        exact_identity_equal: output.trim() == expected,
        expected_identity_token_present,
        returned_uuid_count: returned_uuids.len(),
        sole_uuid_matches_expected: returned_uuids.len() == 1 && expected_identity_token_present,
        output_byte_count: output.len(),
    }
}
