//! R5: real loopback HTTP requests retain rmcp's Host and Origin guards.

use crate::{
    ProxyAuthority,
    api_test_harness::{ServedApi, TEST_PROTOCOL_VERSION, api_config, test_identity},
};
use collaboration_service::CollaborationApplication;
use reqwest::{
    StatusCode,
    header::{ACCEPT, CONTENT_TYPE, HOST, ORIGIN},
};
use serde_json::json;
use std::num::NonZeroU16;

async fn served_api(port: Option<u16>) -> (tempfile::TempDir, ServedApi) {
    let directory = tempfile::tempdir().expect("service directory");
    let config = api_config(
        CollaborationApplication::new(test_identity()),
        directory.path(),
    );
    let proxy = port.map(|port| {
        ProxyAuthority::new(
            "node.example.ts.net".to_owned(),
            NonZeroU16::new(port).expect("port"),
        )
        .expect("validated proxy")
    });
    let api = ServedApi::tcp_with_proxy_authority(&config, "127.0.0.1:0", None, proxy).await;
    (directory, api)
}

async fn tools_status(api: &ServedApi, host: &str, origin: Option<&str>) -> StatusCode {
    let mut request = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("client")
        .post(api.url())
        .header(HOST, host)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("MCP-Protocol-Version", TEST_PROTOCOL_VERSION)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}));
    if let Some(origin) = origin {
        request = request.header(ORIGIN, origin);
    }
    let response = request.send().await.expect("real HTTP request");
    let status = response.status();
    if status == StatusCode::OK {
        let body: serde_json::Value = response.json().await.expect("actual MCP response");
        assert!(
            body["result"]["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .any(|tool| tool["name"] == "board_project_list")
        );
    }
    status
}

#[tokio::test]
async fn opted_in_proxy_accepts_only_the_exact_nondefault_host_and_port() {
    let (_directory, api) = served_api(Some(8443)).await;
    assert_eq!(
        tools_status(&api, "node.example.ts.net:8443", None).await,
        StatusCode::OK
    );
    for forbidden in [
        "node.example.ts.net",
        "node.example.ts.net:9443",
        "other.example.ts.net:8443",
    ] {
        assert_eq!(
            tools_status(&api, forbidden, None).await,
            StatusCode::FORBIDDEN,
            "{forbidden}"
        );
    }
    api.stop().await;
}

#[tokio::test]
async fn default_mode_adds_no_private_proxy_authority() {
    let (_directory, api) = served_api(None).await;
    assert_eq!(
        tools_status(&api, "node.example.ts.net:8443", None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(tools_status(&api, "localhost", None).await, StatusCode::OK);
    api.stop().await;
}

#[tokio::test]
async fn port_443_normalizes_only_the_exact_bare_node_host() {
    let (_directory, api) = served_api(Some(443)).await;
    for allowed in ["node.example.ts.net:443", "node.example.ts.net"] {
        assert_eq!(
            tools_status(&api, allowed, None).await,
            StatusCode::OK,
            "{allowed}"
        );
    }
    for forbidden in [
        "node.example.ts.net:8443",
        "other.example.ts.net",
        "other.example.ts.net:443",
    ] {
        assert_eq!(
            tools_status(&api, forbidden, None).await,
            StatusCode::FORBIDDEN,
            "{forbidden}"
        );
    }
    api.stop().await;
}

#[tokio::test]
async fn proxy_host_permission_does_not_grant_a_remote_browser_origin() {
    let (_directory, api) = served_api(Some(8443)).await;
    assert_eq!(
        tools_status(
            &api,
            "node.example.ts.net:8443",
            Some("https://node.example.ts.net:8443")
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        tools_status(&api, "node.example.ts.net:8443", None).await,
        StatusCode::OK
    );
    let origin = api
        .url()
        .strip_suffix("/mcp")
        .expect("MCP suffix")
        .to_owned();
    assert_eq!(
        tools_status(&api, "node.example.ts.net:8443", Some(&origin)).await,
        StatusCode::OK
    );
    api.stop().await;
}

// This fixture omits managed native definitions; final normal Host readiness remains a live gate.
#[tokio::test]
async fn metadata_fixture_inventory_fits_the_private_https_readiness_response_bound() {
    let (_directory, api) = served_api(None).await;
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("client")
        .post(api.url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("MCP-Protocol-Version", TEST_PROTOCOL_VERSION)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .send()
        .await
        .expect("actual inventory request");
    assert_eq!(response.status(), StatusCode::OK);
    let encoded = response.bytes().await.expect("actual inventory body");
    eprintln!("Actual MCP tools/list body: {} bytes", encoded.len());
    // The Host's bounded readiness client must be able to read the real published schema.
    assert!(
        encoded.len() <= 1024 * 1024,
        "published inventory exceeds the Host readiness response bound"
    );
    api.stop().await;
}
