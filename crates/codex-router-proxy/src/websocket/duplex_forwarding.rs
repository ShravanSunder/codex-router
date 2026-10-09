use super::provider_signals::{
    CODEX_WEBSOCKET_RECONNECT_SIGNAL, maybe_replace_account_quota_exhaustion_with_reconnect_signal,
};
use super::response_metadata::{
    is_response_completed_text, is_response_create, is_response_failed_text,
    is_response_incomplete_text, is_response_terminal_error_text, provider_error_body_from_message,
    provider_error_classification_from_message, record_forwarded_websocket_metadata,
    websocket_metadata_text_handle,
};
use super::session_registry::WebSocketSessionRegistration;
use super::transport_cleanup::{
    close_websocket_sink_best_effort, is_reset_without_closing_handshake,
};
use super::*;

pub(super) struct WebSocketForwardingContext<'a> {
    pub(super) session_registration: WebSocketSessionRegistration,
    pub(super) affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
    pub(super) async_affinity_owner_recorder: Option<Arc<dyn AsyncHttpAffinityOwnerRecorder>>,
    pub(super) affinity_record_tasks: TaskTracker,
    pub(super) affinity_owner_context: Option<&'a WebSocketAffinityOwnerContext>,
    pub(super) provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    pub(super) account_admission_assessor: Option<Arc<dyn LiveAccountAdmissionAssessor>>,
    pub(super) initial_turn_active: bool,
    pub(super) revocation: &'a CancellationToken,
    pub(super) session_shutdown: &'a CancellationToken,
}

pub(super) async fn forward_duplex_until_complete<LocalStream, UpstreamStream>(
    local_websocket: WebSocketStream<LocalStream>,
    upstream_websocket: WebSocketStream<UpstreamStream>,
    context: WebSocketForwardingContext<'_>,
) -> Result<(), WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    UpstreamStream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (local_write, local_read) = local_websocket.split();
    let (upstream_write, upstream_read) = upstream_websocket.split();
    let local_to_upstream_revocation = context.revocation.clone();
    let upstream_to_local_revocation = context.revocation.clone();
    let local_to_upstream_shutdown = context.session_shutdown.clone();
    let upstream_to_local_shutdown = context.session_shutdown.clone();
    let tunnel_shutdown = CancellationToken::new();
    let local_to_upstream_tunnel_shutdown = tunnel_shutdown.clone();
    let upstream_to_local_tunnel_shutdown = tunnel_shutdown.clone();
    let session_registration = context.session_registration;
    let session_registry = session_registration.registry.clone();
    let session_id = session_registration.session_id;
    let quota_floor_reconnect = session_registration.quota_floor_reconnect.clone();
    let early_floor_reconnect = session_registration.early_floor_reconnect.clone();
    let graceful_floor_switch = session_registration.graceful_floor_switch.clone();
    let affinity_owner_context = context.affinity_owner_context.cloned();
    let account_turn_admission = AccountTurnAdmission::new(
        graceful_floor_switch.clone(),
        early_floor_reconnect.clone(),
        quota_floor_reconnect.clone(),
        affinity_owner_context
            .as_ref()
            .map(|context| context.account_id.clone()),
        affinity_owner_context
            .as_ref()
            .map(|context| context.credential_generation),
        context.account_admission_assessor,
        context.initial_turn_active,
    )
    .with_credit_backed_admission_seen(
        affinity_owner_context
            .as_ref()
            .is_some_and(|context| context.credit_backed_at_selection),
    );
    let active_turn_reservation = ActiveTurnReservationState::new(
        affinity_owner_context
            .as_ref()
            .and_then(|context| context.active_reservation_guard.clone()),
    );
    let local_active_turn_reservation = active_turn_reservation.clone();
    let local_account_turn_admission = account_turn_admission.clone();
    let local_early_floor_reconnect = early_floor_reconnect.clone();
    let local_quota_floor_reconnect = quota_floor_reconnect.clone();
    let session_affinity_activity_handle = affinity_owner_context
        .as_ref()
        .and_then(|context| context.session_affinity_activity_handle.clone());
    let local_to_upstream = tokio::spawn(async move {
        pump_local_to_upstream(
            local_read,
            upstream_write,
            LocalToUpstreamPumpContext {
                revocation: local_to_upstream_revocation,
                session_shutdown: local_to_upstream_shutdown,
                tunnel_shutdown: local_to_upstream_tunnel_shutdown,
                active_turn_reservation: local_active_turn_reservation,
                session_affinity_activity_handle,
                account_turn_admission: local_account_turn_admission,
                early_floor_reconnect: local_early_floor_reconnect,
                quota_floor_reconnect: local_quota_floor_reconnect,
            },
        )
        .await
    });
    let upstream_to_local = tokio::spawn(async move {
        pump_upstream_to_local(
            upstream_read,
            local_write,
            UpstreamToLocalPumpContext {
                revocation: upstream_to_local_revocation,
                session_shutdown: upstream_to_local_shutdown,
                tunnel_shutdown: upstream_to_local_tunnel_shutdown,
                session_registry,
                session_id,
                affinity_owner_recorder: context.affinity_owner_recorder,
                async_affinity_owner_recorder: context.async_affinity_owner_recorder,
                affinity_record_tasks: context.affinity_record_tasks,
                affinity_owner_context,
                active_turn_reservation,
                provider_error_observer: context.provider_error_observer,
                quota_floor_reconnect,
                early_floor_reconnect,
                graceful_floor_switch,
                account_turn_admission,
            },
        )
        .await
    });

    let result = supervise_websocket_pumps(
        context.revocation,
        context.session_shutdown,
        &tunnel_shutdown,
        local_to_upstream,
        upstream_to_local,
    )
    .await;

    drop(session_registration);
    result
}

pub(super) async fn supervise_websocket_pumps(
    revocation: &CancellationToken,
    session_shutdown: &CancellationToken,
    tunnel_shutdown: &CancellationToken,
    mut local_to_upstream: JoinHandle<Result<(), WebSocketTunnelError>>,
    mut upstream_to_local: JoinHandle<Result<(), WebSocketTunnelError>>,
) -> Result<(), WebSocketTunnelError> {
    tokio::select! {
        () = revocation.cancelled() => {
            abort_websocket_pump(&mut local_to_upstream).await;
            abort_websocket_pump(&mut upstream_to_local).await;
            Ok(())
        }
        () = session_shutdown.cancelled() => {
            abort_websocket_pump(&mut local_to_upstream).await;
            abort_websocket_pump(&mut upstream_to_local).await;
            Ok(())
        }
        result = &mut local_to_upstream => {
            let local_result = flatten_websocket_pump_join(result);
            if local_result.is_err() || !tunnel_shutdown.is_cancelled() {
                abort_websocket_pump(&mut upstream_to_local).await;
                local_result
            } else {
                await_websocket_cleanup_pump(revocation, session_shutdown, &mut upstream_to_local).await
            }
        }
        result = &mut upstream_to_local => {
            let upstream_result = flatten_websocket_pump_join(result);
            if upstream_result.is_err() || !tunnel_shutdown.is_cancelled() {
                abort_websocket_pump(&mut local_to_upstream).await;
                upstream_result
            } else {
                await_websocket_cleanup_pump(revocation, session_shutdown, &mut local_to_upstream).await
            }
        }
    }
}

async fn await_websocket_cleanup_pump(
    revocation: &CancellationToken,
    session_shutdown: &CancellationToken,
    survivor: &mut JoinHandle<Result<(), WebSocketTunnelError>>,
) -> Result<(), WebSocketTunnelError> {
    tokio::select! {
        () = revocation.cancelled() => {
            abort_websocket_pump(survivor).await;
            Ok(())
        }
        () = session_shutdown.cancelled() => {
            abort_websocket_pump(survivor).await;
            Ok(())
        }
        result = &mut *survivor => flatten_websocket_pump_join(result),
    }
}

pub(super) struct LocalToUpstreamPumpContext {
    pub(super) revocation: CancellationToken,
    pub(super) session_shutdown: CancellationToken,
    pub(super) tunnel_shutdown: CancellationToken,
    pub(super) active_turn_reservation: ActiveTurnReservationState,
    pub(super) session_affinity_activity_handle: Option<SessionAffinityActivityHandle>,
    pub(super) account_turn_admission: AccountTurnAdmission,
    pub(super) early_floor_reconnect: CancellationToken,
    pub(super) quota_floor_reconnect: CancellationToken,
}

pub(super) async fn pump_local_to_upstream<LocalStream, UpstreamStream>(
    mut local_read: SplitStream<WebSocketStream<LocalStream>>,
    mut upstream_write: SplitSink<WebSocketStream<UpstreamStream>, Message>,
    context: LocalToUpstreamPumpContext,
) -> Result<(), WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
    UpstreamStream: AsyncRead + AsyncWrite + Unpin,
{
    let LocalToUpstreamPumpContext {
        revocation,
        session_shutdown,
        tunnel_shutdown,
        active_turn_reservation,
        session_affinity_activity_handle,
        account_turn_admission,
        early_floor_reconnect,
        quota_floor_reconnect,
    } = context;
    loop {
        tokio::select! {
            biased;
            () = quota_floor_reconnect.cancelled() => {
                // Let the other pump deliver the client reconnect signal before this
                // pump's completion can make the supervisor abort it.
                tunnel_shutdown.cancel();
                let _ = close_websocket_sink_best_effort(&mut upstream_write).await;
                return Ok(());
            }
            () = early_floor_reconnect.cancelled() => {
                tunnel_shutdown.cancel();
                let _ = close_websocket_sink_best_effort(&mut upstream_write).await;
                return Ok(());
            }
            () = tunnel_shutdown.cancelled() => {
                close_websocket_sink_best_effort(&mut upstream_write).await?;
                return Ok(());
            }
            () = revocation.cancelled() => {
                close_websocket_sink_best_effort(&mut upstream_write).await?;
                return Ok(());
            }
            () = session_shutdown.cancelled() => {
                close_websocket_sink_best_effort(&mut upstream_write).await?;
                return Ok(());
            }
            local_message = local_read.next() => {
                let Some(local_message) = local_message else {
                    close_websocket_sink_best_effort(&mut upstream_write).await?;
                    return Ok(());
                };
                let local_message = match local_message {
                    Ok(message) => message,
                    Err(error) if is_reset_without_closing_handshake(&error) => {
                        close_websocket_sink_best_effort(&mut upstream_write).await?;
                        return Ok(());
                    }
                    Err(error) => return Err(WebSocketTunnelError::Transport(error)),
                };
                let is_close = matches!(local_message, Message::Close(_));
                let is_response_create = is_response_create(&local_message);
                if is_response_create {
                    if account_turn_admission.before_next_create().await {
                        tunnel_shutdown.cancel();
                        let _ = close_websocket_sink_best_effort(&mut upstream_write).await;
                        return Ok(());
                    }
                    active_turn_reservation.reserve_if_idle(current_unix_seconds());
                }
                if is_close {
                    close_websocket_sink_best_effort(&mut upstream_write).await?;
                    return Ok(());
                }
                upstream_write.send(local_message).await?;
                if is_response_create
                    && let Some(activity_handle) = &session_affinity_activity_handle
                    && activity_handle.touch_if_current(current_unix_seconds()).is_err()
                {
                    tracing::warn!(target: "codex_router_proxy::websocket",
                        component = "session_account_affinity",
                        error.class = "cache_unavailable",
                        "codex_router.session_affinity_activity_degraded"
                    );
                }
            }
        }
    }
}
pub(super) async fn pump_upstream_to_local<LocalStream, UpstreamStream>(
    mut upstream_read: SplitStream<WebSocketStream<UpstreamStream>>,
    mut local_write: SplitSink<WebSocketStream<LocalStream>, Message>,
    mut context: UpstreamToLocalPumpContext,
) -> Result<(), WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
    UpstreamStream: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            biased;
            () = context.quota_floor_reconnect.cancelled() => {
                context.tunnel_shutdown.cancel();
                context.active_turn_reservation.retire();
                context.session_registry.note_quota_reconnect_signal();
                local_write
                    .send(Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL))
                    .await?;
                close_websocket_sink_best_effort(&mut local_write).await?;
                return Ok(());
            }
            () = context.early_floor_reconnect.cancelled() => {
                context.tunnel_shutdown.cancel();
                context.active_turn_reservation.retire();
                context.session_registry.note_quota_reconnect_signal();
                local_write
                    .send(Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL))
                    .await?;
                close_websocket_sink_best_effort(&mut local_write).await?;
                return Ok(());
            }
            changed = context.graceful_floor_switch.changed() => {
                if changed.is_ok() {
                    context.account_turn_admission.on_idle_intent().await;
                }
            }
            () = context.revocation.cancelled() => {
                close_websocket_sink_best_effort(&mut local_write).await?;
                return Ok(());
            }
            () = context.session_shutdown.cancelled() => {
                close_websocket_sink_best_effort(&mut local_write).await?;
                return Ok(());
            }
            upstream_message = upstream_read.next() => {
                let Some(upstream_message) = upstream_message else {
                    close_websocket_sink_best_effort(&mut local_write).await?;
                    return Ok(());
                };
                let upstream_message = match upstream_message {
                    Ok(message) => message,
                    Err(error) if is_reset_without_closing_handshake(&error) => {
                        close_websocket_sink_best_effort(&mut local_write).await?;
                        return Ok(());
                    }
                    Err(error) => return Err(WebSocketTunnelError::Transport(error)),
                };
                let is_close = matches!(upstream_message, Message::Close(_));
                let metadata_text = websocket_metadata_text_handle(&upstream_message);
                let provider_error_classification = provider_error_classification_from_message(&upstream_message);
                let provider_error_body = provider_error_body_from_message(&upstream_message);
                if provider_error_classification
                    == ProviderErrorClassification::AccountQuotaExhausted
                {
                    context.tunnel_shutdown.cancel();
                }
                let upstream_message =
                    maybe_replace_account_quota_exhaustion_with_reconnect_signal(
                        upstream_message,
                        provider_error_classification,
                        provider_error_body.as_deref(),
                        &context,
                    )
                    .await;
                if is_close {
                    close_websocket_sink_best_effort(&mut local_write).await?;
                    return Ok(());
                }
                let is_completed = metadata_text
                    .as_ref()
                    .is_some_and(|text| is_response_completed_text(text));
                let is_terminal_error = !upstream_message.close_after_send
                    && provider_error_classification
                        != ProviderErrorClassification::AccountQuotaExhausted
                    && metadata_text
                        .as_ref()
                        .is_some_and(|text| is_response_terminal_error_text(text));
                let is_terminal = is_completed
                    || metadata_text.as_ref().is_some_and(|text| {
                        is_response_failed_text(text) || is_response_incomplete_text(text)
                    })
                    || is_terminal_error;
                if is_terminal {
                    context
                        .account_turn_admission
                        .deliver_terminal_and_release_turn(async {
                            local_write.send(upstream_message.message).await?;
                            context
                                .session_registry
                                .note_upstream_message_forwarded(context.session_id);
                            if is_completed {
                                context.session_registry.clear_capacity_retry(context.session_id);
                                context.session_registry.note_response_completed(context.session_id);
                            }
                            context.active_turn_reservation.release_current();
                            Ok::<(), WebSocketTunnelError>(())
                        })
                        .await?;
                } else {
                    local_write.send(upstream_message.message).await?;
                    context
                        .session_registry
                        .note_upstream_message_forwarded(context.session_id);
                }
                if let Some(metadata_text) = metadata_text {
                    let affinity_owner_context = context.affinity_owner_context.clone();
                    let async_affinity_owner_recorder =
                        context.async_affinity_owner_recorder.clone();
                    let affinity_owner_recorder = context.affinity_owner_recorder.clone();
                    context.affinity_record_tasks.spawn(async move {
                        record_forwarded_websocket_metadata(
                            metadata_text,
                            affinity_owner_context.as_ref(),
                            async_affinity_owner_recorder,
                            affinity_owner_recorder,
                        )
                        .await;
                    });
                }
                if !matches!(provider_error_classification,
                    ProviderErrorClassification::AccountQuotaExhausted
                        | ProviderErrorClassification::ModelCapacity)
                    && provider_error_body.is_some()
                    && let Some(provider_error_observer) = context.provider_error_observer.clone()
                    && let Some(affinity_owner_context) = context.affinity_owner_context.as_ref()
                {
                    let account_id = affinity_owner_context.account_id.clone();
                    context.affinity_record_tasks.spawn(async move {
                        let _observation_result = provider_error_observer
                            .observe_provider_error(
                                account_id,
                                RouteBand::Responses,
                                provider_error_classification,
                                current_unix_seconds(),
                            )
                            .await;
                    });
                }
                if upstream_message.close_after_send {
                    context.tunnel_shutdown.cancel();
                    close_websocket_sink_best_effort(&mut local_write).await?;
                    return Ok(());
                }
            }
        }
    }
}
impl ActiveTurnReservationState {
    pub(super) fn new(initial_reservation: Option<ActiveReservationGuard>) -> Self {
        Self {
            reservation_template: initial_reservation.clone(),
            current_reservation: Arc::new(Mutex::new(initial_reservation)),
            retired: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn release_current(&self) {
        let Ok(mut current_reservation) = self.current_reservation.lock() else {
            return;
        };
        if let Some(reservation) = current_reservation.take() {
            reservation.release();
        }
    }

    pub(super) fn reserve_if_idle(&self, reserved_unix_seconds: u64) {
        if self.retired.load(Ordering::Acquire) {
            return;
        }
        let Some(template) = self.reservation_template.as_ref() else {
            return;
        };
        let Ok(mut current_reservation) = self.current_reservation.lock() else {
            return;
        };
        if current_reservation.is_some() {
            return;
        }
        *current_reservation = template.reserve_again_at(reserved_unix_seconds);
    }

    pub(super) fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.release_current();
    }
}

pub(super) async fn abort_websocket_pump(
    handle: &mut JoinHandle<Result<(), WebSocketTunnelError>>,
) {
    handle.abort();
    let _join_result = handle.await;
}

pub(super) fn flatten_websocket_pump_join(
    result: Result<Result<(), WebSocketTunnelError>, tokio::task::JoinError>,
) -> Result<(), WebSocketTunnelError> {
    match result {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Ok(()),
        Err(error) => Err(WebSocketTunnelError::TaskJoin(error.to_string())),
    }
}
