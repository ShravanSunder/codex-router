//! Apply the Host-only owner override before the shared OS account lookup.

use message_board::HumanId;

pub(crate) async fn resolve_face_owner_human_id(override_id: Option<HumanId>) -> Option<HumanId> {
    let result = match override_id {
        Some(owner) => Ok(owner),
        None => collaboration_client::resolve_owner_human_id().await,
    };
    match result {
        Ok(owner) => Some(owner),
        Err(error) => {
            tracing::error!(error_kind = ?error,
                "provider app-server faces unavailable: owner identity lookup failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_override_skips_os_lookup() {
        let chosen = HumanId::try_from("chosen-owner".to_owned()).expect("Human ID");
        assert_eq!(
            resolve_face_owner_human_id(Some(chosen.clone())).await,
            Some(chosen)
        );
    }
}
