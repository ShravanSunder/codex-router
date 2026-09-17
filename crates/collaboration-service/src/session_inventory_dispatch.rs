//! Stored catalog and live native inventory keep separate, endpoint-bound pagination.
use crate::native_control_dispatch::NativeControlRequest;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use codex_native_integration::{
    NativeOperation, NativeProtocolConnection, StoredThreadCatalog, StoredThreadCursor,
    StoredThreadProvider, StoredThreadQuery, StoredThreadRoot, StoredThreadSort,
    StoredThreadSource,
};
use collaboration_protocol::{
    CodexGeneration, NativeSessionListParams, NativeSessionScope, NativeSessionSource,
    NativeSessionView,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryCursor {
    endpoint: collaboration_protocol::EndpointRef,
    view: String,
    generation: Option<CodexGeneration>,
    expires_at: Option<i64>,
    native_cursor: Option<String>,
    stored_time: Option<i64>,
    stored_id: Option<String>,
    scope: NativeSessionScope,
    source: NativeSessionSource,
    query: Option<String>,
}
fn failed(id: Value, kind: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Session inventory unavailable","data":{"kind":kind,"stage":"discovery","message":"Session inventory unavailable"}}})
}
fn invalid(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid session inventory parameters or cursor"}})
}
fn classify_source(source: Option<&str>, thread_source: Option<&str>) -> NativeSessionSource {
    if matches!(
        thread_source,
        Some("subagent" | "guardian_review" | "memory_consolidation")
    ) || source == Some("subagent")
        || source.is_some_and(|value| value.contains("subagent"))
    {
        NativeSessionSource::Subagents
    } else {
        NativeSessionSource::Interactive
    }
}
/// The repository identity a `Repo` scope carries, in the shape the canonical predicate
/// takes. Every surface that resolves `--repo` goes through the one predicate.
fn repository_identity_for_scope(
    scope: &NativeSessionScope,
) -> Option<codex_native_integration::RepositoryIdentity> {
    let NativeSessionScope::Repo {
        live_roots,
        normalized_origin,
        basename,
        fallback_cwd,
    } = scope
    else {
        return None;
    };
    Some(codex_native_integration::RepositoryIdentity {
        normalized_origin: normalized_origin.clone(),
        live_roots: live_roots.clone(),
        repository_basename: basename.clone(),
        fallback_cwd: fallback_cwd.clone(),
    })
}

fn runtime_scope_matches(scope: &NativeSessionScope, thread: &Value) -> bool {
    let Some(cwd) = thread.get("cwd").and_then(Value::as_str) else {
        return false;
    };
    let candidate = std::path::Path::new(cwd);
    let exact = |path: &std::path::Path| {
        codex_native_integration::path_sql_values(candidate)
            .iter()
            .any(|value| codex_native_integration::path_sql_values(path).contains(value))
    };
    let child = |root: &std::path::Path| {
        codex_native_integration::path_sql_values(candidate)
            .iter()
            .any(|candidate| {
                codex_native_integration::path_sql_values(root)
                    .iter()
                    .any(|root| {
                        candidate == root
                            || candidate
                                .strip_prefix(root)
                                .is_some_and(|suffix| suffix.starts_with('/'))
                    })
            })
    };
    match scope {
        NativeSessionScope::Any => true,
        NativeSessionScope::Cwd { path } => exact(path),
        NativeSessionScope::Checkout { root } => child(root),
        // The loaded and stored views must answer `--repo` the same way, so both call
        // the catalog's canonical predicate rather than a second reading of it.
        NativeSessionScope::Repo { .. } => {
            repository_identity_for_scope(scope).is_some_and(|identity| {
                codex_native_integration::repository_contains_session(
                    &identity,
                    thread.pointer("/gitInfo/originUrl").and_then(Value::as_str),
                    candidate,
                )
            })
        }
    }
}

/// Seconds between a last-updated instant and the service clock, for every row.
fn idle_seconds_since_ms(updated_at_ms: i64) -> u64 {
    u64::try_from(
        chrono::Utc::now()
            .timestamp_millis()
            .saturating_sub(updated_at_ms)
            / 1000,
    )
    .unwrap_or(0)
}

/// The native `Thread.updatedAt` is a Unix timestamp in seconds.
fn idle_seconds_from_thread(thread: &Value) -> u64 {
    thread
        .get("updatedAt")
        .and_then(Value::as_i64)
        .map_or(0, |seconds| {
            idle_seconds_since_ms(seconds.saturating_mul(1000))
        })
}

/// The identifier of the turn a busy session is running, when the runtime names one.
///
/// `ActiveThreadStatus` in `codex_app_server_protocol.v2.schemas.json` carries only
/// `type` and `activeFlags`, so an active thread reports no turn id today; a turn id
/// would require loading turns, which this read-only listing must not do.
fn active_turn_id(status: &Value) -> Option<&str> {
    if status.get("type").and_then(Value::as_str) != Some("active") {
        return None;
    }
    status
        .get("turnId")
        .or_else(|| status.pointer("/turn/id"))
        .and_then(Value::as_str)
}

pub(crate) async fn dispatch_inventory(request: NativeControlRequest<'_>) -> Value {
    let Ok(params) = serde_json::from_value::<NativeSessionListParams>(request.params) else {
        return invalid(request.id);
    };
    if !(1..=100).contains(&params.page_size) {
        return invalid(request.id);
    }
    if &params.endpoint.service_id != request.service_id {
        return failed(request.id, "wrongService");
    }
    if !request
        .endpoints
        .iter()
        .any(|e| e.endpoint == params.endpoint)
    {
        return failed(request.id, "endpointNotFound");
    }
    let Some(backend) = request.backend.filter(|b| b.endpoint == params.endpoint) else {
        return failed(request.id, "unsupportedCapability");
    };
    let view = match params.view {
        NativeSessionView::Stored => "stored",
        NativeSessionView::Loaded => "loaded",
        NativeSessionView::Active => "active",
    };
    let cursor = match &params.cursor {
        None => None,
        Some(text) => {
            if text.is_empty() || text.len() > 1024 {
                return invalid(request.id);
            }
            let decoded = URL_SAFE_NO_PAD
                .decode(text)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<InventoryCursor>(&bytes).ok());
            let Some(cursor) = decoded.filter(|c| {
                c.endpoint == params.endpoint
                    && c.view == view
                    && c.scope == params.scope
                    && c.source == params.source
                    && c.query == params.query
            }) else {
                return invalid(request.id);
            };
            Some(cursor)
        }
    };
    let result = if matches!(params.view, NativeSessionView::Stored) {
        if cursor.as_ref().is_some_and(|c| {
            c.generation.is_some()
                || c.native_cursor.is_some()
                || c.expires_at.is_some()
                || c.stored_id.as_ref().is_none_or(String::is_empty)
        }) {
            return invalid(request.id);
        }
        stored_page(&backend.codex_home, &params, cursor).await
    } else {
        let Ok(admission) = backend.gate.acquire() else {
            return failed(request.id, "unavailable");
        };
        let Some(schemas) = admission.schemas() else {
            return failed(request.id, "unsupportedCapability");
        };
        if cursor.as_ref().is_some_and(|c| {
            c.generation.as_ref() != Some(admission.generation())
                || c.expires_at
                    .is_none_or(|e| e <= chrono::Utc::now().timestamp())
                || c.stored_id.is_some()
                || c.stored_time.is_some()
        }) {
            return invalid(request.id);
        }
        let retired = admission.retirement();
        tokio::select! {
            _=retired.cancelled()=>Err(()),
            result=runtime_page(&params,cursor,&admission,&schemas)=>result,
        }
    };
    match result {
        Ok(result) => {
            // Admission/rendering must succeed before catalog observations enter the journal.
            let response = json!({"jsonrpc":"2.0","id":request.id,"result":result});
            if serde_json::to_vec(&response)
                .is_ok_and(|bytes| bytes.len() <= collaboration_protocol::MAX_CONTROL_FRAME_BYTES)
            {
                if matches!(params.view, NativeSessionView::Stored)
                    && let Some(observer) = request.stored_observation
                    && let Ok(page) = serde_json::from_value::<
                        collaboration_protocol::NativeSessionListResult,
                    >(result)
                {
                    // A storage failure invalidates C10 through LifecycleStore; it does not
                    // turn a successful read-only catalog operation into a native failure.
                    let _recorded = observer.record_page(&page).await;
                }
                response
            } else {
                failed(request.id, "overloaded")
            }
        }
        Err(()) => failed(request.id, "unavailable"),
    }
}
fn encode(cursor: InventoryCursor) -> Result<String, ()> {
    let value = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).map_err(|_| ())?);
    if value.len() > 1024 {
        Err(())
    } else {
        Ok(value)
    }
}
async fn stored_page(
    home: &std::path::Path,
    params: &NativeSessionListParams,
    cursor: Option<InventoryCursor>,
) -> Result<Value, ()> {
    let catalog = StoredThreadCatalog::open(home).await.map_err(|_| ())?;
    let query = StoredThreadQuery {
        root: match &params.scope {
            NativeSessionScope::Any => StoredThreadRoot::Any,
            NativeSessionScope::Cwd { path } => StoredThreadRoot::Cwd(path.clone()),
            NativeSessionScope::Checkout { root } => StoredThreadRoot::Checkout(root.clone()),
            NativeSessionScope::Repo {
                live_roots,
                normalized_origin,
                basename,
                fallback_cwd,
            } => StoredThreadRoot::Repo {
                live_roots: live_roots.clone(),
                normalized_origin: normalized_origin.clone(),
                basename: basename.clone(),
                fallback_cwd: fallback_cwd.clone(),
            },
        },
        provider: StoredThreadProvider::Any,
        source: match params.source {
            NativeSessionSource::All => StoredThreadSource::All,
            NativeSessionSource::Interactive => StoredThreadSource::Interactive,
            NativeSessionSource::Subagents => StoredThreadSource::Subagents,
        },
        sort: StoredThreadSort::Updated,
        page_size: params.page_size as usize,
        cursor: cursor.and_then(|c| {
            c.stored_id.map(|id| StoredThreadCursor {
                sort_value: c.stored_time,
                session_id: id,
            })
        }),
        query: params.query.clone(),
    };
    let rows = catalog.read_page(&query).await.map_err(|_| ())?;
    // The SQL repository clause bounds the scan; the canonical predicate decides. A page
    // may therefore return fewer rows than the page size. It never returns a gap: the
    // cursor advances over every row read, including the ones the predicate rejects.
    let repository_identity = repository_identity_for_scope(&params.scope);
    let mut has_more = rows.len() == params.page_size as usize;
    let mut sessions = Vec::new();
    let mut last = None;
    // Reserve room for the endpoint, timestamp, maximum cursor and escaped
    // Control request ID. The final full-envelope check remains authoritative.
    let page_budget = collaboration_protocol::MAX_CONTROL_FRAME_BYTES.saturating_sub(4096);
    let mut page_bytes = 0usize;
    for row in rows {
        let id: String = row.try_get("id").map_err(|_| ())?;
        let time: Option<i64> = row.try_get("recency_at_ms").map_err(|_| ())?;
        let cwd: String = row.try_get("cwd").map_err(|_| ())?;
        let model: Option<String> = row.try_get("model").map_err(|_| ())?;
        let reasoning_effort: Option<String> = row.try_get("reasoning_effort").map_err(|_| ())?;
        let name: Option<String> = row.try_get("name").map_err(|_| ())?;
        let title: Option<String> = row.try_get("title").map_err(|_| ())?;
        let git_branch: Option<String> = row.try_get("git_branch").map_err(|_| ())?;
        let source_value: Option<String> = row.try_get("source").map_err(|_| ())?;
        let thread_source: Option<String> = row.try_get("thread_source").map_err(|_| ())?;
        let source = classify_source(source_value.as_deref(), thread_source.as_deref());
        let git_origin_url: Option<String> = row.try_get("git_origin_url").map_err(|_| ())?;
        let scope_cursor = InventoryCursor {
            endpoint: params.endpoint.clone(),
            view: "stored".into(),
            generation: None,
            expires_at: None,
            native_cursor: None,
            stored_time: time,
            stored_id: Some(id.clone()),
            scope: params.scope.clone(),
            source: params.source,
            query: params.query.clone(),
        };
        if let Some(identity) = &repository_identity
            && !codex_native_integration::repository_contains_session(
                identity,
                git_origin_url.as_deref(),
                &codex_native_integration::normalize_path(std::path::Path::new(&cwd)),
            )
        {
            last = Some(scope_cursor);
            continue;
        }
        let updated = time
            .and_then(chrono::DateTime::from_timestamp_millis)
            .ok_or(())?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let idle_seconds = idle_seconds_since_ms(time.ok_or(())?);
        let session = json!({"target":{"endpoint":params.endpoint,"sessionId":id},"name":name,"title":title.unwrap_or_default(),"source":source,"gitBranch":git_branch,"workingDirectory":cwd,"observation":{"kind":"stored","updatedAt":updated},"model":model,"reasoningEffort":reasoning_effort,"idleSeconds":idle_seconds});
        let row_bytes = serde_json::to_vec(&session)
            .map_err(|_| ())?
            .len()
            .saturating_add(1);
        if !sessions.is_empty() && page_bytes.saturating_add(row_bytes) > page_budget {
            has_more = true;
            break;
        }
        // Do not truncate a title or skip an individually oversized first row.
        // The outer envelope guard reports overload if that row cannot fit alone.
        page_bytes = page_bytes.saturating_add(row_bytes);
        sessions.push(session);
        last = Some(scope_cursor);
    }
    catalog.close().await;
    let next = if has_more {
        last.map(encode).transpose()?
    } else {
        None
    };
    page(params, None, sessions, next)
}
async fn runtime_page(
    params: &NativeSessionListParams,
    cursor: Option<InventoryCursor>,
    admission: &crate::NativeAdmission,
    schemas: &codex_native_integration::NativePayloadSchemas,
) -> Result<Value, ()> {
    let mut native = NativeProtocolConnection::connect(admission.backend_path())
        .await
        .map_err(|_| ())?;
    let result = native
        .request_validated(
            schemas,
            NativeOperation::ListLoadedThreads,
            json!({"limit":params.page_size,"cursor":cursor.and_then(|c|c.native_cursor)}),
        )
        .await
        .map_err(|_| ())?;
    let ids = result
        .get("data")
        .and_then(Value::as_array)
        .filter(|ids| ids.len() <= params.page_size as usize)
        .ok_or(())?;
    let mut sessions = Vec::new();
    for id in ids {
        let id = id.as_str().ok_or(())?;
        let read = native
            .request_validated(
                schemas,
                NativeOperation::ReadThread,
                json!({"threadId":id,"includeTurns":false}),
            )
            .await
            .map_err(|_| ())?;
        let thread = read.get("thread").ok_or(())?;
        if thread.get("id").and_then(Value::as_str) != Some(id) {
            return Err(());
        }
        if !runtime_scope_matches(&params.scope, thread) {
            continue;
        }
        let status = thread.get("status").ok_or(())?;
        if matches!(params.view, NativeSessionView::Active)
            && status.get("type").and_then(Value::as_str) != Some("active")
        {
            continue;
        }
        let name = thread.get("name").and_then(Value::as_str);
        let title = thread
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let source = classify_source(
            thread.get("source").and_then(Value::as_str),
            thread.get("threadSource").and_then(Value::as_str),
        );
        if !matches!(params.source, NativeSessionSource::All)
            && std::mem::discriminant(&source) != std::mem::discriminant(&params.source)
        {
            continue;
        }
        if let Some(query) = params.query.as_deref()
            && !name.is_some_and(|value| value.to_lowercase().contains(&query.to_lowercase()))
            && !title.to_lowercase().contains(&query.to_lowercase())
        {
            continue;
        }
        sessions.push(json!({"target":{"endpoint":params.endpoint,"sessionId":id},"name":name,"title":title,"source":source,"gitBranch":thread.pointer("/gitInfo/branch").and_then(Value::as_str),"workingDirectory":thread.get("cwd").ok_or(())?,"observation":{"kind":"runtime","status":status,"turnId":active_turn_id(status)},"model":thread.get("model").and_then(Value::as_str),"reasoningEffort":thread.get("reasoningEffort").and_then(Value::as_str),"idleSeconds":idle_seconds_from_thread(thread)}));
    }
    let next = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(|value| {
            encode(InventoryCursor {
                endpoint: params.endpoint.clone(),
                view: if matches!(params.view, NativeSessionView::Active) {
                    "active"
                } else {
                    "loaded"
                }
                .into(),
                generation: Some(admission.generation().clone()),
                expires_at: Some(chrono::Utc::now().timestamp() + 60),
                native_cursor: Some(value.into()),
                stored_time: None,
                stored_id: None,
                scope: params.scope.clone(),
                source: params.source,
                query: params.query.clone(),
            })
        })
        .transpose()?;
    page(params, Some(admission.generation()), sessions, next)
}
fn page(
    params: &NativeSessionListParams,
    generation: Option<&CodexGeneration>,
    sessions: Vec<Value>,
    next: Option<String>,
) -> Result<Value, ()> {
    let mut result = json!({"endpoint":params.endpoint,"observedAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"generation":generation,"sessions":sessions});
    if let Some(next) = next {
        result
            .as_object_mut()
            .ok_or(())?
            .insert("nextCursor".into(), json!(next));
    }
    let _: collaboration_protocol::NativeSessionListResult =
        serde_json::from_value(result.clone()).map_err(|_| ())?;
    Ok(result)
}
