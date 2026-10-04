#[cfg(test)]
fn test_credential_store_for_config(
    config: &LoopbackRouterRuntimeConfig,
) -> Result<EncryptedCredentialStore, LoopbackRouterRuntimeError> {
    codex_router_secret_store::test_support::open_encrypted_credential_store(
        &config.secret_store_root,
    )
    .map_err(|error| {
        LoopbackRouterRuntimeError::CredentialResources(
            ProxyRuntimeCredentialResourcesOpenError::SecretStore(error),
        )
    })
}

fn active_session_event_compaction_before(now_unix_seconds: u64) -> u64 {
    now_unix_seconds.saturating_sub(ACTIVE_SESSION_EVENT_RETENTION_SECONDS)
}

fn claim_session_affinity_cleanup_day(
    last_attempted_utc_day: &AtomicU64,
    now_unix_seconds: u64,
) -> bool {
    const SECONDS_PER_DAY: u64 = 86_400;
    const NO_CLEANUP_ATTEMPT_UTC_DAY: u64 = u64::MAX;
    let current_utc_day = now_unix_seconds / SECONDS_PER_DAY;
    last_attempted_utc_day
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |last_attempted_day| {
            (last_attempted_day == NO_CLEANUP_ATTEMPT_UTC_DAY
                || current_utc_day > last_attempted_day)
                .then_some(current_utc_day)
        })
        .is_ok()
}

fn handle_connection_join_result(
    joined: Result<Result<(), LoopbackRouterRuntimeError>, JoinError>,
) -> Result<(), LoopbackRouterRuntimeError> {
    match joined {
        Ok(Ok(())) => Ok(()),
        Ok(Err(LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::Transport(ref error),
        ))) if crate::websocket::is_normal_websocket_cleanup_close(error) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(source) => Err(LoopbackRouterRuntimeError::ConnectionJoin(source)),
    }
}

fn store_connection_join_error(
    first_connection_error: &mut Option<LoopbackRouterRuntimeError>,
    joined: Result<Result<(), LoopbackRouterRuntimeError>, JoinError>,
) -> bool {
    match handle_connection_join_result(joined) {
        Ok(()) => false,
        Err(error) => {
            if first_connection_error.is_none() {
                *first_connection_error = Some(error);
            }
            true
        }
    }
}

fn store_optional_connection_join_error(
    first_connection_error: &mut Option<LoopbackRouterRuntimeError>,
    joined: Option<Result<Result<(), LoopbackRouterRuntimeError>, JoinError>>,
) -> bool {
    match joined {
        Some(joined) => store_connection_join_error(first_connection_error, joined),
        None => false,
    }
}

fn supervise_detached_connection_handler(
    handler: UpgradeTaskHandle,
    reporter: Arc<dyn LoopbackConnectionErrorReporter>,
) {
    tokio::spawn(async move {
        match handler.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                reporter.report_connection_error(&loopback_connection_diagnostic(&error).render());
            }
            Err(_source) => reporter.report_connection_error(
                &LoopbackConnectionDiagnostic::new("join_failure", "task_join", "error").render(),
            ),
        }
    });
}
