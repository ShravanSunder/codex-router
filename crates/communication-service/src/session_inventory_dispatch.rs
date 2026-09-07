//! Stored catalog and live native inventory keep separate, endpoint-bound pagination.
use crate::native_control_dispatch::NativeControlRequest;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use codex_native_integration::{
    NativeOperation, NativeProtocolConnection, StoredThreadCatalog, StoredThreadCursor,
    StoredThreadProvider, StoredThreadQuery, StoredThreadRoot, StoredThreadSort,
    StoredThreadSource,
};
use communication_protocol::{CodexGeneration, NativeSessionListParams, NativeSessionView};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryCursor {
    endpoint: communication_protocol::EndpointRef,
    view: String,
    generation: Option<CodexGeneration>,
    expires_at: Option<i64>,
    native_cursor: Option<String>,
    stored_time: Option<i64>,
    stored_id: Option<String>,
}
fn failed(id: Value, kind: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Session inventory unavailable","data":{"kind":kind,"stage":"discovery","message":"Session inventory unavailable"}}})
}
fn invalid(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid session inventory parameters or cursor"}})
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
            let Some(cursor) = decoded.filter(|c| c.endpoint == params.endpoint && c.view == view)
            else {
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
                .is_ok_and(|bytes| bytes.len() <= communication_protocol::MAX_CONTROL_FRAME_BYTES)
            {
                if matches!(params.view, NativeSessionView::Stored)
                    && let Some(observer) = request.stored_observation
                    && let Ok(page) = serde_json::from_value::<
                        communication_protocol::NativeSessionListResult,
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
        root: StoredThreadRoot::Any,
        provider: StoredThreadProvider::Any,
        source: StoredThreadSource::All,
        sort: StoredThreadSort::Updated,
        page_size: params.page_size as usize,
        cursor: cursor.and_then(|c| {
            c.stored_id.map(|id| StoredThreadCursor {
                sort_value: c.stored_time,
                session_id: id,
            })
        }),
    };
    let rows = catalog.read_page(&query).await.map_err(|_| ())?;
    let mut has_more = rows.len() == params.page_size as usize;
    let mut sessions = Vec::new();
    let mut last = None;
    // Reserve room for the endpoint, timestamp, maximum cursor and escaped
    // Control request ID. The final full-envelope check remains authoritative.
    let page_budget = communication_protocol::MAX_CONTROL_FRAME_BYTES.saturating_sub(4096);
    let mut page_bytes = 0usize;
    for row in rows {
        let id: String = row.try_get("id").map_err(|_| ())?;
        let time: Option<i64> = row.try_get("recency_at_ms").map_err(|_| ())?;
        let cwd: String = row.try_get("cwd").map_err(|_| ())?;
        let title: Option<String> = row
            .try_get::<Option<String>, _>("name")
            .map_err(|_| ())?
            .or(row.try_get("title").map_err(|_| ())?);
        let updated = time
            .and_then(chrono::DateTime::from_timestamp_millis)
            .ok_or(())?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let session = json!({"target":{"endpoint":params.endpoint,"sessionId":id},"title":title.unwrap_or_default(),"workingDirectory":cwd,"observation":{"kind":"stored","updatedAt":updated}});
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
        last = Some(InventoryCursor {
            endpoint: params.endpoint.clone(),
            view: "stored".into(),
            generation: None,
            expires_at: None,
            native_cursor: None,
            stored_time: time,
            stored_id: Some(id),
        });
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
        let status = thread.get("status").ok_or(())?;
        if matches!(params.view, NativeSessionView::Active)
            && status.get("type").and_then(Value::as_str) != Some("active")
        {
            continue;
        }
        sessions.push(json!({"target":{"endpoint":params.endpoint,"sessionId":id},"title":thread.get("name").and_then(Value::as_str).unwrap_or_default(),"workingDirectory":thread.get("cwd").ok_or(())?,"observation":{"kind":"runtime","status":status,"turnId":null}}));
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
    let _: communication_protocol::NativeSessionListResult =
        serde_json::from_value(result.clone()).map_err(|_| ())?;
    Ok(result)
}
