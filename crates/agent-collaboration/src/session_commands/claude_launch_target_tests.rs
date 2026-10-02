use super::*;
use crate::CliContext;
use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderStartup,
};
use collaboration_protocol::ProviderKind;
use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt as _, path::PathBuf};
use tokio::net::TcpListener;

const SESSION_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";

fn service_manifest(router_proxy_endpoint: Option<&str>) -> serde_json::Value {
    let mut manifest = serde_json::json!({
        "version": 2,
        "serviceId": "00000000-0000-4000-8000-000000000001",
        "machineLabel": "fixture-machine",
        "serviceEpoch": "00000000-0000-4000-8000-000000000002",
        "control": {"transport": "unixJsonLines", "path": "control.sock"},
        "controlSchemaDigest": format!("sha256:{}", "a".repeat(64)),
        "mcp": {
            "transport": "streamableHttp",
            "url": "http://127.0.0.1:18788/mcp"
        }
    });
    if let Some(router_proxy_endpoint) = router_proxy_endpoint {
        manifest["routerProxyEndpoint"] = serde_json::json!(router_proxy_endpoint);
    }
    manifest
}

#[test]
fn service_manifest_router_proxy_endpoint_round_trips_a_custom_port() {
    let directory = tempfile::tempdir().expect("service directory");
    let manifest_path = directory.path().join("service.json");
    fs::write(
        &manifest_path,
        service_manifest(Some("127.0.0.1:18787")).to_string(),
    )
    .expect("service manifest");

    let endpoint = parse_router_proxy_endpoint(&manifest_path).expect("Router proxy endpoint");

    assert_eq!(endpoint, "127.0.0.1:18787".parse().expect("socket address"));
}

#[test]
fn missing_service_manifest_router_endpoint_reports_host_restart_action() {
    let directory = tempfile::tempdir().expect("service directory");
    let manifest_path = directory.path().join("service.json");
    fs::write(&manifest_path, service_manifest(None).to_string()).expect("service manifest");

    let error = parse_router_proxy_endpoint(&manifest_path)
        .expect_err("missing Router endpoint must fail clearly");

    assert_eq!(error, ROUTER_PROXY_ENDPOINT_NOT_PUBLISHED);
}

#[test]
fn non_file_service_manifest_is_rejected() {
    let directory = tempfile::tempdir().expect("service directory");
    let manifest_path = directory.path().join("service.json");
    fs::create_dir(&manifest_path).expect("manifest path directory");

    let error = parse_router_proxy_endpoint(&manifest_path)
        .expect_err("a service manifest path must be a regular file");

    assert_eq!(
        error,
        "Router service discovery is invalid; restart the Router Host"
    );
}

#[test]
fn oversized_service_manifest_is_rejected() {
    let directory = tempfile::tempdir().expect("service directory");
    let manifest_path = directory.path().join("service.json");
    let oversized_manifest = vec![b' '; (MAX_SERVICE_MANIFEST_BYTES + 1) as usize];
    fs::write(&manifest_path, oversized_manifest).expect("oversized manifest fixture");

    let error = parse_router_proxy_endpoint(&manifest_path)
        .expect_err("oversized service manifests must not be read");

    assert_eq!(
        error,
        "Router service discovery is invalid; restart the Router Host"
    );
}

#[test]
fn unsupported_service_manifest_version_fails_as_invalid_discovery() {
    let directory = tempfile::tempdir().expect("service directory");
    let manifest_path = directory.path().join("service.json");
    let mut manifest = service_manifest(Some("127.0.0.1:18787"));
    manifest["version"] = serde_json::json!(1);
    fs::write(&manifest_path, manifest.to_string()).expect("service manifest");

    let error = parse_router_proxy_endpoint(&manifest_path)
        .expect_err("endpoint from an unsupported manifest version must not be used");

    assert_eq!(
        error,
        "Router service discovery is invalid; restart the Router Host"
    );
}

#[tokio::test]
async fn health_preflight_sends_get_healthz_to_the_published_router_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("health fixture listener");
    let endpoint = listener.local_addr().expect("health fixture address");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("health request accepted");
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0_u8; 1];
            let count = stream.read(&mut byte).await.expect("request byte");
            assert_ne!(count, 0, "request ended before headers were complete");
            request.push(byte[0]);
        }
        assert!(request.starts_with(b"GET /healthz HTTP/1.1\r\n"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
            .await
            .expect("health response");
    });

    preflight_router_health(endpoint)
        .await
        .expect("Router health preflight");
    server.await.expect("health fixture task");
}

#[tokio::test]
async fn routed_environment_uses_the_published_listener_and_local_router_token() {
    let home = tempfile::tempdir().expect("isolated home");
    let router_root = home.path().join(".codex-router");
    let service_directory = router_root.join("agent-communication");
    let secret_root = router_root.join("secrets");
    fs::create_dir_all(&service_directory).expect("service directory");
    fs::create_dir_all(&secret_root).expect("Router secret root");
    fs::write(
        secret_root.join(LOCAL_ROUTER_TOKEN_FILE),
        "router-token-canary",
    )
    .expect("local Router token");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Router health fixture");
    let endpoint = listener.local_addr().expect("Router health address");
    fs::write(
        service_directory.join("service.json"),
        service_manifest(Some(&endpoint.to_string())).to_string(),
    )
    .expect("service manifest");
    let health_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("health request");
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0_u8; 1];
            let count = stream.read(&mut byte).await.expect("request byte");
            assert_ne!(count, 0, "health request completed headers");
            request.push(byte[0]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
            .await
            .expect("health response");
    });
    let context = CliContext::new(vec![(
        "HOME".to_owned(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let launch_target = ClaudeLaunchTarget::resolve(&context).expect("Claude launch target");

    let environment = launch_target
        .routed_environment()
        .await
        .expect("routed Claude environment");
    health_server.await.expect("Router health fixture task");

    assert_eq!(environment.base_url, format!("http://{endpoint}/anthropic"));
    assert_eq!(environment.auth_token, "router-token-canary");
}

#[tokio::test]
async fn missing_local_token_fails_after_router_health_preflight() {
    let home = tempfile::tempdir().expect("isolated home");
    let router_root = home.path().join(".codex-router");
    let service_directory = router_root.join("agent-communication");
    fs::create_dir_all(&service_directory).expect("service directory");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Router health fixture");
    let endpoint = listener.local_addr().expect("Router health address");
    fs::write(
        service_directory.join("service.json"),
        service_manifest(Some(&endpoint.to_string())).to_string(),
    )
    .expect("service manifest");
    let health_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("health request");
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0_u8; 1];
            let count = stream.read(&mut byte).await.expect("request byte");
            assert_ne!(count, 0, "health request completed headers");
            request.push(byte[0]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
            .await
            .expect("health response");
    });
    let context = CliContext::new(vec![(
        "HOME".to_owned(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let launch_target = ClaudeLaunchTarget::resolve(&context).expect("Claude launch target");

    let error = match launch_target.routed_environment().await {
        Ok(_environment) => panic!("a routed launch cannot fall back to the owner's Claude login"),
        Err(error) => error,
    };
    health_server.await.expect("Router health fixture task");

    assert!(error.to_string().contains("Router is not ready"));
    assert!(error.to_string().contains("local Router token"));
}

#[tokio::test]
async fn new_and_resume_commands_set_routed_environment_on_child_command_only() {
    let launch_target = ClaudeLaunchTarget {
        service_directory: PathBuf::new(),
        secret_root: PathBuf::new(),
        claude_projects_directory: None,
    };
    let routed_environment = RoutedClaudeEnvironment {
        base_url: "http://127.0.0.1:18787/anthropic".to_owned(),
        auth_token: "router-token-canary".to_owned(),
    };
    let parent_auth_token = std::env::var_os(ANTHROPIC_AUTH_TOKEN_ENV);
    let new_command = launch_target.command(
        None,
        &[OsString::from("--verbose")],
        None,
        RoutedClaudeEnvironment {
            base_url: routed_environment.base_url.clone(),
            auth_token: routed_environment.auth_token.clone(),
        },
    );
    let recorded_working_directory = PathBuf::from("/repo/project");
    let resume_command = launch_target.command(
        Some(SESSION_ID),
        &[],
        Some(&recorded_working_directory),
        routed_environment,
    );

    assert_eq!(
        new_command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["--verbose"]
    );
    assert_eq!(
        resume_command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["--resume", SESSION_ID]
    );
    assert_eq!(new_command.get_current_dir(), None);
    assert_eq!(
        resume_command.get_current_dir(),
        Some(recorded_working_directory.as_path())
    );
    let mut resume_environment = resume_command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect::<Vec<_>>();
    resume_environment.sort_by(|(left_name, _), (right_name, _)| left_name.cmp(right_name));
    assert_eq!(
        resume_environment,
        vec![
            (
                ANTHROPIC_AUTH_TOKEN_ENV.to_owned(),
                Some("router-token-canary".to_owned()),
            ),
            (
                ANTHROPIC_BASE_URL_ENV.to_owned(),
                Some("http://127.0.0.1:18787/anthropic".to_owned()),
            ),
        ]
    );
    assert_eq!(
        std::env::var_os(ANTHROPIC_AUTH_TOKEN_ENV),
        parent_auth_token
    );
}

#[test]
fn stored_transcript_inventory_reads_project_metadata_without_printing_content() {
    let home = tempfile::tempdir().expect("Claude home");
    let project_directory = home.path().join(".claude/projects/project-encoded");
    fs::create_dir_all(&project_directory).expect("Claude project directory");
    fs::write(
        project_directory.join(format!("{SESSION_ID}.jsonl")),
        b"owner transcript content must not be listed",
    )
    .expect("Claude transcript fixture");

    let records = list_stored_transcript_records(&home.path().join(".claude/projects"))
        .expect("stored transcript metadata");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["source"], "storedTranscript");
    assert_eq!(records[0]["sessionId"], SESSION_ID);
    assert_eq!(records[0]["project"], "project-encoded");
    assert!(
        serde_json::to_string(&records)
            .expect("record JSON")
            .find("owner transcript content")
            .is_none()
    );
}

#[test]
fn claude_resume_uses_working_directory_present_in_the_existing_listing() {
    let records = vec![
        serde_json::json!({
            "source": "storedTranscript",
            "sessionId": SESSION_ID,
            "project": "project-encoded"
        }),
        serde_json::json!({
            "source": "active",
            "sessionId": SESSION_ID,
            "workingDirectory": "/repo/original-project"
        }),
    ];

    let working_directory = recorded_working_directory(&records, SESSION_ID)
        .expect("resume uses the recorded listing directory");

    assert_eq!(working_directory, PathBuf::from("/repo/original-project"));
}

#[test]
fn claude_resume_without_listed_working_directory_inherits_invoking_directory() {
    let records = vec![serde_json::json!({
        "source": "storedTranscript",
        "sessionId": SESSION_ID,
        "project": "project-encoded"
    })];

    assert_eq!(recorded_working_directory(&records, SESSION_ID), None);

    let launch_target = ClaudeLaunchTarget {
        service_directory: PathBuf::new(),
        secret_root: PathBuf::new(),
        claude_projects_directory: None,
    };
    let command = launch_target.command(
        Some(SESSION_ID),
        &[],
        recorded_working_directory(&records, SESSION_ID).as_deref(),
        RoutedClaudeEnvironment {
            base_url: "http://127.0.0.1:18787/anthropic".to_owned(),
            auth_token: "router-token-canary".to_owned(),
        },
    );

    assert_eq!(command.get_current_dir(), None);
}

#[tokio::test]
async fn unreadable_claude_session_listing_keeps_resume_on_invoking_directory() {
    let home = tempfile::tempdir().expect("Claude home");
    let projects_path = home.path().join("projects-is-a-file");
    fs::write(&projects_path, b"not a directory").expect("invalid project inventory path");
    let launch_target = ClaudeLaunchTarget {
        service_directory: PathBuf::new(),
        secret_root: PathBuf::new(),
        claude_projects_directory: Some(projects_path),
    };

    let working_directory = launch_target.resume_working_directory(SESSION_ID).await;
    let command = launch_target.command(
        Some(SESSION_ID),
        &[],
        working_directory.as_deref(),
        RoutedClaudeEnvironment {
            base_url: "http://127.0.0.1:18787/anthropic".to_owned(),
            auth_token: "router-token-canary".to_owned(),
        },
    );

    assert_eq!(working_directory, None);
    assert_eq!(command.get_current_dir(), None);
}

#[test]
fn active_inventory_records_include_only_local_claude_interactive_sessions() {
    let summary: ProviderSessionSummary = serde_json::from_value(serde_json::json!({
        "origin": "claudeCodeInteractive",
        "target": {
            "endpoint": {
                "serviceId": "00000000-0000-4000-8000-000000000001",
                "endpointId": "claude-local"
            },
            "sessionId": SESSION_ID
        },
        "name": "Active Claude session",
        "workingDirectory": "/repo/project",
        "status": "busy",
        "startedAt": 1_760_000_000,
        "updatedAt": 1_760_000_001,
        "statusUpdatedAt": null,
        "kind": "interactive",
        "entrypoint": "cli"
    }))
    .expect("active Claude session summary");

    let record = active_claude_session_record(summary).expect("active Claude record");

    assert_eq!(record["source"], "active");
    assert_eq!(record["sessionId"], SESSION_ID);
    assert_eq!(record["workingDirectory"], "/repo/project");
}

#[tokio::test]
async fn active_inventory_uses_the_existing_claude_local_discovery() {
    let root = tempfile::tempdir().expect("runtime root");
    let service_directory = root.path().join("agent-communication");
    let peer_registry_directory = root.path().join("claude-sessions");
    fs::create_dir(&service_directory).expect("private service directory");
    fs::set_permissions(&service_directory, fs::Permissions::from_mode(0o700))
        .expect("service directory permissions");
    fs::create_dir_all(&peer_registry_directory).expect("Claude peer registry");
    fs::write(
        peer_registry_directory.join(format!("{}.json", std::process::id())),
        serde_json::json!({
            "pid": std::process::id(),
            "sessionId": SESSION_ID,
            "name": "Active local Claude",
            "cwd": root.path().to_string_lossy(),
            "status": "busy",
            "startedAt": 1_760_000_000_000_i64,
            "updatedAt": 1_760_000_001_000_i64,
            "statusUpdatedAt": 1_760_000_001_000_i64,
            "kind": "interactive",
            "entrypoint": "cli",
            "peerProtocol": 1,
            "messagingSocketPath": root.path().join("claude-peer.sock").to_string_lossy(),
        })
        .to_string(),
    )
    .expect("Claude peer registry record");
    let claude_endpoint = ExternalProviderStartup::unavailable(
        ProviderKind::ClaudeCode,
        "Claude ACP fixture is not required for peer inventory".to_owned(),
        "fixture endpoint".to_owned(),
    )
    .expect("Claude endpoint fixture");
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: service_directory.clone(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: "127.0.0.1:0".parse().expect("MCP bind"),
            native_schema: None,
            peer_registry_directory: Some(peer_registry_directory),
            remote_control_server_name: None,
            owner_human_id: None,
        },
        vec![claude_endpoint],
    )
    .await
    .expect("collaboration runtime");
    let launch_target = ClaudeLaunchTarget {
        service_directory,
        secret_root: root.path().join("secrets"),
        claude_projects_directory: None,
    };

    let inventory = launch_target.active_session_records(10).await;
    runtime.shutdown().await.expect("runtime shutdown");
    let records = inventory.expect("active Claude inventory");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["sessionId"], SESSION_ID);
    assert_eq!(records[0]["source"], "active");
    assert_eq!(records[0]["name"], "Active local Claude");
}
