use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use std::path::PathBuf;

const CREATE_OPERATION: &str = "019f0000-0000-7000-8000-000000000101";
const PROMPT_OPERATION: &str = "019f0000-0000-7000-8000-000000000102";

async fn bounded_client_output(
    mut command: tokio::process::Command,
    label: &'static str,
) -> std::process::Output {
    command.kill_on_drop(true);
    tokio::time::timeout(std::time::Duration::from_secs(20), command.output())
        .await
        .unwrap_or_else(|_| panic!("{label} timed out"))
        .unwrap_or_else(|error| panic!("{label} failed to launch: {error}"))
}

/// Runs the installed clients' own no-model catalog paths against an owned
/// current-source Host. Both clients receive isolated configuration homes, so
/// the user's registered servers, approvals and authentication remain untouched.
#[tokio::test]
#[ignore = "requires installed Cursor and Claude clients plus loopback socket permission"]
async fn live_current_source_catalog_is_accepted_by_cursor_and_claude() {
    let root = tempfile::tempdir().expect("runtime root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private runtime root");
    }
    std::fs::create_dir_all(root.path().join("router")).expect("Router runtime directory");
    std::fs::create_dir_all(root.path().join("codex-home")).expect("isolated Codex home");
    #[cfg(unix)]
    for directory in [root.path().join("router"), root.path().join("codex-home")] {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .expect("private fixture directory");
    }
    let runtime = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.path().join("router"),
        codex_home: root.path().join("codex-home"),
        backend_socket: root.path().join("backend.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
    })
    .await
    .expect("current-source collaboration Host");
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_slice(
        &std::fs::read(root.path().join("router/service.json")).expect("service manifest"),
    )
    .expect("typed service manifest");

    let cursor_home = root.path().join("cursor-home");
    let cursor_project = root.path().join("cursor-project");
    std::fs::create_dir_all(cursor_project.join(".cursor")).expect("Cursor project config");
    std::fs::create_dir_all(&cursor_home).expect("Cursor isolated home");
    std::fs::write(
        cursor_project.join(".cursor/mcp.json"),
        serde_json::to_vec(&json!({
            "mcpServers": {
                "agent-collaboration-current-source": {"url": manifest.mcp.url}
            }
        }))
        .expect("Cursor config JSON"),
    )
    .expect("Cursor config write");
    let cursor_executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(PathBuf::from)
        .expect("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE");
    let mut cursor_enable_command = tokio::process::Command::new(&cursor_executable);
    cursor_enable_command
        .args(["mcp", "enable", "agent-collaboration-current-source"])
        .current_dir(&cursor_project)
        .env("HOME", &cursor_home);
    let cursor_enable = bounded_client_output(cursor_enable_command, "Cursor MCP consent").await;
    assert!(
        cursor_enable.status.success(),
        "Cursor MCP consent failed: {}",
        String::from_utf8_lossy(&cursor_enable.stderr)
    );
    let mut cursor_catalog_command = tokio::process::Command::new(&cursor_executable);
    cursor_catalog_command
        .args(["mcp", "list-tools", "agent-collaboration-current-source"])
        .current_dir(&cursor_project)
        .env("HOME", &cursor_home);
    let cursor_catalog = bounded_client_output(cursor_catalog_command, "Cursor tools/list").await;
    let cursor_stdout = String::from_utf8_lossy(&cursor_catalog.stdout);
    assert!(
        cursor_catalog.status.success(),
        "Cursor rejected current catalog: {}",
        String::from_utf8_lossy(&cursor_catalog.stderr)
    );
    assert!(
        cursor_stdout.contains("endpoints_list"),
        "Cursor catalog omitted endpoints_list"
    );

    let claude_home = root.path().join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("Claude isolated home");
    let mut claude_add_command = tokio::process::Command::new("claude");
    claude_add_command
        .args([
            "mcp",
            "add",
            "--scope",
            "user",
            "--transport",
            "http",
            "agent-collaboration-current-source",
            &manifest.mcp.url,
        ])
        .env("HOME", &claude_home);
    let claude_add = bounded_client_output(claude_add_command, "Claude MCP add").await;
    assert!(
        claude_add.status.success(),
        "Claude MCP registration failed: {}",
        String::from_utf8_lossy(&claude_add.stderr)
    );
    let mut claude_catalog_command = tokio::process::Command::new("claude");
    claude_catalog_command
        .args(["mcp", "get", "agent-collaboration-current-source"])
        .env("HOME", &claude_home);
    let claude_catalog = bounded_client_output(claude_catalog_command, "Claude MCP catalog").await;
    let claude_output = format!(
        "{}{}",
        String::from_utf8_lossy(&claude_catalog.stdout),
        String::from_utf8_lossy(&claude_catalog.stderr)
    );
    assert!(
        claude_catalog.status.success(),
        "Claude rejected current catalog: {claude_output}"
    );
    assert!(
        claude_output.contains("Connected") || claude_output.contains("connected"),
        "Claude did not report a connected current-source server"
    );

    runtime.shutdown().await.expect("runtime shutdown");
}

/// Proves the shared initialized HTTP surface against an explicitly selected
/// authenticated ACP provider. Provider-specific MCP execution has separate
/// acceptance and is intentionally not inferred from this journey.
#[tokio::test]
#[ignore = "requires an explicitly selected authenticated external ACP provider"]
async fn live_provider_create_and_prompt_through_initialized_http() {
    let executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(PathBuf::from)
        .expect("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE");
    let arguments = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_ARGUMENTS")
        .ok()
        .map(|encoded| serde_json::from_str::<Vec<String>>(&encoded).expect("JSON argument array"))
        .unwrap_or_default();
    let provider_cwd = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let provider_kind = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_PROVIDER")
        .unwrap_or_else(|_| "cursor".to_owned());
    let binding = match provider_kind.as_str() {
        "cursor" => ExternalProviderLaunchBinding::cursor(executable, arguments),
        "claude" => ExternalProviderLaunchBinding::claude(executable, arguments),
        other => panic!("unsupported live provider {other:?}; expected cursor or claude"),
    }
    .expect("external provider binding");

    let root = tempfile::tempdir().expect("runtime root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private runtime root");
    }
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
        },
        vec![codex_router_host::ExternalProviderStartup::Launch(binding)],
    )
    .await
    .expect("collaboration runtime with external provider");
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_slice(
        &std::fs::read(root.path().join("service.json")).expect("service manifest"),
    )
    .expect("typed service manifest");
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &manifest.mcp.url).await;
    let inventory = call_tool(
        &client,
        &manifest.mcp.url,
        &session,
        2,
        "endpoints_list",
        json!({}),
    )
    .await;
    let endpoints = inventory["result"]["structuredContent"]["endpoints"]
        .as_array()
        .expect("endpoint inventory");
    let expected_endpoint_id = if provider_kind == "cursor" {
        "cursor-local"
    } else {
        "claude-local"
    };
    let provider_endpoint = endpoints
        .iter()
        .find(|description| description["endpoint"]["endpointId"] == expected_endpoint_id)
        .expect("external provider endpoint");
    let endpoint = provider_endpoint["endpoint"].clone();
    let generation_number = provider_endpoint["channels"]
        .as_array()
        .expect("provider channels")
        .iter()
        .find(|channel| channel["kind"] == "externalProvider")
        .and_then(|channel| channel["bindingGeneration"].as_u64())
        .expect("provider binding generation");
    let generation = json!({"serviceEpoch":inventory["result"]["structuredContent"]["serviceEpoch"],"generation":generation_number});
    let actor = json!({"endpoint":{"serviceId":endpoint["serviceId"],"endpointId":"codex-local"},"sessionId":"mcp-live-provider-caller"});

    let create = call_tool(&client, &manifest.mcp.url, &session, 3, "provider_conversation_create", json!({"operationId":CREATE_OPERATION,"endpoint":endpoint,"generation":generation,"workingDirectory":provider_cwd,"createdBy":actor,"approver":actor,"requestedPolicy":{"access":"write-restricted"}})).await;
    assert_tool_success(&create, "create admission");
    let create_wait =
        wait_for_operation(&client, &manifest.mcp.url, &session, 4, CREATE_OPERATION).await;
    let target =
        create_wait["result"]["structuredContent"]["output"]["settlement"]["target"].clone();
    assert_eq!(
        create_wait["result"]["structuredContent"]["output"]["settlement"]["kind"],
        "created"
    );

    let prompt = call_tool(&client, &manifest.mcp.url, &session, 5, "provider_conversation_prompt", json!({"operationId":PROMPT_OPERATION,"target":target,"generation":generation,"requestedBy":actor,"approver":actor,"prompt":{"kind":"humanUser","text":"Reply with exactly PR2_MCP_LIVE_PROVIDER_OK and no other text."}})).await;
    assert_tool_success(&prompt, "prompt admission");
    let prompt_wait =
        wait_for_operation(&client, &manifest.mcp.url, &session, 6, PROMPT_OPERATION).await;
    assert_eq!(
        prompt_wait["result"]["structuredContent"]["output"]["settlement"]["target"],
        target
    );
    assert_eq!(
        prompt_wait["result"]["structuredContent"]["output"]["settlement"]["kind"],
        "promptCompleted"
    );
    assert_eq!(
        prompt_wait["result"]["structuredContent"]["output"]["settlement"]["response"],
        "PR2_MCP_LIVE_PROVIDER_OK"
    );
    runtime.shutdown().await.expect("runtime shutdown");
}

async fn wait_for_operation(
    client: &reqwest::Client,
    mcp_url: &str,
    session: &reqwest::header::HeaderValue,
    request_id: u64,
    operation_id: &str,
) -> Value {
    let response = call_tool(
        client,
        mcp_url,
        session,
        request_id,
        "provider_conversation_operation_wait",
        json!({"operationId":operation_id,"timeoutSeconds":20}),
    )
    .await;
    assert_tool_success(&response, "operation wait");
    response
}

fn assert_tool_success(response: &Value, label: &str) {
    assert_eq!(
        response["result"]["isError"], false,
        "{label} failed: {response}"
    );
}

async fn initialize_mcp(client: &reqwest::Client, mcp_url: &str) -> reqwest::header::HeaderValue {
    let response = client.post(mcp_url).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"provider-live-http-test","version":"1"}}})).send().await.expect("MCP initialize");
    assert!(response.status().is_success(), "MCP initialize status");
    let session = response
        .headers()
        .get("mcp-session-id")
        .expect("MCP session header")
        .clone();
    let initialized = client
        .post(mcp_url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
        .send()
        .await
        .expect("MCP initialized notification");
    assert!(initialized.status().is_success(), "MCP initialized status");
    session
}

async fn call_tool(
    client: &reqwest::Client,
    mcp_url: &str,
    session: &reqwest::header::HeaderValue,
    request_id: u64,
    tool_name: &str,
    arguments: Value,
) -> Value {
    let response = client.post(mcp_url).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-session-id", session).header("mcp-protocol-version", "2025-11-25").json(&json!({"jsonrpc":"2.0","id":request_id,"method":"tools/call","params":{"name":tool_name,"arguments":arguments}})).send().await.expect("MCP tools/call");
    protocol_response_json(response).await
}

async fn protocol_response_json(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.text().await.expect("protocol body");
    assert!(
        !body.is_empty(),
        "empty protocol response with status {status}"
    );
    let payload = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(body.as_str());
    serde_json::from_str(payload).expect("protocol JSON")
}
