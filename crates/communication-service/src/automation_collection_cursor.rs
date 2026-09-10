//! One closed cursor encoding for automation collections; navigation is scoped, never authorization.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use communication_protocol::UuidIdentity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CollectionCursor {
    pub version: u8,
    pub service_id: UuidIdentity,
    pub collection: String,
    pub upper_key: (i64, String),
    pub last_key: (i64, String),
    pub filter_digest: String,
}
pub(crate) fn filter_digest(filters: &serde_json::Value) -> Result<String, ()> {
    let bytes = serde_json::to_vec(filters).map_err(|_| ())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub(crate) fn encode(cursor: &CollectionCursor) -> Result<String, ()> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| ())
}
pub(crate) fn decode(
    value: &str,
    service_id: &UuidIdentity,
    collection: &str,
    digest: &str,
) -> Result<CollectionCursor, ()> {
    if value.is_empty() || value.len() > 4096 {
        return Err(());
    }
    let decoded: CollectionCursor =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(value).map_err(|_| ())?).map_err(|_| ())?;
    if decoded.version != 1
        || decoded.service_id != *service_id
        || decoded.collection != collection
        || decoded.filter_digest != digest
        || decoded.last_key >= decoded.upper_key
        || decoded.last_key.0 < 0
        || decoded.upper_key.0 < 0
        || decoded.last_key.1.is_empty()
        || decoded.upper_key.1.is_empty()
    {
        return Err(());
    }
    Ok(decoded)
}
