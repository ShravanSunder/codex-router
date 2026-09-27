//! ACP lifecycle requests that are not tied to a prompt turn.

use std::{collections::BTreeSet, path::PathBuf};

use agent_client_protocol::schema::v1::{CloseSessionRequest, ListSessionsRequest};
use agent_client_protocol::{Agent, ConnectionTo};

use super::{ExternalProviderRuntimeError, ProviderSessionSummary, acp_operation_error};

pub(super) async fn list_provider_sessions(
    connection: ConnectionTo<Agent>,
    cwd: Option<PathBuf>,
) -> Result<Vec<ProviderSessionSummary>, ExternalProviderRuntimeError> {
    let mut sessions = Vec::new();
    let mut cursor = None::<String>;
    let mut seen_cursors = BTreeSet::new();
    loop {
        let request = ListSessionsRequest::new()
            .cwd(cwd.clone())
            .cursor(cursor.clone());
        let response = connection
            .send_request_to(Agent, request)
            .block_task()
            .await
            .map_err(acp_operation_error)?;
        sessions.extend(
            response
                .sessions
                .into_iter()
                .map(|session| ProviderSessionSummary {
                    provider_session_id: session.session_id.0.to_string(),
                    cwd: session.cwd,
                    title: session.title,
                    updated_at: session.updated_at,
                }),
        );
        let Some(next_cursor) = response.next_cursor else {
            return Ok(sessions);
        };
        if !seen_cursors.insert(next_cursor.clone()) || seen_cursors.len() > 128 {
            return Err(ExternalProviderRuntimeError::Operation(
                "provider session list pagination did not terminate".to_owned(),
            ));
        }
        cursor = Some(next_cursor);
    }
}

pub(super) async fn close_provider_session(
    connection: ConnectionTo<Agent>,
    provider_session_id: String,
) -> Result<(), ExternalProviderRuntimeError> {
    connection
        .send_request_to(Agent, CloseSessionRequest::new(provider_session_id))
        .block_task()
        .await
        .map(|_| ())
        .map_err(acp_operation_error)
}
