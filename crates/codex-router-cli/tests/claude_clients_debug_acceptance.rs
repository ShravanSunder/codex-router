#[path = "support/claude_clients_debug_acceptance.rs"]
mod claude_clients_debug_acceptance;
#[path = "support/claude_fake_anthropic.rs"]
mod claude_fake_anthropic;

use claude_clients_debug_acceptance::{
    ACP_PROMPT_MARKER, HostProcess, IsolatedAcceptanceRoot, NEW_PROMPT_MARKER,
    RESUME_PROMPT_MARKER, SYNTHETIC_ACCOUNT_ACCESS, SYNTHETIC_ACCOUNT_REFRESH, UPSTREAM_ADDRESS,
};
use claude_fake_anthropic::{
    FakeAnthropicServer, ObservedRequest, USAGE_LIMIT_PROMPT_MARKER, is_messages_request_target,
};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

const ACCEPTANCE_RESPONSE_MARKER: &str = "CLAUDE-ACCEPTANCE-OK";
const USAGE_LIMIT_CLIENT_MESSAGE: &str = "Claude usage limit reached";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(90);

#[tokio::test]
#[ignore = "opt-in real-client acceptance; binds only announced loopback ports 19876 and 19877"]
async fn real_claude_clients_route_through_an_isolated_host_and_fake_pool()
-> Result<(), Box<dyn std::error::Error>> {
    let root = IsolatedAcceptanceRoot::create()?;
    if let Err(error) = run_acceptance(&root).await {
        let safe_log_tail = root
            .sanitized_host_log_tail()
            .unwrap_or_else(|_| "isolated Host logs could not be read".to_owned());
        let preserved_root = root.preserve();
        return Err(format!(
            "{error}; isolated acceptance root retained at {}; sanitized Host logs follow:\n{safe_log_tail}",
            preserved_root.display()
        )
        .into());
    }
    Ok(())
}

async fn run_acceptance(root: &IsolatedAcceptanceRoot) -> Result<(), Box<dyn std::error::Error>> {
    let settings_before = root.settings_bytes()?;
    let upstream = FakeAnthropicServer::start(UPSTREAM_ADDRESS.parse()?, SYNTHETIC_ACCOUNT_ACCESS)?;
    root.seed_synthetic_claude_account().await?;

    let mut host = HostProcess::start(root).await?;
    let manifest = host.wait_until_ready(root).await?;
    let service_id = manifest
        .get("serviceId")
        .and_then(Value::as_str)
        .ok_or("Host manifest has no serviceId")?;
    let agent_sessions = workspace_binary("agent-sessions")?;
    let agent_collaboration = workspace_binary("agent-collaboration")?;
    require_real_client_on_path("claude")?;
    require_real_client_on_path("claude-agent-acp")?;

    let new_output = run_agent_sessions(
        &agent_sessions,
        root,
        [
            "--provider",
            "claude",
            "--new",
            "--",
            "--print",
            "--output-format",
            "json",
            NEW_PROMPT_MARKER,
        ],
        None,
    )
    .await?;
    if !new_output.status.success() {
        let observations = upstream.drain_requests();
        let client_failure = assert_client_success(&new_output, "real Claude Code new session")
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "real Claude Code new session unexpectedly succeeded".to_owned());
        let summary = observations
            .iter()
            .map(|request| {
                format!(
                    "path={} pooled_token={} prompt_marker={}",
                    request.path, request.has_synthetic_account_token, request.contains_new_prompt
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("{client_failure}; captured fake-upstream facts: {summary}").into());
    }
    assert_client_success(&new_output, "real Claude Code new session")?;
    assert_output_contains(
        &new_output,
        ACCEPTANCE_RESPONSE_MARKER,
        "new session response",
    )?;
    let new_requests = take_phase_requests(&upstream, |request| request.contains_new_prompt)?;
    let accept_encoding_values = new_requests
        .iter()
        .map(|request| {
            if request.accept_encoding.is_empty() {
                "<absent>".to_owned()
            } else {
                request.accept_encoding.join(" | ")
            }
        })
        .collect::<std::collections::BTreeSet<_>>();
    eprintln!(
        "Real Claude Code Accept-Encoding observed by the fake upstream: {accept_encoding_values:?}"
    );
    ensure(
        new_requests
            .iter()
            .all(|request| !request.received_usage_limit),
        "new Claude session received an unexpected usage-limit response",
    )?;

    let list_output = run_agent_sessions(
        &agent_sessions,
        root,
        ["--provider", "claude", "--list", "--format", "json"],
        None,
    )
    .await?;
    assert_client_success(&list_output, "agent-sessions stored-session listing")?;
    let session_id = first_session_id(&list_output.stdout)?;
    let resume_arguments = [
        "--provider",
        "claude",
        "--id",
        &session_id,
        "--",
        "--print",
        "--output-format",
        "json",
        RESUME_PROMPT_MARKER,
    ];
    let resume_output = if listed_session_has_working_directory(&list_output.stdout, &session_id)? {
        run_agent_sessions_from(&agent_sessions, root, &root.home, resume_arguments, None).await?
    } else {
        eprintln!(
            "Claude session listing omitted workingDirectory; resume acceptance keeps the invoking project directory"
        );
        run_agent_sessions(&agent_sessions, root, resume_arguments, None).await?
    };
    assert_client_success(&resume_output, "real Claude Code resume")?;
    assert_output_contains(
        &resume_output,
        ACCEPTANCE_RESPONSE_MARKER,
        "resume response",
    )?;
    let resume_requests = take_phase_requests(&upstream, |request| request.contains_resume_prompt)?;
    ensure(
        resume_requests
            .iter()
            .all(|request| !request.received_usage_limit),
        "resumed Claude session received an unexpected usage-limit response",
    )?;

    let requester = serde_json::json!({
        "endpoint": {"serviceId": service_id, "endpointId": "codex-local"},
        "sessionId": "00000000-0000-4000-8000-000000000021"
    })
    .to_string();
    let acp_output = run_agent_collaboration(
        &agent_collaboration,
        root,
        [
            "conversation",
            "prompt",
            "--endpoint",
            "claude-local",
            "--new",
            "--access",
            "write-restricted",
            "--cwd",
            root.workspace
                .to_str()
                .ok_or("workspace path is not UTF-8")?,
            "--from",
            requester.as_str(),
            "--text",
            ACP_PROMPT_MARKER,
            "--service-directory",
            root.service_directory
                .to_str()
                .ok_or("service directory is not UTF-8")?,
            "--timeout-seconds",
            "90",
            "--json",
        ],
    )
    .await?;
    assert_client_success(&acp_output, "real claude-agent-acp Host session")?;
    assert_output_contains(&acp_output, ACCEPTANCE_RESPONSE_MARKER, "Host ACP response")?;
    let acp_requests = take_phase_requests(&upstream, |request| request.contains_acp_prompt)?;
    ensure(
        acp_requests
            .iter()
            .all(|request| !request.received_usage_limit),
        "Host ACP session received an unexpected usage-limit response",
    )?;

    upstream.return_usage_limit();
    let exhausted_output = run_agent_sessions(
        &agent_sessions,
        root,
        [
            "--provider",
            "claude",
            "--new",
            "--",
            "--print",
            "--output-format",
            "json",
            USAGE_LIMIT_PROMPT_MARKER,
        ],
        None,
    )
    .await?;
    let exhausted_requests =
        take_phase_requests(&upstream, |request| request.contains_usage_limit_prompt)?;
    let exhausted_prompt_requests: Vec<_> = exhausted_requests
        .iter()
        .filter(|request| request.contains_usage_limit_prompt)
        .collect();
    ensure(
        exhausted_prompt_requests
            .iter()
            .all(|request| request.received_usage_limit),
        "the all-exhausted prompt request must receive the fake provider usage limit",
    )?;
    ensure(
        !exhausted_output.status.success(),
        "real Claude Code should surface the provider usage-limit response",
    )?;
    assert_output_contains(
        &exhausted_output,
        USAGE_LIMIT_CLIENT_MESSAGE,
        "usage-limit display",
    )?;
    assert_output_contains(&exhausted_output, "seconds", "usage-limit reset hint")?;
    ensure(
        exhausted_prompt_requests
            .iter()
            .all(|request| !request.contains_acp_prompt),
        "usage-limit request was unexpectedly routed through the ACP client",
    )?;

    for output in [
        &new_output,
        &list_output,
        &resume_output,
        &acp_output,
        &exhausted_output,
    ] {
        assert_output_does_not_contain(output, SYNTHETIC_ACCOUNT_ACCESS)?;
        assert_output_does_not_contain(output, SYNTHETIC_ACCOUNT_REFRESH)?;
    }
    let settings_after = root.settings_bytes()?;
    ensure(
        settings_after == settings_before,
        "isolated plain Claude settings changed",
    )?;
    ensure(
        !root.home.join(".claude/.credentials.json").exists(),
        "routed clients created a native Claude credential file in the isolated home",
    )?;

    let stopped_status = host.stop().await?;
    ensure(
        stopped_status.success(),
        "isolated Host did not stop cleanly",
    )?;

    let sentinel_directory = root.root.join("sentinel-bin");
    std::fs::create_dir_all(&sentinel_directory)?;
    let marker_path = root.root.join("claude-started-after-router-stop");
    install_claude_sentinel(&sentinel_directory, &marker_path)?;
    let stopped_output = run_agent_sessions(
        &agent_sessions,
        root,
        [
            "--provider",
            "claude",
            "--new",
            "--",
            "--print",
            "--output-format",
            "json",
            "STOPPED-ROUTER-PROMPT",
        ],
        Some(sentinel_directory.as_path()),
    )
    .await?;
    ensure(
        !stopped_output.status.success(),
        "agent-sessions should fail when the isolated Router is stopped",
    )?;
    assert_output_contains(
        &stopped_output,
        "Router proxy endpoint not published; restart the Router Host",
        "stopped Router preflight",
    )?;
    ensure(
        !marker_path.exists(),
        "Claude launched after Router preflight failed",
    )?;

    let host_logs = format!(
        "{}{}",
        std::fs::read_to_string(&root.host_stdout)?,
        std::fs::read_to_string(&root.host_stderr)?
    );
    ensure(
        !host_logs.contains(SYNTHETIC_ACCOUNT_ACCESS)
            && !host_logs.contains(SYNTHETIC_ACCOUNT_REFRESH),
        "Host logs contain a synthetic credential canary",
    )?;
    Ok(())
}

fn workspace_binary(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let router_binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-router"));
    let target_directory = router_binary
        .parent()
        .ok_or("Router binary has no target directory")?;
    let binary = target_directory.join(name);
    if !binary.is_file() {
        return Err(format!("missing built acceptance CLI {}", binary.display()).into());
    }
    Ok(binary)
}

fn require_real_client_on_path(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var_os("PATH").ok_or("PATH is unavailable")?;
    let found = std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .any(|candidate| candidate.is_file());
    if found {
        Ok(())
    } else {
        Err(format!("required real client {name} was not found on PATH").into())
    }
}

async fn run_agent_sessions<const N: usize>(
    binary: &Path,
    root: &IsolatedAcceptanceRoot,
    arguments: [&str; N],
    path_override: Option<&Path>,
) -> Result<Output, Box<dyn std::error::Error>> {
    run_agent_sessions_from(binary, root, &root.workspace, arguments, path_override).await
}

async fn run_agent_sessions_from<const N: usize>(
    binary: &Path,
    root: &IsolatedAcceptanceRoot,
    working_directory: &Path,
    arguments: [&str; N],
    path_override: Option<&Path>,
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = Command::new(binary);
    command
        .args(arguments)
        .current_dir(working_directory)
        .envs(root.isolated_client_environment())
        .env(
            "PATH",
            path_override.map_or_else(
                || std::env::var_os("PATH").unwrap_or_default(),
                |path| path.as_os_str().to_owned(),
            ),
        )
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_CUSTOM_HEADERS")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("DISABLE_ERROR_REPORTING", "1");
    tokio::time::timeout(CLIENT_TIMEOUT, command.output())
        .await
        .map_err(|_| "agent-sessions real-client command timed out")?
        .map_err(Into::into)
}

async fn run_agent_collaboration<const N: usize>(
    binary: &Path,
    root: &IsolatedAcceptanceRoot,
    arguments: [&str; N],
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = Command::new(binary);
    command
        .args(arguments)
        .current_dir(&root.workspace)
        .envs(root.isolated_client_environment())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("DISABLE_ERROR_REPORTING", "1");
    tokio::time::timeout(CLIENT_TIMEOUT, command.output())
        .await
        .map_err(|_| "agent-collaboration real ACP command timed out")?
        .map_err(Into::into)
}

fn first_session_id(output: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    let records: Value = serde_json::from_slice(output)?;
    records
        .as_array()
        .and_then(|records| records.first())
        .and_then(|record| record.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "agent-sessions did not list the stored Claude session".into())
}

fn listed_session_has_working_directory(
    output: &[u8],
    session_id: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let records: Value = serde_json::from_slice(output)?;
    Ok(records
        .as_array()
        .into_iter()
        .flatten()
        .filter(|record| record.get("sessionId").and_then(Value::as_str) == Some(session_id))
        .filter_map(|record| record.get("workingDirectory").and_then(Value::as_str))
        .any(|working_directory| Path::new(working_directory).is_absolute()))
}

fn take_phase_requests(
    server: &FakeAnthropicServer,
    phase_marker: impl Fn(&ObservedRequest) -> bool,
) -> Result<Vec<ObservedRequest>, Box<dyn std::error::Error>> {
    let first = server
        .next_request(Duration::from_secs(20))
        .map_err(|_| "fake Anthropic upstream did not receive a request")?;
    let mut requests = vec![first];
    requests.extend(server.drain_requests());
    let unsupported_paths: Vec<_> = requests
        .iter()
        .filter(|request| !is_messages_request_target(&request.path))
        .map(|request| request.path.as_str())
        .collect();
    if !unsupported_paths.is_empty() {
        return Err(format!(
            "real Claude client requested unsupported upstream paths: {}",
            unsupported_paths.join(", ")
        )
        .into());
    }
    if !requests
        .iter()
        .all(|request| request.has_synthetic_account_token)
    {
        return Err("Router did not send the synthetic pooled credential upstream".into());
    }
    if !requests.iter().any(phase_marker) {
        return Err("fake upstream request did not contain the current synthetic prompt".into());
    }
    Ok(requests)
}

fn assert_client_success(output: &Output, action: &str) -> Result<(), Box<dyn std::error::Error>> {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{action} exited with status {}; sanitized client output follows:\n{}",
            output.status,
            sanitized_client_output_excerpt(output)
        )
        .into())
    }
}

fn sanitized_client_output_excerpt(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace(SYNTHETIC_ACCOUNT_ACCESS, "[REDACTED]")
        .replace(SYNTHETIC_ACCOUNT_REFRESH, "[REDACTED]");
    let stderr = String::from_utf8_lossy(&output.stderr)
        .replace(SYNTHETIC_ACCOUNT_ACCESS, "[REDACTED]")
        .replace(SYNTHETIC_ACCOUNT_REFRESH, "[REDACTED]");
    stdout
        .lines()
        .chain(stderr.lines())
        .filter(|line| {
            let lowercase = line.to_ascii_lowercase();
            !lowercase.contains("authorization") && !lowercase.contains("api_key")
        })
        .take(8)
        .collect::<Vec<_>>()
        .join("\n")
}

fn ensure(condition: bool, failure: impl Into<String>) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(failure.into().into())
    }
}

fn assert_output_contains(
    output: &Output,
    marker: &str,
    observation: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains(marker) || stderr.contains(marker) {
        Ok(())
    } else {
        Err(format!(
            "{observation} did not contain the expected marker; sanitized client output follows:\n{}",
            sanitized_client_output_excerpt(output)
        )
        .into())
    }
}

fn assert_output_does_not_contain(
    output: &Output,
    secret_canary: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.contains(secret_canary) && !stderr.contains(secret_canary) {
        Ok(())
    } else {
        Err("a routed client output contains a synthetic credential canary".into())
    }
}

fn install_claude_sentinel(
    directory: &Path,
    marker_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let sentinel = directory.join("claude");
    let script = format!("#!/bin/sh\n: > '{}'\nexit 88\n", marker_path.display());
    std::fs::write(&sentinel, script)?;
    std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}
