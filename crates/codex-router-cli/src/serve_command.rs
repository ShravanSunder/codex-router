//! Serve command runtime composition and worker lifetime ownership.

use super::*;
use codex_router_auth::resolver::CredentialRefreshTaskSupervisor;
use codex_router_proxy::server::{LoopbackRouterRuntimeError, ServerBindError};

pub(crate) async fn run_serve_command(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
) -> Result<(), CliError> {
    #[cfg(test)]
    let upkeep_start = |path, credentials, supervisor| {
        credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock(
            path,
            credentials,
            supervisor,
            codex_router_auth::resolver::NoopCredentialRefreshClient,
            || codex_router_auth::resolver::current_unix_seconds().unwrap_or(0),
        )
    };
    #[cfg(not(test))]
    let upkeep_start = credential_upkeep_worker::start_background_credential_upkeep_worker;
    run_serve_composition(
        stdout,
        command,
        None,
        upkeep_start,
        quota::start_background_quota_refresh_worker,
        |_| {},
    )
    .await
}

#[cfg(test)]
pub(crate) async fn run_serve_command_with_upkeep_start<UpkeepStart, UpkeepFuture>(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
    _credential_store: EncryptedCredentialStore,
    upkeep_start: UpkeepStart,
) -> Result<(), CliError>
where
    UpkeepStart:
        FnOnce(PathBuf, EncryptedCredentialStore, CredentialRefreshTaskSupervisor) -> UpkeepFuture,
    UpkeepFuture: Future<
        Output = Result<
            credential_upkeep_worker::CredentialUpkeepWorker,
            credential_upkeep_worker::CredentialUpkeepStartError,
        >,
    >,
{
    run_serve_command_with_upkeep_start_and_token_reload_observer(
        stdout,
        command,
        _credential_store,
        upkeep_start,
        |_generation| {},
    )
    .await
}

#[cfg(test)]
pub(crate) async fn run_serve_command_with_upkeep_start_and_token_reload_observer<
    UpkeepStart,
    UpkeepFuture,
>(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
    _credential_store: EncryptedCredentialStore,
    upkeep_start: UpkeepStart,
    token_reload_observer: impl Fn(codex_router_core::ids::TokenGeneration) + Send + 'static,
) -> Result<(), CliError>
where
    UpkeepStart:
        FnOnce(PathBuf, EncryptedCredentialStore, CredentialRefreshTaskSupervisor) -> UpkeepFuture,
    UpkeepFuture: Future<
        Output = Result<
            credential_upkeep_worker::CredentialUpkeepWorker,
            credential_upkeep_worker::CredentialUpkeepStartError,
        >,
    >,
{
    run_serve_command_with_worker_starts_and_token_reload_observer(
        stdout,
        command,
        _credential_store,
        upkeep_start,
        quota::start_background_quota_refresh_worker,
        token_reload_observer,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn run_serve_command_with_worker_starts_and_token_reload_observer<
    UpkeepStart,
    UpkeepFuture,
    QuotaStart,
    QuotaFuture,
>(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
    _credential_store: EncryptedCredentialStore,
    upkeep_start: UpkeepStart,
    quota_start: QuotaStart,
    token_reload_observer: impl Fn(codex_router_core::ids::TokenGeneration) + Send + 'static,
) -> Result<(), CliError>
where
    UpkeepStart:
        FnOnce(PathBuf, EncryptedCredentialStore, CredentialRefreshTaskSupervisor) -> UpkeepFuture,
    UpkeepFuture: Future<
        Output = Result<
            credential_upkeep_worker::CredentialUpkeepWorker,
            credential_upkeep_worker::CredentialUpkeepStartError,
        >,
    >,
    QuotaStart: FnOnce(
        PathBuf,
        PathBuf,
        secret_store_factory::CliRuntimeSecretStore,
        String,
        Duration,
        codex_router_proxy::websocket::WebSocketQuotaFloorNotifier,
        CredentialRefreshTaskSupervisor,
    ) -> QuotaFuture,
    QuotaFuture:
        Future<Output = Result<quota::BackgroundQuotaRefreshWorker, quota::QuotaRefreshError>>,
{
    run_serve_composition(
        stdout,
        command,
        Some(_credential_store),
        upkeep_start,
        quota_start,
        token_reload_observer,
    )
    .await
}

pub(crate) async fn run_serve_composition<UpkeepStart, UpkeepFuture, QuotaStart, QuotaFuture>(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
    fixture_credentials: Option<EncryptedCredentialStore>,
    upkeep_start: UpkeepStart,
    quota_start: QuotaStart,
    token_reload_observer: impl Fn(codex_router_core::ids::TokenGeneration) + Send + 'static,
) -> Result<(), CliError>
where
    UpkeepStart:
        FnOnce(PathBuf, EncryptedCredentialStore, CredentialRefreshTaskSupervisor) -> UpkeepFuture,
    UpkeepFuture: Future<
        Output = Result<
            credential_upkeep_worker::CredentialUpkeepWorker,
            credential_upkeep_worker::CredentialUpkeepStartError,
        >,
    >,
    QuotaStart: FnOnce(
        PathBuf,
        PathBuf,
        secret_store_factory::CliRuntimeSecretStore,
        String,
        Duration,
        codex_router_proxy::websocket::WebSocketQuotaFloorNotifier,
        CredentialRefreshTaskSupervisor,
    ) -> QuotaFuture,
    QuotaFuture:
        Future<Output = Result<quota::BackgroundQuotaRefreshWorker, quota::QuotaRefreshError>>,
{
    let mut runtime_config = base_serve_runtime_config(&command)?;
    if let Some(audit_file) = command.audit_file.clone() {
        runtime_config = runtime_config.with_audit_file(audit_file);
    }
    if let Some(report_file) = command.websocket_registry_report_file.clone() {
        validate_websocket_registry_report_file(&report_file)?;
        runtime_config = runtime_config.with_websocket_registry_report_file(report_file);
    }
    if let Some(now) = command.now_unix_seconds {
        runtime_config = runtime_config.with_quota_clock(now, command.max_snapshot_age_seconds);
    }
    let gate = codex_router_descriptor_boundary::DescriptorGate::global();
    let address = runtime_config.bind_address().socket_addr();
    let (listener, actual_address) = {
        let _creation = gate.creation().await;
        let listener = std::net::TcpListener::bind(address).map_err(|source| {
            CliError::Runtime(LoopbackRouterRuntimeError::Bind(ServerBindError::Bind {
                address,
                source,
            }))
        })?;
        let actual_address = listener.local_addr().map_err(|source| {
            CliError::Runtime(LoopbackRouterRuntimeError::Bind(ServerBindError::Bind {
                address,
                source,
            }))
        })?;
        (listener, actual_address)
    };
    let listener = codex_router_descriptor_boundary::OwnedListener::from_tcp_owned(
        std::os::fd::OwnedFd::from(listener),
        actual_address,
        gate,
    )
    .await
    .map_err(|error| CliError::Runtime(LoopbackRouterRuntimeError::ListenerGrant(error)))?;
    let config = build_serve_role_config(runtime_config, &command);
    #[cfg(test)]
    let prepared = match fixture_credentials {
        Some(credentials) => {
            agent_proxy_services::test_support::prepare_fresh_with_fixture_credentials(
                config,
                credentials,
                listener,
                gate,
            )
            .await?
        }
        None => {
            agent_proxy_services::ProxyRoleRuntime::prepare(
                config,
                codex_router_keeper_protocol::PrepareMode::Fresh,
                listener,
                gate,
            )
            .await?
        }
    };
    #[cfg(not(test))]
    let prepared = {
        let _fixture_credentials = fixture_credentials;
        agent_proxy_services::ProxyRoleRuntime::prepare(
            config,
            codex_router_keeper_protocol::PrepareMode::Fresh,
            listener,
            gate,
        )
        .await?
    };
    let mut runtime = prepared
        .activate_with_worker_starts(upkeep_start, quota_start, token_reload_observer, |core| {
            crate::presentation::host::render_progress_event(
                stdout,
                codex_router_host::HostProgress::RouterReady,
            )?;
            writeln!(stdout, "listening: {}", core.local_addr())
        })
        .await?;
    let serve_result = match runtime.wait_serving().await {
        Ok(handled) => match command.websocket_registry_report_file {
            Some(path) => write_websocket_registry_report_file(&path, handled, runtime.core()),
            None => Ok(()),
        },
        Err(error) => Err(CliError::from(error)),
    };
    let cleanup_result = runtime.shutdown().await.map_err(CliError::from);
    match serve_result {
        Err(error) => Err(error),
        Ok(()) => cleanup_result,
    }
}

pub(crate) fn base_serve_runtime_config(
    command: &cli_argument_parsing::ServeCommand,
) -> Result<LoopbackRouterRuntimeConfig, CliError> {
    let bind_address = LoopbackBindAddress::new(&command.listen_host, command.port)?;
    let upstream_endpoint = UpstreamEndpoint::new(command.upstream_base_url.clone())?;
    let runtime_config = LoopbackRouterRuntimeConfig::new_tokenless(
        bind_address,
        upstream_endpoint,
        command.state_db.clone(),
        command.secret_root.clone(),
    )
    .with_session_pin_idle_ttl(Duration::from_secs(command.session_pin_idle_ttl_seconds))
    .with_claude_five_hour_reserve_percent(command.claude_five_hour_reserve_percent);
    #[cfg(debug_assertions)]
    let runtime_config = if let Some(base_url) = &command.debug_claude_upstream_base_url {
        let endpoint = ClaudeUpstreamEndpoint::isolated_debug_override(
            base_url.clone(),
            command.require_debug_isolation,
        )?;
        runtime_config.with_debug_claude_upstream_endpoint(endpoint)
    } else {
        runtime_config
    };
    Ok(runtime_config)
}

/// Projects parsed Serve options into the role configuration used by production preparation.
pub(crate) fn build_serve_role_config(
    runtime_config: LoopbackRouterRuntimeConfig,
    command: &cli_argument_parsing::ServeCommand,
) -> agent_proxy_services::ProxyRoleConfig {
    agent_proxy_services::ProxyRoleConfig {
        core: runtime_config,
        local_token: if command.require_local_token {
            agent_proxy_services::ProxyLocalTokenPolicy::Required
        } else {
            agent_proxy_services::ProxyLocalTokenPolicy::Optional
        },
        quota_refresh: if command.background_quota_refresh_enabled {
            agent_proxy_services::ProxyQuotaRefreshPolicy::Enabled
        } else {
            agent_proxy_services::ProxyQuotaRefreshPolicy::Disabled
        },
        quota_refresh_interval: Duration::from_secs(command.quota_refresh_interval_seconds),
        max_connections: command.max_connections,
    }
}
