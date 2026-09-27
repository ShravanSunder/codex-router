//! Read-only provider inventory from Router's durable Session records.

use crate::ServiceIdentity;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use collaboration_protocol::{
    ChannelDescription, NativeSessionScope, NativeSessionSource, NativeSessionView,
    ProviderSessionListParams, ProviderSessionListResult, ProviderSessionState,
    ProviderSessionSummary,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

fn failure(id: Value, kind: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,
        "message":"Provider session inventory unavailable",
        "data":{"kind":kind,"stage":"discovery","message":"Provider session inventory unavailable"}}})
}

fn invalid(id: Value, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":message}})
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

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<ProviderSessionListParams>(params) else {
        return invalid(id, "Invalid provider session inventory parameters");
    };
    if !(1..=100).contains(&params.page_size) {
        return invalid(id, "Provider session page size must be 1..100");
    }
    if !matches!(params.source, NativeSessionSource::All) {
        return invalid(id, "Provider sessions carry no source; use source all");
    }
    if params.endpoint.service_id != identity.service_id {
        return failure(id, "wrongService");
    }
    let description = match identity.directory.read_endpoint(&params.endpoint) {
        Ok(Some(description)) => description,
        Ok(None) => return failure(id, "endpointNotFound"),
        Err(_) => return failure(id, "unavailable"),
    };
    if !description
        .channels
        .iter()
        .any(|channel| matches!(channel, ChannelDescription::ExternalProvider { .. }))
    {
        return failure(id, "unsupportedCapability");
    }
    let cursor = match &params.cursor {
        None => None,
        Some(encoded) => {
            if encoded.is_empty() || encoded.len() > 1024 {
                return invalid(id, "Invalid provider session inventory cursor");
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
                return invalid(id, "Invalid provider session inventory cursor");
            };
            Some(cursor)
        }
    };
    let Some(store) = identity.provider_operations.as_ref() else {
        return failure(id, "unavailable");
    };
    let inventory = match store.lock().await.list_sessions(&params.endpoint).await {
        Ok(inventory) => inventory,
        Err(_) => return failure(id, "unavailable"),
    };
    let mut rows: Vec<(i64, String, ProviderSessionSummary)> = Vec::new();
    let mut next_cursor = None;
    for record in inventory {
        let session_id = String::from(record.target.session_id.clone());
        let cwd = String::from(record.working_directory.clone());
        if !scope_matches(&params.scope, &cwd)
            || params.query.as_ref().is_some_and(|query| {
                let query = query.to_lowercase();
                !session_id.to_lowercase().contains(&query) && !cwd.to_lowercase().contains(&query)
            })
            || cursor
                .as_ref()
                .is_some_and(|cursor| !cursor_after(record.updated_at_ms, &session_id, cursor))
        {
            continue;
        }
        let state = if let Some(hub) = identity.provider_session_hub.as_ref() {
            let hub_session = match serde_json::to_value(&record.target)
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
            {
                Some(session) => session,
                None => return failure(id, "unavailable"),
            };
            match hub.state(hub_session).await {
                Ok(state) => state_tag(&state),
                Err(_) => return failure(id, "unavailable"),
            }
        } else {
            ProviderSessionState::Unloaded
        };
        if !view_matches(params.view, state) {
            continue;
        }
        if rows.len() == params.page_size as usize {
            let previous = match rows.last() {
                Some(previous) => previous,
                None => return failure(id, "unavailable"),
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
            next_cursor = serde_json::to_vec(&cursor)
                .ok()
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes));
            if next_cursor
                .as_ref()
                .is_none_or(|cursor| cursor.len() > 1024)
            {
                return failure(id, "overloaded");
            }
            break;
        }
        let approver = match serde_json::to_value(&record.approver)
            .ok()
            .and_then(|value| serde_json::from_value(value).ok())
        {
            Some(session) => message_board::Identity::Session { session },
            None => return failure(id, "unavailable"),
        };
        rows.push((
            record.updated_at_ms,
            session_id,
            ProviderSessionSummary {
                target: record.target,
                working_directory: record.working_directory,
                updated_at: record.updated_at_ms.div_euclid(1_000),
                state,
                approver,
                created_by: record.created_by,
            },
        ));
    }
    let observed_at = match collaboration_protocol::ObservationTimestamp::try_from(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    ) {
        Ok(time) => time,
        Err(_) => return failure(id, "unavailable"),
    };
    let result = ProviderSessionListResult {
        endpoint: params.endpoint,
        observed_at,
        sessions: rows.into_iter().map(|(_, _, row)| row).collect(),
        next_cursor,
    };
    let response = json!({"jsonrpc":"2.0","id":id,"result":result});
    if serde_json::to_vec(&response)
        .is_ok_and(|bytes| bytes.len() <= collaboration_protocol::MAX_CONTROL_FRAME_BYTES)
    {
        response
    } else {
        failure(id, "overloaded")
    }
}
