use super::ClaudeUpstreamEndpoint;
#[cfg(debug_assertions)]
use super::ClaudeUpstreamEndpointError;
use crate::routes::RouteKind;
use crate::upstream::HyperHttpUpstreamTransport;
use crate::upstream::UpstreamEndpoint;

#[test]
fn claude_fixed_endpoint_preserves_v1_messages_path_and_query() {
    let endpoint = ClaudeUpstreamEndpoint::production();
    assert_eq!(
        endpoint.url_for_path("/v1/messages?beta=true"),
        "https://api.anthropic.com/v1/messages?beta=true"
    );
    assert_eq!(ClaudeUpstreamEndpoint::default(), endpoint);
}

#[test]
fn claude_transport_uses_fixed_destination_despite_configurable_codex_endpoint() {
    let codex = UpstreamEndpoint::new("http://127.0.0.1:18889/backend-api")
        .unwrap_or_else(|error| panic!("Codex endpoint: {error}"));
    let transport = HyperHttpUpstreamTransport::new(codex);
    assert_eq!(
        transport.upstream_url(RouteKind::ClaudeMessages, "/v1/messages"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        transport.upstream_url(RouteKind::Responses, "/v1/responses"),
        "http://127.0.0.1:18889/backend-api/codex/responses"
    );
}

#[test]
fn claude_destination_selection_preserves_all_codex_url_rules() {
    let codex = UpstreamEndpoint::new("https://chatgpt.com/backend-api")
        .unwrap_or_else(|error| panic!("Codex endpoint: {error}"));
    let transport = HyperHttpUpstreamTransport::new(codex.clone());
    for (route, path) in [
        (RouteKind::Responses, "/v1/responses?beta=true"),
        (RouteKind::Models, "/v1/models?client_version=1"),
        (RouteKind::ResponsesCompact, "/v1/responses/compact"),
        (
            RouteKind::MemoriesTraceSummarize,
            "/v1/memories/trace_summarize",
        ),
        (RouteKind::ImageGenerations, "/v1/images/generations"),
        (RouteKind::ImageEdits, "/v1/images/edits"),
    ] {
        assert_eq!(
            transport.upstream_url(route, path),
            codex.url_for_path(path)
        );
    }
}

#[cfg(debug_assertions)]
#[test]
fn claude_debug_override_rejects_missing_debug_isolation() {
    assert_eq!(
        ClaudeUpstreamEndpoint::isolated_debug_override("http://127.0.0.1:18888", false),
        Err(ClaudeUpstreamEndpointError::DebugIsolationRequired)
    );
}

#[cfg(debug_assertions)]
#[test]
fn claude_debug_override_is_provider_scoped_and_preserves_v1_path() {
    let endpoint = ClaudeUpstreamEndpoint::isolated_debug_override("http://127.0.0.1:18888/", true)
        .unwrap_or_else(|error| panic!("isolated endpoint: {error}"));
    let codex = UpstreamEndpoint::new("https://chatgpt.com/backend-api")
        .unwrap_or_else(|error| panic!("Codex endpoint: {error}"));
    let transport =
        HyperHttpUpstreamTransport::new(codex).with_debug_claude_upstream_endpoint(endpoint);
    assert_eq!(
        transport.upstream_url(RouteKind::ClaudeMessages, "/v1/messages"),
        "http://127.0.0.1:18888/v1/messages"
    );
    assert_eq!(
        transport.upstream_url(RouteKind::Responses, "/v1/responses"),
        "https://chatgpt.com/backend-api/codex/responses"
    );
}

#[cfg(debug_assertions)]
#[test]
fn claude_debug_override_rejects_non_http_or_relative_urls() {
    for invalid in [
        "",
        "127.0.0.1:18888",
        "/relative",
        "file:///tmp/socket",
        "https://api.anthropic.com?redirect=1",
    ] {
        assert_eq!(
            ClaudeUpstreamEndpoint::isolated_debug_override(invalid, true),
            Err(ClaudeUpstreamEndpointError::InvalidBaseUrl)
        );
    }
}
