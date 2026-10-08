//! Serve command runtime composition and worker lifetime ownership.

use super::*;

pub(crate) async fn run_serve_command_with_upkeep_start<UpkeepStart, UpkeepFuture>(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
    credential_store: EncryptedCredentialStore,
    upkeep_start: UpkeepStart,
) -> Result<(), CliError>
where
    UpkeepStart: FnOnce(PathBuf, EncryptedCredentialStore) -> UpkeepFuture,
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
        credential_store,
        upkeep_start,
        |_generation| {},
    )
    .await
}

pub(crate) async fn run_serve_command_with_upkeep_start_and_token_reload_observer<
    UpkeepStart,
    UpkeepFuture,
>(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
    credential_store: EncryptedCredentialStore,
    upkeep_start: UpkeepStart,
    token_reload_observer: impl Fn(codex_router_core::ids::TokenGeneration) + Send + 'static,
) -> Result<(), CliError>
where
    UpkeepStart: FnOnce(PathBuf, EncryptedCredentialStore) -> UpkeepFuture,
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
        credential_store,
        upkeep_start,
        quota::start_background_quota_refresh_worker,
        token_reload_observer,
    )
    .await
}

pub(crate) async fn run_serve_command_with_worker_starts_and_token_reload_observer<
    UpkeepStart,
    UpkeepFuture,
    QuotaStart,
    QuotaFuture,
>(
    stdout: &mut impl Write,
    command: cli_argument_parsing::ServeCommand,
    credential_store: EncryptedCredentialStore,
    upkeep_start: UpkeepStart,
    quota_start: QuotaStart,
    token_reload_observer: impl Fn(codex_router_core::ids::TokenGeneration) + Send + 'static,
) -> Result<(), CliError>
where
    UpkeepStart: FnOnce(PathBuf, EncryptedCredentialStore) -> UpkeepFuture,
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
    ) -> QuotaFuture,
    QuotaFuture: Future<Output = Result<quota::BackgroundQuotaRefreshWorker, QuotaCommandError>>,
{
    let mut runtime_config = base_serve_runtime_config(&command)?;
    let state_db = command.state_db.clone();
    let secret_root = command.secret_root.clone();
    if let Some(audit_file) = command.audit_file.clone() {
        runtime_config = runtime_config.with_audit_file(audit_file);
    }
    if let Some(report_file) = command.websocket_registry_report_file.clone() {
        validate_websocket_registry_report_file(&report_file)?;
        runtime_config = runtime_config.with_websocket_registry_report_file(report_file);
    }
    let local_token_store =
        FileSecretStore::open(&secret_root).map_err(TokenCommandError::SecretStore)?;
    let token_service = LocalRouterTokenService::new(local_token_store.clone());
    let local_token = token_service.ensure_local_token(&secret_root)?;
    let initial_token_generation = local_token.generation();
    let (mut runtime_config, quota_refresh_interval) =
        configure_serve_claude_edge_runtime(runtime_config, &command, local_token.clone());
    if command.require_local_token {
        runtime_config = runtime_config.with_required_local_token(local_token);
    }
    if let Some(now_unix_seconds) = command.now_unix_seconds {
        runtime_config =
            runtime_config.with_quota_clock(now_unix_seconds, command.max_snapshot_age_seconds);
    }
    let runtime = LoopbackRouterRuntime::start(runtime_config, credential_store.clone()).await?;
    let local_auth_reloader = runtime.local_auth_reloader();
    let mut token_reload_watcher =
        LocalTokenReloadWatcher::start(local_token_store, initial_token_generation, move |auth| {
            let current_generation = auth.current_generation();
            local_auth_reloader.reload_auth(auth);
            token_reload_observer(current_generation);
        });

    if let Err(error) = crate::presentation::host::render_progress_event(
        stdout,
        codex_router_host::HostProgress::RouterReady,
    ) {
        token_reload_watcher.shutdown().await;
        runtime.shutdown().await;
        return Err(CliError::Stdout(error));
    }
    let listening_output_result = writeln!(stdout, "listening: {}", runtime.local_addr());
    if let Err(error) = listening_output_result {
        token_reload_watcher.shutdown().await;
        runtime.shutdown().await;
        return Err(CliError::Stdout(error));
    }
    let mut upkeep_worker = match upkeep_start(state_db.clone(), credential_store.clone()).await {
        Ok(worker) => worker,
        Err(error) => {
            token_reload_watcher.shutdown().await;
            runtime.shutdown().await;
            return Err(CliError::from(error));
        }
    };
    let quota_refresh_worker = if command.background_quota_refresh_enabled {
        let quota_floor_notifier = runtime.websocket_quota_floor_notifier();
        match quota_start(
            state_db,
            secret_root,
            credential_store,
            DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
            quota_refresh_interval,
            quota_floor_notifier,
        )
        .await
        {
            Ok(worker) => Some(worker),
            Err(error) => {
                upkeep_worker.shutdown().await;
                token_reload_watcher.shutdown().await;
                runtime.shutdown().await;
                return Err(CliError::from(error));
            }
        }
    } else {
        None
    };
    let serve_result = match runtime
        .serve_protocol_connections(command.max_connections)
        .await
    {
        Ok(handled_connections) => match command.websocket_registry_report_file {
            Some(report_file) => {
                write_websocket_registry_report_file(&report_file, handled_connections, &runtime)
            }
            None => Ok(()),
        },
        Err(error) => Err(CliError::from(error)),
    };
    if let Some(mut worker) = quota_refresh_worker {
        worker.shutdown().await;
    }
    upkeep_worker.shutdown().await;
    token_reload_watcher.shutdown().await;
    runtime.shutdown().await;
    serve_result
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

pub(crate) fn configure_serve_claude_edge_runtime(
    runtime_config: LoopbackRouterRuntimeConfig,
    command: &cli_argument_parsing::ServeCommand,
    local_token: LocalRouterTokenRecord,
) -> (LoopbackRouterRuntimeConfig, Duration) {
    let quota_refresh_interval = Duration::from_secs(command.quota_refresh_interval_seconds);
    (
        runtime_config.with_claude_edge_local_token(local_token, quota_refresh_interval),
        quota_refresh_interval,
    )
}
