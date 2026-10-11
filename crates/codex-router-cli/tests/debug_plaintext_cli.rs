//! Compiled process proof for explicit root-local plaintext storage; no OAuth or real provider.
use std::path::PathBuf;
#[cfg(debug_assertions)]
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

type ProofResult<TValue> = Result<TValue, Box<dyn std::error::Error>>;

fn check(condition: bool, message: &str) -> ProofResult<()> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned().into())
    }
}

fn candidate_binary() -> PathBuf {
    std::env::var_os("CODEX_ROUTER_PLAINTEXT_CANDIDATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_codex-router")))
}

fn candidate_command() -> Command {
    let mut command = Command::new(candidate_binary());
    command
        .env("OTEL_SDK_DISABLED", "true")
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")
        .kill_on_drop(true);
    command
}

#[cfg(debug_assertions)]
async fn checked_output(mut command: Command) -> ProofResult<Vec<u8>> {
    let output = timeout(Duration::from_secs(15), command.output()).await??;
    if !output.status.success() {
        return Err(format!(
            "candidate exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn debug_plaintext_compiled_cli_activates_and_selects_current_token_across_reopen()
-> ProofResult<()> {
    use codex_router_auth::credential_activation::{
        CredentialActivation, CredentialActivationRequest,
    };
    use codex_router_core::ids::AccountId;
    use codex_router_core::provider::Provider;
    use codex_router_secret_store::account_tokens::{
        AccountCredentialBundle, openai_account_credential_bundle_key,
    };
    use codex_router_secret_store::credential_bundle::CredentialBundle;
    use codex_router_secret_store::file_backend::FileSecretStore;
    use codex_router_secret_store::local_router_token::LocalRouterTokenService;
    use codex_router_secret_store::runtime_credential_store::RuntimeCredentialStore;
    use codex_router_state::quota_snapshot::{
        PersistedQuotaSnapshot, PersistedSelectorQuotaWindow, QuotaSnapshotSource,
        SelectorQuotaWindowStatus,
    };
    use codex_router_state::repositories::{QuotaSnapshotRepository, SelectorQuotaRepository};
    use codex_router_state::sqlite::{AsyncSqliteStateStore, SqliteStateStore};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let directory = tempfile::tempdir()?;
    let router_root = directory.path().join("router");
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    let first_port = reserve_loopback_port()?;
    let mut initialize = candidate_command();
    initialize
        .arg("serve")
        .args(["--port", &first_port.to_string()])
        .arg("--state-db")
        .arg(&state_path)
        .arg("--secret-root")
        .arg(&secret_root)
        .args([
            "--allow-plaintext-file-secrets",
            "--require-debug-isolation",
            "--disable-background-quota-refresh",
            "--max-connections",
            "0",
        ]);
    checked_output(initialize).await?;
    check(
        (std::fs::read(secret_root.join("debug-plaintext.marker"))?) == (b"debug-plaintext-v1\n"),
        "explicit initializer must publish the exact plaintext declaration",
    )?;
    check(
        std::net::TcpListener::bind(("127.0.0.1", first_port)).is_ok(),
        "initialization process must release its listener",
    )?;
    let store = RuntimeCredentialStore::DebugPlaintext(
        FileSecretStore::open_declared_debug_plaintext(&secret_root)?
            .ok_or("root has no declaration")?,
    );
    let state = AsyncSqliteStateStore::open(&state_path).await?;
    let account_id = AccountId::new("compiled_plaintext_fixture")?;
    for (token, expected_generation) in
        [("previous-access-canary", 1), ("current-access-canary", 2)]
    {
        let bundle = CredentialBundle::OpenAi(
            AccountCredentialBundle::imported_codex_auth(token, None)
                .with_expires_unix_seconds(4_000_000_000),
        );
        let generation = CredentialActivation::activate_login(
            &state,
            &store,
            CredentialActivationRequest::new(
                Provider::Openai,
                account_id.clone(),
                "plaintext fixture",
                bundle,
            ),
        )
        .await?;
        check(
            (generation) == (expected_generation),
            "credential activation must advance the active generation",
        )?;
    }
    drop(state);
    let current_key = openai_account_credential_bundle_key(&account_id, 2)?;
    check(
        std::fs::read_to_string(secret_root.join(format!("{}.secret", current_key.as_str())))?
            .contains("current-access-canary"),
        "active bundle must be an actual plaintext file",
    )?;
    check(
        !secret_root.join("store-id").exists(),
        "plaintext root must not create encrypted store identity",
    )?;
    check(
        !secret_root.join("format-v2.marker").exists(),
        "plaintext root must not create encrypted format marker",
    )?;
    let state = SqliteStateStore::open(&state_path)?;
    QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 88),
    )?;
    for (duration, effective) in [(18_000, true), (604_800, false)] {
        SelectorQuotaRepository::upsert_selector_window(
            &state,
            &PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                "responses",
                duration,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(88)
            .with_reset_unix_seconds(duration)
            .with_effective(effective)
            .with_observed_unix_seconds(1_000),
        )?;
    }
    drop(state);
    let mut list = candidate_command();
    list.args(["account", "list", "--router-root"])
        .arg(&router_root);
    check(
        String::from_utf8(checked_output(list).await?)?.contains("plaintext fixture"),
        "account list must reopen the declared plaintext root",
    )?;
    let mut quota = candidate_command();
    quota
        .args(["quota", "--router-root"])
        .arg(&router_root)
        .args(["--format", "json", "--now-unix-seconds", "1030"]);
    let report: serde_json::Value = serde_json::from_slice(&checked_output(quota).await?)?;
    check(
        (report["credential_store_status"]) == ("ready"),
        "quota must report the declared credential store ready",
    )?;
    check(
        (report["accounts"][0]["safe_account_label"]) == ("plaintext fixture"),
        "quota must retain the activated account label",
    )?;

    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let upstream_address = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = timeout(Duration::from_secs(15), upstream.accept()).await??;
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let count = timeout(Duration::from_secs(15), stream.read(&mut buffer)).await??;
            if count == 0 {
                return Err(std::io::Error::other("upstream closed before request"));
            }
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|length| length.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\ndata: ok\n\n",
            )
            .await?;
        Ok::<_, std::io::Error>(request)
    });
    let router_port = reserve_loopback_port()?;
    let mut serve = candidate_command();
    serve
        .arg("serve")
        .args(["--port", &router_port.to_string()])
        .arg("--state-db")
        .arg(&state_path)
        .arg("--secret-root")
        .arg(&secret_root)
        .args([
            "--upstream-base-url",
            &format!("http://{upstream_address}/v1"),
            "--now-unix-seconds",
            "1030",
            "--max-snapshot-age-seconds",
            "60",
            "--disable-background-quota-refresh",
            "--require-local-token",
            "--max-connections",
            "1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The second serve reads the declaration; it receives no plaintext opt-in flag.
    let mut child = serve.spawn()?;
    let mut output = BufReader::new(child.stdout.take().ok_or("child stdout missing")?);
    timeout(Duration::from_secs(15), async {
        loop {
            let mut line = String::new();
            if output.read_line(&mut line).await? == 0 {
                return Err(std::io::Error::other("candidate exited before listening"));
            }
            if line.contains("listening:") {
                break;
            }
        }
        Ok::<_, std::io::Error>(())
    })
    .await??;
    let local_token =
        LocalRouterTokenService::new(FileSecretStore::open(&secret_root)?).load_current()?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()?
        .post(format!("http://127.0.0.1:{router_port}/v1/responses"))
        .header("x-codex-router-token", local_token.token().expose_secret())
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"controlled plaintext proof"}"#)
        .send()
        .await?;
    check(
        (response.status()) == (reqwest::StatusCode::OK),
        "controlled provider response must return HTTP 200",
    )?;
    check(
        (response.text().await?) == ("data: ok\n\n"),
        "controlled provider response body must arrive unchanged",
    )?;
    let request = String::from_utf8(timeout(Duration::from_secs(15), upstream_task).await???)?;
    check(
        request.starts_with("POST /v1/responses HTTP/1.1\r\n"),
        "upstream must receive the Responses request",
    )?;
    check(
        request.contains("authorization: Bearer current-access-canary\r\n"),
        "upstream must receive the active generation access token",
    )?;
    check(
        !request.contains("previous-access-canary"),
        "upstream must never receive the previous generation token",
    )?;
    check(
        !request.contains(local_token.token().expose_secret()),
        "upstream must never receive the local router token",
    )?;
    let exit = timeout(Duration::from_secs(15), child.wait_with_output()).await??;
    check(
        exit.status.success(),
        "serve process must exit successfully and be reaped",
    )?;
    check(
        std::net::TcpListener::bind(("127.0.0.1", router_port)).is_ok(),
        "serve process must release its listener",
    )?;
    eprintln!(
        "candidate_sha256={:x}; initialization_exit=0; account_reopen=ok; quota_ready=ok; active_generation=2; tokened_http=200; serve_exit=0; child_reaped=true; listener_released=true",
        Sha256::digest(std::fs::read(candidate_binary())?)
    );
    Ok(())
}

#[cfg(debug_assertions)]
fn reserve_loopback_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

#[cfg(not(debug_assertions))]
#[tokio::test]
async fn release_compiled_cli_refuses_plaintext_flags_and_declared_roots() -> ProofResult<()> {
    let directory = tempfile::tempdir()?;
    for arguments in [
        vec!["serve", "--allow-plaintext-file-secrets"],
        vec![
            "account",
            "login",
            "--label",
            "fixture",
            "--allow-plaintext-file-secrets",
        ],
    ] {
        let output = timeout(
            Duration::from_secs(15),
            candidate_command().args(arguments).output(),
        )
        .await??;
        check(
            (output.status.code()) == (Some(2)),
            "release flag rejection must exit 2",
        )?;
        check(
            String::from_utf8_lossy(&output.stderr)
                .contains("unknown option: --allow-plaintext-file-secrets"),
            "release must reject the literal plaintext option",
        )?;
    }
    let secret_root = directory.path().join("secrets");
    std::fs::create_dir(&secret_root)?;
    std::fs::write(
        secret_root.join("debug-plaintext.marker"),
        b"debug-plaintext-v1\n",
    )?;
    let output = timeout(
        Duration::from_secs(15),
        candidate_command()
            .arg("serve")
            .arg("--state-db")
            .arg(directory.path().join("state.sqlite"))
            .arg("--secret-root")
            .arg(&secret_root)
            .args(["--max-connections", "0"])
            .output(),
    )
    .await??;
    check(
        (output.status.code()) == (Some(2)),
        "release declared-root rejection must exit 2",
    )?;
    check(
        String::from_utf8_lossy(&output.stderr).contains("credential store"),
        "release must report the unavailable credential root",
    )?;
    check(
        !directory.path().join("state.sqlite").exists(),
        "release rejection must precede SQLite creation",
    )?;
    check(
        !secret_root.join("store-id").exists(),
        "release rejection must precede encrypted store identity creation",
    )?;
    check(
        !secret_root.join("format-v2.marker").exists(),
        "release rejection must precede encrypted format marker creation",
    )?;
    Ok(())
}
