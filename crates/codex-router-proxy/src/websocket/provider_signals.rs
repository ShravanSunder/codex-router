use super::*;

pub(super) const CODEX_WEBSOCKET_RECONNECT_SIGNAL: &str = r#"{"type":"error","status":400,"error":{"type":"invalid_request_error","code":"websocket_connection_limit_reached","message":"Responses websocket connection limit reached (60 minutes). Create a new websocket connection to continue."}}"#;
pub(crate) const ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL: &str = r#"{"type":"error","status":429,"error":{"type":"usage_limit_reached","code":"codex_router_all_accounts_exhausted","message":"All configured codex-router accounts are out of usable quota."}}"#;
pub(crate) const ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL: &str = r#"{"type":"error","status":503,"error":{"type":"codex_router_quota_state_unavailable","code":"codex_router_quota_state_unavailable","message":"codex-router cannot safely rotate accounts because quota state is unavailable."}}"#;
pub(super) const POST_EXHAUSTION_ALTERNATIVE_SELECTION_TIMEOUT: Duration =
    Duration::from_millis(250);

pub(super) fn short_quota_wait_signal(retry_after_seconds: u64) -> String {
    format!(
        r#"{{"type":"response.failed","response":{{"id":"resp_router_short_quota_wait","status":"failed","error":{{"code":"rate_limit_exceeded","message":"Rate limit exceeded. Try again in {retry_after_seconds} seconds."}}}}}}"#
    )
}

pub(super) fn model_capacity_wait_signal(retry_after_seconds: u64) -> String {
    format!(
        r#"{{"type":"response.failed","response":{{"id":"resp_router_model_capacity_wait","status":"failed","error":{{"code":"rate_limit_exceeded","message":"Rate limit exceeded. Try again in {retry_after_seconds} seconds."}}}}}}"#
    )
}
pub(super) fn selection_close_reason_from_http_error(
    error: HttpProxyError,
) -> WebSocketCloseReason {
    match error {
        HttpProxyError::Selection { reason } => WebSocketCloseReason::Selection { reason },
        _ => WebSocketCloseReason::Selection {
            reason: QuotaAwareAccountSelectorError::StateUnavailable,
        },
    }
}
pub(super) struct UpstreamMessageOutcome {
    pub(super) message: Message,
    pub(super) close_after_send: bool,
}

pub(super) async fn maybe_replace_account_quota_exhaustion_with_reconnect_signal(
    upstream_message: Message,
    classification: ProviderErrorClassification,
    provider_error_body: Option<&[u8]>,
    context: &UpstreamToLocalPumpContext,
) -> UpstreamMessageOutcome {
    if classification == ProviderErrorClassification::ModelCapacity {
        return match context
            .session_registry
            .record_capacity_retry(context.session_id)
        {
            Some(CapacityRetryOutcome::Retry {
                retry_after_seconds,
            }) => UpstreamMessageOutcome {
                message: Message::text(model_capacity_wait_signal(retry_after_seconds)),
                close_after_send: true,
            },
            Some(CapacityRetryOutcome::Exhausted | CapacityRetryOutcome::Full) | None => {
                UpstreamMessageOutcome {
                    message: upstream_message,
                    close_after_send: false,
                }
            }
        };
    }
    if classification != ProviderErrorClassification::AccountQuotaExhausted {
        return UpstreamMessageOutcome {
            message: upstream_message,
            close_after_send: false,
        };
    }
    let Some(_provider_error_body) = provider_error_body else {
        return UpstreamMessageOutcome {
            message: upstream_message,
            close_after_send: false,
        };
    };
    let Some(provider_error_observer) = context.provider_error_observer.as_ref() else {
        return UpstreamMessageOutcome {
            message: upstream_message,
            close_after_send: false,
        };
    };
    let Some(affinity_owner_context) = context.affinity_owner_context.as_ref() else {
        return UpstreamMessageOutcome {
            message: upstream_message,
            close_after_send: false,
        };
    };

    let observed_unix_seconds = current_unix_seconds();
    let exhausted_account_id = affinity_owner_context.account_id.clone();
    context.active_turn_reservation.retire();
    if provider_error_observer
        .mark_runtime_account_quota_exhausted(
            exhausted_account_id.clone(),
            RouteBand::Responses,
            observed_unix_seconds,
        )
        .is_err()
    {
        crate::telemetry::record_websocket_event(
            RouteBand::Responses.as_str(),
            "quota_state_unavailable",
        );
        return UpstreamMessageOutcome {
            message: Message::text(ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL),
            close_after_send: true,
        };
    }

    let post_exhaustion_outcome = provider_error_observer.route_band_post_exhaustion_outcome(
        exhausted_account_id.clone(),
        RouteBand::Responses,
        codex_router_core::route_profile::RESPONSES_WEBSOCKET.clone(),
        observed_unix_seconds,
    );
    tokio::pin!(post_exhaustion_outcome);
    let post_exhaustion_outcome_result = tokio::select! {
        result = &mut post_exhaustion_outcome => result,
        () = context.revocation.cancelled() => {
            Err(ProviderErrorObservationError::SelectionStateUnavailable)
        }
        () = context.session_shutdown.cancelled() => {
            Err(ProviderErrorObservationError::SelectionStateUnavailable)
        }
        () = tokio::time::sleep(POST_EXHAUSTION_ALTERNATIVE_SELECTION_TIMEOUT) => {
            Err(ProviderErrorObservationError::SelectionStateUnavailable)
        }
    };

    let short_quota_wait = matches!(
        post_exhaustion_outcome_result,
        Ok(PostExhaustionRouteBandOutcome::ShortQuotaWait { .. })
    );
    let enqueue_result = if short_quota_wait {
        DbWriteEnqueueResult::Enqueued
    } else {
        provider_error_observer.enqueue_provider_quota_exhaustion(
            exhausted_account_id,
            RouteBand::Responses,
            classification,
            observed_unix_seconds,
        )
    };
    if !short_quota_wait
        && matches!(
            enqueue_result,
            DbWriteEnqueueResult::FullDegraded | DbWriteEnqueueResult::ClosedDegraded
        )
    {
        crate::telemetry::record_websocket_event(
            RouteBand::Responses.as_str(),
            "quota_state_unavailable",
        );
        return UpstreamMessageOutcome {
            message: Message::text(ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL),
            close_after_send: true,
        };
    }

    match post_exhaustion_outcome_result {
        Ok(PostExhaustionRouteBandOutcome::SelectableAlternative) => {
            crate::telemetry::record_websocket_event(
                RouteBand::Responses.as_str(),
                "quota_reconnect",
            );
            context.session_registry.note_quota_reconnect_signal();
            UpstreamMessageOutcome {
                message: Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL),
                close_after_send: true,
            }
        }
        Ok(PostExhaustionRouteBandOutcome::ShortQuotaWait {
            retry_after_seconds,
        }) => {
            crate::telemetry::record_websocket_event(
                RouteBand::Responses.as_str(),
                "quota_short_window_wait",
            );
            UpstreamMessageOutcome {
                message: Message::text(short_quota_wait_signal(retry_after_seconds)),
                close_after_send: true,
            }
        }
        Ok(PostExhaustionRouteBandOutcome::NoSelectableAlternative) => {
            crate::telemetry::record_websocket_event(
                RouteBand::Responses.as_str(),
                "quota_all_accounts_exhausted",
            );
            UpstreamMessageOutcome {
                message: Message::text(ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL),
                close_after_send: true,
            }
        }
        Err(_error) => {
            crate::telemetry::record_websocket_event(
                RouteBand::Responses.as_str(),
                "quota_state_unavailable",
            );
            UpstreamMessageOutcome {
                message: Message::text(ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL),
                close_after_send: true,
            }
        }
    }
}
