use codex_acp_adapter::{
    AcpConnectionContext, AcpRouteFuture, AcpRouterChannels, AcpSessionRoute,
    serve_acp_router_connection,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct ScriptedProviderRoute;

struct ScriptedCodexRoute {
    load_calls: Arc<AtomicUsize>,
}

impl AcpSessionRoute for ScriptedCodexRoute {
    fn endpoint_id(&self) -> &str {
        "codex-local"
    }

    fn run(
        self: Box<Self>,
        mut router: AcpRouterChannels,
        _: AcpConnectionContext,
    ) -> AcpRouteFuture {
        Box::pin(async move {
            while let Some(frame) = router.input.recv().await {
                if frame.get("method") == Some(&json!("session/load")) {
                    self.load_calls.fetch_add(1, Ordering::SeqCst);
                    router
                        .output
                        .send(json!({"jsonrpc":"2.0","id":frame.get("id"),
                        "result":{"sessionId":"wrong-codex-route"}}))
                        .await?;
                }
            }
            Ok(())
        })
    }
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn unknown_bare_session_load_does_not_default_to_codex_route()
-> Result<(), Box<dyn std::error::Error>> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let load_calls = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(serve_acp_router_connection(
        server,
        vec![
            Box::new(ScriptedCodexRoute {
                load_calls: Arc::clone(&load_calls),
            }),
            Box::new(ScriptedProviderRoute),
        ],
    ));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await?;
    let initialized: Value =
        serde_json::from_str(&lines.next_line().await?.ok_or("initialize response")?)?;
    assert_eq!(initialized["id"], 1);
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/load\",\"params\":{\"sessionId\":\"unknown-provider-session\",\"cwd\":\"/tmp\",\"mcpServers\":[]}}\n").await?;
    let rejected: Value = serde_json::from_str(&lines.next_line().await?.ok_or("load response")?)?;
    assert_eq!(rejected["id"], 2);
    assert_eq!(rejected["error"]["code"], -32602);
    assert_eq!(load_calls.load(Ordering::SeqCst), 0);
    writer.shutdown().await?;
    task.await??;
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn explicit_codex_session_ref_routes_load_on_a_new_multi_route_connection()
-> Result<(), Box<dyn std::error::Error>> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let load_calls = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(serve_acp_router_connection(
        server,
        vec![
            Box::new(ScriptedCodexRoute {
                load_calls: Arc::clone(&load_calls),
            }),
            Box::new(ScriptedProviderRoute),
        ],
    ));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await?;
    let _: Value = serde_json::from_str(&lines.next_line().await?.ok_or("initialize")?)?;
    let load = json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{
        "sessionId":"stored-native-session","cwd":"/tmp","mcpServers":[],
        "_meta":{"router":{"sessionRef":{"endpoint":{
            "serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"
        },"sessionId":"stored-native-session"}}}
    }});
    writer.write_all(format!("{load}\n").as_bytes()).await?;
    let loaded: Value = serde_json::from_str(&lines.next_line().await?.ok_or("load")?)?;
    assert_eq!(loaded["result"]["sessionId"], "wrong-codex-route");
    assert_eq!(load_calls.load(Ordering::SeqCst), 1);
    writer.shutdown().await?;
    task.await??;
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn single_native_route_can_validate_a_bare_stored_session_load()
-> Result<(), Box<dyn std::error::Error>> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let load_calls = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(serve_acp_router_connection(
        server,
        vec![Box::new(ScriptedCodexRoute {
            load_calls: Arc::clone(&load_calls),
        })],
    ));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await?;
    let _: Value = serde_json::from_str(&lines.next_line().await?.ok_or("initialize")?)?;
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/load\",\"params\":{\"sessionId\":\"stored-native-session\",\"cwd\":\"/tmp\",\"mcpServers\":[]}}\n").await?;
    let loaded: Value = serde_json::from_str(&lines.next_line().await?.ok_or("load")?)?;
    assert_eq!(loaded["result"]["sessionId"], "wrong-codex-route");
    assert_eq!(load_calls.load(Ordering::SeqCst), 1);
    writer.shutdown().await?;
    task.await??;
    Ok(())
}

impl AcpSessionRoute for ScriptedProviderRoute {
    fn endpoint_id(&self) -> &str {
        "claude-local"
    }

    fn run(
        self: Box<Self>,
        mut router: AcpRouterChannels,
        _: AcpConnectionContext,
    ) -> AcpRouteFuture {
        Box::pin(async move {
            while let Some(frame) = router.input.recv().await {
                if frame.get("method") == Some(&json!("session/new")) {
                    router
                        .output
                        .send(json!({"jsonrpc":"2.0","id":frame.get("id"),
                        "result":{"sessionId":"provider-session"}}))
                        .await?;
                    router.output.send(json!({"jsonrpc":"2.0","id":"approval-1",
                        "method":"session/request_permission","params":{"sessionId":"provider-session"}})).await?;
                } else if frame.get("id") == Some(&json!("approval-1"))
                    && frame.get("result").is_some()
                {
                    router
                        .output
                        .send(json!({"jsonrpc":"2.0","method":"test/replyObserved",
                        "params":{"selection":frame.pointer("/result/selection")}}))
                        .await?;
                }
            }
            Ok(())
        })
    }
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn server_initiated_approval_reply_returns_to_owning_session_route()
-> Result<(), Box<dyn std::error::Error>> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_acp_router_connection(
        server,
        vec![Box::new(ScriptedProviderRoute)],
    ));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await?;
    let initialized: Value =
        serde_json::from_str(&lines.next_line().await?.ok_or("initialize response")?)?;
    assert_eq!(initialized["result"]["protocolVersion"], 1);
    writer
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":2,"method":"session/new",
        "params":{"cwd":"/tmp","mcpServers":[],"_meta":{"router":{"endpoint":"claude-local"}}}})
            )
            .as_bytes(),
        )
        .await?;
    let created: Value = serde_json::from_str(&lines.next_line().await?.ok_or("create response")?)?;
    assert_eq!(created["result"]["sessionId"], "provider-session");
    let approval: Value =
        serde_json::from_str(&lines.next_line().await?.ok_or("approval request")?)?;
    assert_eq!(approval["id"], "approval-1");
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"approval-1\",\"result\":{\"selection\":\"allow-once\"}}\n").await?;
    let observed: Value = serde_json::from_str(&lines.next_line().await?.ok_or("routed reply")?)?;
    assert_eq!(observed["method"], "test/replyObserved");
    assert_eq!(observed["params"]["selection"], "allow-once");
    writer.shutdown().await?;
    task.await??;
    Ok(())
}
