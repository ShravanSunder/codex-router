//! Opaque callback IDs retain exact session routing without delimiter ambiguity.
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
struct PermissionAddress {
    session: String,
    turn: String,
    sequence: u64,
}
pub(crate) fn permission_id(
    session: &str,
    turn: &str,
    sequence: u64,
) -> Result<String, serde_json::Error> {
    Ok(format!(
        "permission:{}",
        serde_json::to_string(&PermissionAddress {
            session: session.to_owned(),
            turn: turn.to_owned(),
            sequence
        })?
    ))
}
pub(crate) fn permission_session(id: &str) -> Option<String> {
    serde_json::from_str::<PermissionAddress>(id.strip_prefix("permission:")?)
        .ok()
        .map(|address| address.session)
}
