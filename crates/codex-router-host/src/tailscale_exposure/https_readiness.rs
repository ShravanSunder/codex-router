//! Read-only MCP requests over ordinary verified TLS; no trust override or retry.

use collaboration_protocol::{EndpointInventory, UuidIdentity};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::{io, time::Duration};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct RpcEnvelope<TResult> {
    jsonrpc: String,
    id: u64,
    #[serde(flatten)]
    outcome: RpcOutcome<TResult>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RpcOutcome<TResult> {
    Success { result: TResult },
    Failure { error: RpcFailure },
}

#[derive(Deserialize)]
struct RpcFailure {
    code: i64,
}

#[derive(Deserialize)]
struct ToolInventory {
    tools: Vec<ToolName>,
}

#[derive(Deserialize)]
struct ToolName {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndpointAnswer {
    is_error: Option<bool>,
    structured_content: EndpointInventory,
}

pub(super) async fn verify(
    url: &str,
    service_id: &UuidIdentity,
    service_epoch: &UuidIdentity,
) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(75), async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(65))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(transport_failure)?;
        let tools: ToolInventory = call(&client, url, 1, "tools/list", json!({})).await?;
        if !tools.tools.iter().any(|tool| tool.name == "endpoints_list")
            || !tools
                .tools
                .iter()
                .any(|tool| tool.name == "board_project_list")
        {
            return Err(io::Error::other(
                "private HTTPS endpoint lacks the collaboration MCP inventory",
            ));
        }
        let answer: EndpointAnswer = call(
            &client,
            url,
            2,
            "tools/call",
            json!({"name":"endpoints_list","arguments":{}}),
        )
        .await?;
        validate_endpoint_identity(&answer, service_id, service_epoch)
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "private MCP HTTPS readiness timed out; no request was retried",
        )
    })?
}

#[derive(Debug)]
struct ReadinessTransportFailure(reqwest::Error);

impl std::fmt::Display for ReadinessTransportFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The URL is constructed from the validated private node authority. Only
        // error Display/source text is included, never request headers or bodies.
        write!(formatter, "private MCP HTTPS transport failed: {}", self.0)?;
        let mut cause = std::error::Error::source(&self.0);
        while let Some(error) = cause {
            write!(formatter, "; caused by: {error}")?;
            cause = error.source();
        }
        Ok(())
    }
}

impl std::error::Error for ReadinessTransportFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

fn transport_failure(error: reqwest::Error) -> io::Error {
    let kind = if error.is_timeout() {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(kind, ReadinessTransportFailure(error))
}

async fn call<TResult: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    id: u64,
    method: &str,
    parameters: serde_json::Value,
) -> io::Result<TResult> {
    let body =
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":parameters}))
            .map_err(io::Error::other)?;
    let mut response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .body(body)
        .send()
        .await
        .map_err(transport_failure)?
        .error_for_status()
        .map_err(transport_failure)?;
    let mut encoded = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_failure)? {
        if encoded.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(io::Error::other(
                "private MCP readiness response exceeded its size bound",
            ));
        }
        encoded.extend_from_slice(&chunk);
    }
    let response: RpcEnvelope<TResult> = serde_json::from_slice(&encoded).map_err(|_| {
        io::Error::other("private MCP readiness returned a malformed JSON-RPC response")
    })?;
    if response.jsonrpc != "2.0" || response.id != id {
        return Err(io::Error::other(
            "private MCP readiness response does not match its request",
        ));
    }
    match response.outcome {
        RpcOutcome::Success { result } => Ok(result),
        RpcOutcome::Failure { error } => Err(io::Error::other(format!(
            "private MCP readiness request was rejected ({})",
            error.code
        ))),
    }
}

fn validate_endpoint_identity(
    answer: &EndpointAnswer,
    service_id: &UuidIdentity,
    service_epoch: &UuidIdentity,
) -> io::Result<()> {
    if answer.is_error == Some(true)
        || answer.structured_content.service_epoch != *service_epoch
        || answer.structured_content.endpoints.is_empty()
        || answer
            .structured_content
            .endpoints
            .iter()
            .any(|endpoint| endpoint.endpoint.service_id != *service_id)
    {
        return Err(io::Error::other(
            "private HTTPS endpoint does not identify this Host invocation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
    const EPOCH: &str = "00000000-0000-4000-8000-000000000002";
    const OTHER: &str = "00000000-0000-4000-8000-000000000003";

    fn answer(service: &str, epoch: &str) -> EndpointAnswer {
        serde_json::from_value(json!({
            "isError": false,
            "structuredContent": {"serviceEpoch": epoch, "sequence": 0, "endpoints":[{
                "endpoint":{"serviceId":service,"endpointId":"codex-local"},
                "label":"identity fixture",
                "availability":{"state":"unavailable","observedAt":"2026-10-16T00:00:00Z","reason":"identity-only fixture","fix":null},
                "channels":[]
            }]}
        })).expect("typed inventory fixture")
    }

    #[test]
    fn tailscale_readiness_requires_the_intended_service_and_current_epoch() {
        let service = UuidIdentity::try_from(SERVICE.to_owned()).expect("service");
        let epoch = UuidIdentity::try_from(EPOCH.to_owned()).expect("epoch");
        assert!(validate_endpoint_identity(&answer(SERVICE, EPOCH), &service, &epoch).is_ok());
        assert!(validate_endpoint_identity(&answer(OTHER, EPOCH), &service, &epoch).is_err());
        assert!(validate_endpoint_identity(&answer(SERVICE, OTHER), &service, &epoch).is_err());
        let mut empty = answer(SERVICE, EPOCH);
        empty.structured_content.endpoints.clear();
        assert!(validate_endpoint_identity(&empty, &service, &epoch).is_err());
        let mut rejected = answer(SERVICE, EPOCH);
        rejected.is_error = Some(true);
        assert!(validate_endpoint_identity(&rejected, &service, &epoch).is_err());
    }
}

#[cfg(test)]
mod transport_error_tests {
    use super::*;
    #[tokio::test]
    async fn connection_failure_preserves_the_safe_root_cause_in_the_visible_error() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()
            .expect("client");
        let result = call::<ToolInventory>(
            &client,
            "http://127.0.0.1:0/mcp",
            1,
            "tools/list",
            json!({}),
        )
        .await;
        let error = match result {
            Ok(_) => panic!("unbound port0 must fail"),
            Err(error) => error,
        };
        let mut cause: &dyn std::error::Error = error.get_ref().expect("transport source retained");
        while let Some(source) = cause.source() {
            cause = source;
        }
        assert!(
            error.to_string().contains(&cause.to_string()),
            "visible error hides the root cause: {error}; root cause: {cause}"
        );
    }
}

#[cfg(test)]
#[path = "https_readiness_tests.rs"]
mod budget_tests;
