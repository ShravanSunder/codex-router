//! Real Host composition of provider-facing ACP and app-server sockets.
use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_protocol::{EndpointId, EndpointRef, SessionId, SessionRef};
use std::os::unix::fs::PermissionsExt as _;

async fn assert_provider_ready(directory: &std::path::Path) {
    let mut client = collaboration_client::ControlClient::connect(directory, "face-proof", "1")
        .await
        .expect("Control client");
    let inventory = client.list_endpoints().await.expect("endpoint inventory");
    let provider = inventory
        .endpoints
        .iter()
        .find(|endpoint| String::from(endpoint.endpoint.endpoint_id.clone()) == "claude-local")
        .expect("Claude endpoint");
    assert!(
        matches!(
            provider.availability,
            collaboration_protocol::EndpointAvailability::Available { .. }
        ),
        "provider unavailable: {:?}",
        provider.availability
    );
}

#[tokio::test]
async fn composed_acp_connection_admits_provider_without_codex_generation() {
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixStream,
        time::{Duration, timeout},
    };

    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let provider = root.path().join("acp-provider.py");
    std::fs::write(
        &provider,
        r#"#!/usr/bin/python3
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'acp-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'acp-provider-session'}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
    .expect("provider fixture");
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700))
        .expect("provider executable");
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            owner_human_id: Some(
                message_board::HumanId::try_from("acp-owner".to_owned()).expect("owner"),
            ),
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
        },
        vec![ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::claude(provider, Vec::new()).expect("provider binding"),
        )],
    )
    .await
    .expect("Host starts provider face");
    assert_provider_ready(root.path()).await;
    let stream = UnixStream::connect(root.path().join("codex-acp.sock"))
        .await
        .expect("ACP socket");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":1,"clientCapabilities":{},"_meta":{
            "router":{"actor":{"kind":"human","humanId":"acp-owner"}},
            "sessionProfile":{"version":1,"elements":[]}
        }
    }});
    write
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .expect("initialize");
    let initialized: Value = serde_json::from_str(
        &timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("initialize deadline")
            .expect("read initialize")
            .expect("initialize response"),
    )
    .expect("initialize JSON");
    assert_eq!(initialized["id"], 1);
    write
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":2,
                    "method":"session/new","params":{"cwd":root.path(),"mcpServers":[],
                    "_meta":{"router":{"endpoint":"claude-local"}}}
                })
            )
            .as_bytes(),
        )
        .await
        .expect("provider new");
    let provider_result: Value = serde_json::from_str(
        &timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("provider deadline")
            .expect("read provider")
            .expect("provider response"),
    )
    .expect("provider JSON");
    assert_eq!(provider_result["id"], 2, "{provider_result}");
    assert_eq!(
        provider_result["result"]["sessionId"], "acp-provider-session",
        "{provider_result}"
    );
    assert_eq!(
        provider_result["result"]["_meta"]["router"]["approver"],
        json!({"kind":"human","humanId":"acp-owner"})
    );

    write
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":3,
                    "method":"session/new","params":{"cwd":root.path(),"mcpServers":[]}
                })
            )
            .as_bytes(),
        )
        .await
        .expect("Codex new");
    let codex_result: Value = serde_json::from_str(
        &timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("Codex deadline")
            .expect("read Codex")
            .expect("Codex response"),
    )
    .expect("Codex JSON");
    assert_eq!(codex_result["id"], 3, "{codex_result}");
    assert_eq!(codex_result["error"]["code"], -32000);
    runtime.shutdown().await.expect("Host shutdown");
}

#[tokio::test]
async fn app_server_socket_creates_and_prompts_with_the_owner_identity() {
    use collaboration_service::ProviderOperationStore;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixStream,
        time::{Duration, timeout},
    };
    use tokio_tungstenite::{client_async, tungstenite::Message};

    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let provider = root.path().join("tui-provider.py");
    std::fs::write(&provider, r#"#!/usr/bin/python3
import json,sys
def receive(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value)); sys.stdout.flush()
request=receive()
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'tui-fixture','version':'1'}}})
request=receive()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'tui-provider-session'}})
request=receive()
assert request['method']=='session/prompt'
assert request['params']['prompt']==[{'type':'text','text':'hello from TUI'}]
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'tui-provider-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'fixture reply'}}}})
send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
request=receive()
assert request['method']=='session/prompt'
assert request['params']['prompt']==[{'type':'text','text':'queued by another agent'}]
send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
request=receive()
assert request['method']=='session/prompt'
assert request['params']['prompt']==[{'type':'text','text':'prompt by another agent'}]
send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#).expect("provider fixture");
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700))
        .expect("provider executable");
    let owner = message_board::HumanId::try_from("tui-owner".to_owned()).expect("owner HumanId");
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            owner_human_id: Some(owner),
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
        },
        vec![ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::claude(provider, Vec::new()).expect("provider binding"),
        )],
    )
    .await
    .expect("Host starts provider face");
    assert_provider_ready(root.path()).await;
    let socket = root.path().join("router-sessions/claude-local.sock");
    let stream = UnixStream::connect(socket)
        .await
        .expect("provider app-server socket");
    let (mut client, _) = client_async("ws://localhost/rpc", stream)
        .await
        .expect("websocket handshake");
    client
        .send(Message::Text(
            json!({"id":"trust","method":"config/read","params":{"includeLayers":true,"cwd":root.path()}})
                .to_string()
                .into(),
        ))
        .await
        .expect("trust read");
    let trust: Value = serde_json::from_str(
        client
            .next()
            .await
            .expect("trust response")
            .expect("frame")
            .to_text()
            .expect("text"),
    )
    .expect("trust JSON");
    assert_eq!(trust["result"]["config"]["projects"], json!({}));
    client
        .send(Message::Text(
            json!({"id":1,"method":"thread/start","params":{
                "cwd":root.path(),"model":"provider-default"
            }})
            .to_string()
            .into(),
        ))
        .await
        .expect("start request");
    let started: Value = timeout(Duration::from_secs(5), async {
        loop {
            let frame = client.next().await.expect("start response").expect("frame");
            let response: Value =
                serde_json::from_str(frame.to_text().expect("text frame")).expect("JSON response");
            if response["id"] == 1 {
                break response;
            }
        }
    })
    .await
    .expect("thread start deadline");
    let thread_id = started["result"]["thread"]["id"]
        .as_str()
        .expect("thread alias")
        .to_owned();
    let initial_status = started["result"]["thread"]["status"]["type"].clone();

    client
        .send(Message::Text(
            json!({"id":2,"method":"turn/start","params":{
                "threadId":thread_id,"input":[{"type":"text","text":"hello from TUI"}]
            }})
            .to_string()
            .into(),
        ))
        .await
        .expect("turn request");
    let mut streamed_reply = false;
    let mut turn_completed = false;
    let turn: Value = timeout(Duration::from_secs(5), async {
        loop {
            let frame = client.next().await.expect("turn response").expect("frame");
            let response: Value =
                serde_json::from_str(frame.to_text().expect("text frame")).expect("JSON response");
            streamed_reply |= response.to_string().contains("fixture reply");
            turn_completed |= response["method"] == "turn/completed";
            if response["id"] == 2 {
                break response;
            }
        }
    })
    .await
    .expect("turn start deadline");
    assert!(turn["result"]["turn"]["id"].as_str().is_some(), "{turn}");
    if !turn_completed {
        timeout(Duration::from_secs(5), async {
            loop {
                let frame = client.next().await.expect("turn event").expect("frame");
                let event: Value =
                    serde_json::from_str(frame.to_text().expect("text frame")).expect("JSON event");
                streamed_reply |= event.to_string().contains("fixture reply");
                if event["method"] == "turn/completed" {
                    break;
                }
            }
        })
        .await
        .expect("turn completion deadline");
    }
    assert!(
        streamed_reply,
        "agent reply must stream through the composed hub"
    );
    client
        .send(Message::Text(
            json!({"id":3,"method":"turn/interrupt","params":{
                "threadId":thread_id,"turnId":turn["result"]["turn"]["id"]
            }})
            .to_string()
            .into(),
        ))
        .await
        .expect("idle cancel request");
    let idle_cancel: Value = timeout(Duration::from_secs(5), async {
        loop {
            let frame = client
                .next()
                .await
                .expect("cancel response")
                .expect("frame");
            let response: Value =
                serde_json::from_str(frame.to_text().expect("text frame")).expect("JSON response");
            if response["id"] == 3 {
                break response;
            }
        }
    })
    .await
    .expect("idle cancel deadline");
    assert_eq!(idle_cancel["error"]["code"], -32000);
    let target = SessionRef {
        endpoint: EndpointRef {
            service_id: runtime.service_id().clone(),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint"),
        },
        session_id: SessionId::try_from("tui-provider-session".to_owned()).expect("session"),
    };
    let other_agent = SessionRef {
        endpoint: EndpointRef {
            service_id: runtime.service_id().clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("actor endpoint"),
        },
        session_id: SessionId::try_from("cross-agent".to_owned()).expect("actor session"),
    };
    let acp_stream = UnixStream::connect(root.path().join("codex-acp.sock"))
        .await
        .expect("ACP socket");
    let (acp_read, mut acp_write) = acp_stream.into_split();
    let mut acp_lines = BufReader::new(acp_read).lines();
    let initialize = json!({"jsonrpc":"2.0","id":10,"method":"initialize","params":{
        "protocolVersion":1,"clientCapabilities":{},
        "_meta":{"router":{"actor":{"kind":"session","session":other_agent}}}
    }});
    acp_write
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .expect("ACP initialize");
    let initialized: Value = serde_json::from_str(
        &timeout(Duration::from_secs(5), acp_lines.next_line())
            .await
            .expect("ACP initialize deadline")
            .expect("read ACP initialize")
            .expect("ACP initialize response"),
    )
    .expect("ACP initialize JSON");
    assert_eq!(initialized["id"], 10);
    let queue = json!({"jsonrpc":"2.0","id":11,"method":"_session/queue/add","params":{
        "sessionId":"tui-provider-session",
        "prompt":[{"type":"text","text":"queued by another agent"}],
        "_meta":{"router":{"sessionRef":target}}
    }});
    acp_write
        .write_all(format!("{queue}\n").as_bytes())
        .await
        .expect("cross-agent queue request");
    let queued: Value = timeout(Duration::from_secs(5), async {
        loop {
            let line = acp_lines
                .next_line()
                .await
                .expect("read ACP queue")
                .expect("ACP queue response");
            let frame: Value = serde_json::from_str(&line).expect("ACP queue JSON");
            if frame["id"] == 11 {
                break frame;
            }
        }
    })
    .await
    .expect("ACP queue deadline");
    assert!(queued["result"]["inputId"].as_str().is_some(), "{queued}");
    timeout(Duration::from_secs(5), async {
        loop {
            let frame = client
                .next()
                .await
                .expect("queued turn event")
                .expect("frame");
            let event: Value = serde_json::from_str(frame.to_text().expect("text frame"))
                .expect("JSON queued turn event");
            if event["method"] == "turn/completed" {
                break;
            }
        }
    })
    .await
    .expect("queued turn completion deadline");

    let load = json!({"jsonrpc":"2.0","id":12,"method":"session/load","params":{
        "sessionId":"tui-provider-session","cwd":root.path(),"mcpServers":[],
        "_meta":{"router":{"sessionRef":target}}
    }});
    acp_write
        .write_all(format!("{load}\n").as_bytes())
        .await
        .expect("noncreator attach request");
    let attached: Value = timeout(Duration::from_secs(5), async {
        loop {
            let line = acp_lines
                .next_line()
                .await
                .expect("read attach")
                .expect("attach response");
            let frame: Value = serde_json::from_str(&line).expect("attach JSON");
            if frame["id"] == 12 {
                break frame;
            }
        }
    })
    .await
    .expect("noncreator attach deadline");
    assert_eq!(
        attached["result"]["sessionId"], "tui-provider-session",
        "{attached}"
    );

    let prompt = json!({"jsonrpc":"2.0","id":13,"method":"session/prompt","params":{
        "sessionId":"tui-provider-session",
        "prompt":[{"type":"text","text":"prompt by another agent"}],
        "_meta":{"router":{"sessionRef":target}}
    }});
    acp_write
        .write_all(format!("{prompt}\n").as_bytes())
        .await
        .expect("noncreator prompt request");
    let prompted: Value = timeout(Duration::from_secs(5), async {
        loop {
            let line = acp_lines
                .next_line()
                .await
                .expect("read prompt")
                .expect("prompt response");
            let frame: Value = serde_json::from_str(&line).expect("prompt JSON");
            if frame["id"] == 13 {
                break frame;
            }
        }
    })
    .await
    .expect("noncreator prompt deadline");
    assert!(prompted["result"].is_object(), "{prompted}");
    client.close(None).await.expect("close websocket");
    runtime.shutdown().await.expect("Host shutdown");
    let mut store = ProviderOperationStore::open(&root.path().join("provider-operations.sqlite"))
        .await
        .expect("provider records");
    let record = store
        .session_record(&target)
        .await
        .expect("read session")
        .expect("TUI-created Session record");
    assert_eq!(
        serde_json::to_value(record.created_by).expect("identity JSON"),
        json!({"humanId":"tui-owner"})
    );
    assert_eq!(
        serde_json::to_value(record.approver).expect("identity JSON"),
        json!({"humanId":"tui-owner"})
    );
    assert_eq!(
        initial_status, "idle",
        "new provider Session must be loaded"
    );
}
