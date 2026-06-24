//! Route-native black-box proof harness.

mod client;
mod fixture;
mod upstream;

use client::assert_rejected_locally;
use client::assert_success_response;
use client::is_local_websocket_rejection;
use client::require_websocket_rejection;
use client::send_http_request;
use client::send_websocket_request;
use fixture::LOCAL_TOKEN;
use fixture::RouteNativeTempRoot;
use fixture::seed_route_native_state;
use fixture::start_route_native_router;
pub use upstream::RouteNativeRecordedRequest;
pub use upstream::RouteNativeTranscript;
use upstream::RouteNativeUpstream;
use upstream::assert_route_native_transcript;

/// Runs route-native black-box proof against a served local router.
pub fn run_route_native_black_box() -> Result<RouteNativeReport, String> {
    let test_root = RouteNativeTempRoot::new("route-native")?;
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    seed_route_native_state(&state_path, &secret_root)?;

    let upstream = RouteNativeUpstream::start(5)?;
    let router = start_route_native_router(
        &state_path,
        &secret_root,
        format!("http://{}/v1", upstream.address()),
        9,
    )?;
    let router_address = router.address;

    let responses = send_http_request(
        router_address,
        "POST",
        "/v1/responses?route_native=1",
        br#"{"model":"gpt-5","input":"route-native prompt"}"#,
    )?;
    assert_success_response(&responses, "POST /v1/responses")?;

    let models = send_http_request(router_address, "GET", "/v1/models", b"")?;
    assert_success_response(&models, "GET /v1/models")?;

    let memory = send_http_request(
        router_address,
        "POST",
        "/v1/memories/trace_summarize",
        br#"{"input":"trace summary canary"}"#,
    )?;
    assert_success_response(&memory, "POST /v1/memories/trace_summarize")?;

    let compact = send_http_request(
        router_address,
        "POST",
        "/v1/responses/compact",
        br#"{"input":"compact canary"}"#,
    )?;
    assert_success_response(&compact, "POST /v1/responses/compact")?;

    let websocket_message =
        send_websocket_request(router_address, "/v1/responses", Some(LOCAL_TOKEN))?;
    if !websocket_message.contains("response.completed") {
        return Err(format!(
            "route-native websocket did not receive completion frame: {websocket_message}"
        ));
    }

    let unsupported_http = send_http_request(router_address, "POST", "/v1/unsupported", b"{}")?;
    assert_rejected_locally(&unsupported_http, "unsupported HTTP path")?;

    let wrong_method = send_http_request(router_address, "GET", "/v1/responses", b"")?;
    assert_rejected_locally(&wrong_method, "wrong HTTP method")?;

    let unsupported_websocket_error = require_websocket_rejection(
        send_websocket_request(router_address, "/v1/realtime", Some(LOCAL_TOKEN)),
        "unsupported websocket path",
    )?;
    if !is_local_websocket_rejection(&unsupported_websocket_error) {
        return Err(format!(
            "unsupported websocket path should reject before 101 upgrade or connect: {unsupported_websocket_error}"
        ));
    }

    let invalid_auth_error = require_websocket_rejection(
        send_websocket_request(router_address, "/v1/responses", None),
        "missing websocket auth",
    )?;
    if !is_local_websocket_rejection(&invalid_auth_error) {
        return Err(format!(
            "invalid websocket auth should reject before 101 upgrade or connect: {invalid_auth_error}"
        ));
    }

    router.join()?;
    let transcript = upstream.join()?;
    assert_route_native_transcript(&transcript)?;

    Ok(RouteNativeReport { transcript })
}

/// Route-native black-box proof report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteNativeReport {
    transcript: RouteNativeTranscript,
}

impl RouteNativeReport {
    /// Returns the redacted upstream transcript.
    #[must_use]
    pub const fn transcript(&self) -> &RouteNativeTranscript {
        &self.transcript
    }
}

#[cfg(test)]
mod tests {
    use super::run_route_native_black_box;

    #[test]
    #[ignore = "T8b route-native black-box proof"]
    fn route_native_black_box_all_supported_routes_and_rejections() {
        let report = match run_route_native_black_box() {
            Ok(report) => report,
            Err(error) => panic!("route-native black-box proof failed: {error}"),
        };

        assert_eq!(report.transcript().requests().len(), 5);
    }
}
