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
    tokio::time::timeout(Duration::from_secs(15), async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(io::Error::other)?;
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
        .map_err(io::Error::other)?
        .error_for_status()
        .map_err(io::Error::other)?;
    let mut encoded = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
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
