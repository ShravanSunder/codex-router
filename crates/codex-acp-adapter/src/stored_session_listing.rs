//! ACP inventory maps the same read-only catalog used by the Sessions picker.
use crate::AcpStoredSessions;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use codex_native_integration::{
    StoredThreadCatalog, StoredThreadCursor, StoredThreadProvider, StoredThreadQuery,
    StoredThreadRoot, StoredThreadSort, StoredThreadSource,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
};

pub struct NativeStoredSessions {
    codex_home: PathBuf,
    scope: String,
}
impl NativeStoredSessions {
    #[must_use]
    pub fn new(codex_home: PathBuf, scope: String) -> Self {
        Self { codex_home, scope }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCursor {
    scope_fingerprint: String,
    sort_value: Option<i64>,
    session_id: String,
}
impl AcpStoredSessions for NativeStoredSessions {
    fn list(&self, params: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(self.list_page(params))
    }
}
impl NativeStoredSessions {
    async fn list_page(&self, params: Value) -> io::Result<Value> {
        let cwd = params
            .get("cwd")
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_str()
                    .filter(|cwd| Path::new(cwd).is_absolute())
                    .map(str::to_owned)
                    .ok_or_else(invalid_cursor)
            })
            .transpose()?;
        // Bind opaque pagination to both inputs without embedding unbounded paths.
        let scope_fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&self.scope, &cwd)).map_err(io::Error::other)?)
        );
        let cursor = params
            .get("cursor")
            .filter(|value| !value.is_null())
            .map(|value| {
                let text = value
                    .as_str()
                    .filter(|value| !value.is_empty() && value.len() <= 1024)
                    .ok_or_else(invalid_cursor)?;
                let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| invalid_cursor())?;
                let cursor: StoredCursor =
                    serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
                if cursor.scope_fingerprint != scope_fingerprint {
                    return Err(invalid_cursor());
                }
                Ok::<_, io::Error>(StoredThreadCursor {
                    sort_value: cursor.sort_value,
                    session_id: cursor.session_id,
                })
            })
            .transpose()?;
        let catalog = StoredThreadCatalog::open(&self.codex_home)
            .await
            .map_err(io::Error::other)?;
        let mut query = StoredThreadQuery {
            root: StoredThreadRoot::Any,
            provider: StoredThreadProvider::Any,
            source: StoredThreadSource::All,
            sort: StoredThreadSort::Updated,
            page_size: 100,
            cursor,
        };
        let mut sessions = Vec::new();
        let mut exhausted = false;
        // Bounded scans may yield an empty page with continuation; absence is not deletion.
        for _ in 0..10 {
            let rows = catalog.read_page(&query).await.map_err(io::Error::other)?;
            exhausted = rows.len() < 100;
            for row in rows {
                let session_id: String = row.try_get("id").map_err(io::Error::other)?;
                let recency: Option<i64> =
                    row.try_get("recency_at_ms").map_err(io::Error::other)?;
                query.cursor = Some(StoredThreadCursor {
                    sort_value: recency,
                    session_id: session_id.clone(),
                });
                let directory: Option<String> = row.try_get("cwd").map_err(io::Error::other)?;
                let Some(directory) =
                    directory.filter(|directory| Path::new(directory).is_absolute())
                else {
                    continue;
                };
                if cwd.as_ref().is_some_and(|filter| filter != &directory) {
                    continue;
                }
                let title: Option<String> = row
                    .try_get::<Option<String>, _>("name")
                    .map_err(io::Error::other)?
                    .or(row.try_get("title").map_err(io::Error::other)?);
                let updated = recency
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
                sessions.push(json!({"sessionId":session_id,"cwd":directory,"title":title,"updatedAt":updated}));
            }
            if exhausted || !sessions.is_empty() {
                break;
            }
        }
        catalog.close().await;
        let cursor = if exhausted {
            None
        } else {
            query
                .cursor
                .map(|cursor| {
                    serde_json::to_vec(&StoredCursor {
                        scope_fingerprint,
                        sort_value: cursor.sort_value,
                        session_id: cursor.session_id,
                    })
                    .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
                    .map_err(io::Error::other)
                })
                .transpose()?
        };
        Ok(json!({"sessions":sessions,"nextCursor":cursor}))
    }
}
fn invalid_cursor() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid stored session cursor or scope",
    )
}
