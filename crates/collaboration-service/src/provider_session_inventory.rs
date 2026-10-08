//! Read-only provider inventory from Router's durable Session records.

use crate::ServiceIdentity;
use crate::collaboration_application::{
    ProviderInventoryFailure, ProviderInventoryFailureKind, ResultByteBudget,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use claude_code_peer_messaging::PeerSessionStatus;
use collaboration_protocol::{
    ChannelDescription, ClaudeCodeInteractiveOrigin, ClaudeCodeInteractiveStatus,
    HostedProviderOrigin, NativeSessionScope, NativeSessionSource, NativeSessionView,
    ProviderSessionListParams, ProviderSessionListResult, ProviderSessionState,
    ProviderSessionSummary, ProviderWorkingDirectory, SessionRef,
};
use serde::{Deserialize, Serialize};
use session_event_model::SessionState;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderInventoryCursor {
    endpoint: collaboration_protocol::EndpointRef,
    view: String,
    scope: NativeSessionScope,
    source: NativeSessionSource,
    query: Option<String>,
    updated_at_ms: i64,
    session_id: String,
    expires_at: i64,
}

fn failure(kind: ProviderInventoryFailureKind) -> ProviderInventoryFailure {
    ProviderInventoryFailure::Unavailable(kind)
}

fn invalid(message: &'static str) -> ProviderInventoryFailure {
    ProviderInventoryFailure::InvalidRequest(message)
}

fn view_name(view: NativeSessionView) -> &'static str {
    match view {
        NativeSessionView::Stored => "stored",
        NativeSessionView::Loaded => "loaded",
        NativeSessionView::Active => "active",
    }
}

fn state_tag(state: &SessionState) -> ProviderSessionState {
    match state {
        SessionState::Unloaded => ProviderSessionState::Unloaded,
        SessionState::Idle => ProviderSessionState::Idle,
        SessionState::Running => ProviderSessionState::Running,
        SessionState::RequiresAction { .. } => ProviderSessionState::RequiresAction,
        SessionState::AuthenticationRequired => ProviderSessionState::AuthenticationRequired,
        SessionState::Closed => ProviderSessionState::Closed,
    }
}

fn view_matches(view: NativeSessionView, state: ProviderSessionState) -> bool {
    match view {
        NativeSessionView::Stored => true,
        NativeSessionView::Loaded => !matches!(
            state,
            ProviderSessionState::Unloaded | ProviderSessionState::Closed
        ),
        NativeSessionView::Active => matches!(
            state,
            ProviderSessionState::Running | ProviderSessionState::RequiresAction
        ),
    }
}

fn scope_matches(scope: &NativeSessionScope, cwd: &str) -> bool {
    let candidate = std::path::Path::new(cwd);
    let candidate_values = codex_native_integration::path_sql_values(candidate);
    match scope {
        NativeSessionScope::Any => true,
        NativeSessionScope::Cwd { path } => candidate_values
            .iter()
            .any(|value| codex_native_integration::path_sql_values(path).contains(value)),
        NativeSessionScope::Checkout { root } => {
            let root_values = codex_native_integration::path_sql_values(root);
            candidate_values.iter().any(|candidate| {
                root_values.iter().any(|root| {
                    candidate == root
                        || candidate
                            .strip_prefix(root)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                })
            })
        }
        NativeSessionScope::Repo {
            live_roots,
            normalized_origin,
            basename,
            fallback_cwd,
        } => {
            let identity = codex_native_integration::RepositoryIdentity {
                normalized_origin: normalized_origin.clone(),
                live_roots: live_roots.clone(),
                repository_basename: basename.clone(),
                fallback_cwd: fallback_cwd.clone(),
            };
            codex_native_integration::repository_contains_session(&identity, None, candidate)
        }
    }
}

fn cursor_after(record_time: i64, record_id: &str, cursor: &ProviderInventoryCursor) -> bool {
    record_time < cursor.updated_at_ms
        || (record_time == cursor.updated_at_ms && record_id > cursor.session_id.as_str())
}

fn interactive_status(status: &PeerSessionStatus) -> ClaudeCodeInteractiveStatus {
    match status {
        PeerSessionStatus::Busy => ClaudeCodeInteractiveStatus::Busy,
        PeerSessionStatus::Idle => ClaudeCodeInteractiveStatus::Idle,
        PeerSessionStatus::Waiting => ClaudeCodeInteractiveStatus::Waiting,
        PeerSessionStatus::Shell => ClaudeCodeInteractiveStatus::Shell,
        PeerSessionStatus::Unreported => ClaudeCodeInteractiveStatus::Unreported,
        PeerSessionStatus::Other(_) => ClaudeCodeInteractiveStatus::Other,
    }
}

/// Pages a provider endpoint's hosted sessions, or Claude Code's live terminal sessions.
pub(crate) async fn list_provider_sessions(
    identity: &ServiceIdentity,
    params: ProviderSessionListParams,
    budget: ResultByteBudget,
) -> Result<ProviderSessionListResult, ProviderInventoryFailure> {
    if !(1..=100).contains(&params.page_size) {
        return Err(invalid("Provider session page size must be 1..100"));
    }
    let is_claude = String::from(params.endpoint.endpoint_id.clone()) == "claude-local";
    if !is_claude && !matches!(params.source, NativeSessionSource::All) {
        return Err(invalid("Provider sessions carry no source; use source all"));
    }
    if is_claude && matches!(params.source, NativeSessionSource::Subagents) {
        return Err(invalid(
            "Claude Code terminal discovery supports source interactive or all",
        ));
    }
    if is_claude
        && matches!(params.view, NativeSessionView::Stored)
        && matches!(params.source, NativeSessionSource::Interactive)
    {
        return Err(ProviderInventoryFailure::NoStoredTerminalInventory);
    }
    if params.endpoint.service_id != identity.service_id {
        return Err(failure(ProviderInventoryFailureKind::WrongService));
    }
    let description = match identity.directory.read_endpoint(&params.endpoint) {
        Ok(Some(description)) => description,
        Ok(None) => return Err(failure(ProviderInventoryFailureKind::EndpointNotFound)),
        Err(_) => return Err(failure(ProviderInventoryFailureKind::Unavailable)),
    };
    if !is_claude
        && !description
            .channels
            .iter()
            .any(|channel| matches!(channel, ChannelDescription::ExternalProvider { .. }))
    {
        return Err(failure(ProviderInventoryFailureKind::UnsupportedCapability));
    }
    let cursor = match &params.cursor {
        None => None,
        Some(encoded) => {
            if encoded.is_empty() || encoded.len() > 1024 {
                return Err(invalid("Invalid provider session inventory cursor"));
            }
            let decoded = URL_SAFE_NO_PAD
                .decode(encoded)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ProviderInventoryCursor>(&bytes).ok());
            let Some(cursor) = decoded.filter(|cursor| {
                cursor.endpoint == params.endpoint
                    && cursor.view == view_name(params.view)
                    && cursor.scope == params.scope
                    && cursor.source == params.source
                    && cursor.query == params.query
                    && cursor.expires_at > chrono::Utc::now().timestamp()
            }) else {
                return Err(invalid("Invalid provider session inventory cursor"));
            };
            Some(cursor)
        }
    };
    let inventory = if matches!(params.source, NativeSessionSource::Interactive) {
        Vec::new()
    } else {
        let Some(store) = identity.provider_operations.as_ref() else {
            return Err(failure(ProviderInventoryFailureKind::Unavailable));
        };
        match store.lock().await.list_sessions(&params.endpoint).await {
            Ok(inventory) => inventory,
            Err(_) => return Err(failure(ProviderInventoryFailureKind::Unavailable)),
        }
    };
    let mut rows: Vec<(i64, String, ProviderSessionSummary)> = Vec::new();
    let mut skipped_records = None;
    if is_claude && !matches!(params.view, NativeSessionView::Stored) {
        if let Some(registry) = identity.claude_code_sessions.clone() {
            let live = match tokio::task::spawn_blocking(move || registry.live_sessions()).await {
                Ok(Ok(live)) => live,
                _ => return Err(failure(ProviderInventoryFailureKind::Unavailable)),
            };
            let mut skipped = live.skipped_records;
            for session in live.sessions {
                let session_id = String::from(session.session_id.clone());
                let cwd = session.cwd.to_string_lossy().into_owned();
                let target = SessionRef {
                    endpoint: params.endpoint.clone(),
                    session_id: session.session_id.clone(),
                };
                let Some(working_directory) = ProviderWorkingDirectory::try_from(cwd.clone()).ok()
                else {
                    skipped = skipped.saturating_add(1);
                    continue;
                };
                if !scope_matches(&params.scope, &cwd)
                    || params.query.as_ref().is_some_and(|query| {
                        let query = query.to_lowercase();
                        !session_id.to_lowercase().contains(&query)
                            && !cwd.to_lowercase().contains(&query)
                            && !session
                                .name
                                .as_ref()
                                .is_some_and(|name| name.to_lowercase().contains(&query))
                    })
                {
                    continue;
                }
                if let Some(name) = session.name.as_deref() {
                    identity.display_names.remember(target.clone(), name);
                } else {
                    identity.display_names.forget(target.clone());
                }
                rows.push((
                    session.updated_at.saturating_mul(1_000),
                    session_id,
                    ProviderSessionSummary::ClaudeCodeInteractive {
                        origin: ClaudeCodeInteractiveOrigin::ClaudeCodeInteractive,
                        target,
                        name: session.name,
                        working_directory,
                        status: interactive_status(&session.status),
                        started_at: session.started_at,
                        updated_at: session.updated_at,
                        status_updated_at: session.status_updated_at,
                        kind: session.kind,
                        entrypoint: session.entrypoint,
                    },
                ));
            }
            skipped_records = Some(skipped);
        } else {
            return Err(failure(ProviderInventoryFailureKind::Unavailable));
        }
    }
    for record in inventory {
        let session_id = String::from(record.target.session_id.clone());
        let cwd = String::from(record.working_directory.clone());
        if !scope_matches(&params.scope, &cwd)
            || params.query.as_ref().is_some_and(|query| {
                let query = query.to_lowercase();
                !session_id.to_lowercase().contains(&query) && !cwd.to_lowercase().contains(&query)
            })
        {
            continue;
        }
        let state = if let Some(hub) = identity.provider_session_hub.as_ref() {
            let hub_session = match serde_json::to_value(&record.target)
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
            {
                Some(session) => session,
                None => return Err(failure(ProviderInventoryFailureKind::Unavailable)),
            };
            match hub.state(hub_session).await {
                Ok(state) => state_tag(&state),
                Err(_) => return Err(failure(ProviderInventoryFailureKind::Unavailable)),
            }
        } else {
            ProviderSessionState::Unloaded
        };
        if !view_matches(params.view, state) {
            continue;
        }
        let approver = record.approver;
        rows.push((
            record.updated_at_ms,
            session_id,
            ProviderSessionSummary::HostedProvider {
                origin: is_claude.then_some(HostedProviderOrigin::HostedProvider),
                target: record.target,
                working_directory: record.working_directory,
                updated_at: record.updated_at_ms.div_euclid(1_000),
                state,
                approver,
                created_by: record.created_by,
            },
        ));
    }
    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    if let Some(cursor) = &cursor {
        rows.retain(|(time, session_id, _)| cursor_after(*time, session_id, cursor));
    }
    let has_more = rows.len() > params.page_size as usize;
    rows.truncate(params.page_size as usize);
    let next_cursor = if has_more {
        let Some(previous) = rows.last() else {
            return Err(failure(ProviderInventoryFailureKind::Unavailable));
        };
        let cursor = ProviderInventoryCursor {
            endpoint: params.endpoint.clone(),
            view: view_name(params.view).to_owned(),
            scope: params.scope.clone(),
            source: params.source,
            query: params.query.clone(),
            updated_at_ms: previous.0,
            session_id: previous.1.clone(),
            expires_at: chrono::Utc::now().timestamp() + 60,
        };
        let encoded = serde_json::to_vec(&cursor)
            .ok()
            .map(|bytes| URL_SAFE_NO_PAD.encode(bytes));
        if encoded.as_ref().is_none_or(|cursor| cursor.len() > 1024) {
            return Err(failure(ProviderInventoryFailureKind::ResponseTooLarge));
        }
        encoded
    } else {
        None
    };
    let observed_at = match collaboration_protocol::ObservationTimestamp::try_from(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    ) {
        Ok(time) => time,
        Err(_) => return Err(failure(ProviderInventoryFailureKind::Unavailable)),
    };
    let result = ProviderSessionListResult {
        endpoint: params.endpoint,
        observed_at,
        sessions: rows.into_iter().map(|(_, _, row)| row).collect(),
        skipped_records,
        next_cursor,
    };
    if budget.admits(&result) {
        Ok(result)
    } else {
        Err(failure(ProviderInventoryFailureKind::ResponseTooLarge))
    }
}
