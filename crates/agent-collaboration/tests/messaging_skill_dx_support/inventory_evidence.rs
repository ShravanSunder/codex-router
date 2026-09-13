use crate::proof_context::{ProofContext, ProofResult};
use collaboration_client::protocol::{
    NativeSessionListParams, NativeSessionSummary, NativeSessionView, SessionRef,
};
use std::{path::Path, time::Duration};

pub(super) async fn list_all(
    proof: &mut ProofContext,
    view: NativeSessionView,
) -> ProofResult<Vec<NativeSessionSummary>> {
    let mut cursor = None;
    let mut sessions = Vec::new();
    loop {
        let page = proof
            .client
            .list_sessions(NativeSessionListParams {
                endpoint: proof.endpoint.clone(),
                view,
                page_size: 100,
                cursor,
            })
            .await?;
        sessions.extend(page.sessions);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(sessions);
        };
        cursor = Some(next_cursor);
    }
}

pub(super) async fn wait_until_stored(
    proof: &mut ProofContext,
    target: &SessionRef,
) -> ProofResult<Vec<NativeSessionSummary>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let sessions = list_all(proof, NativeSessionView::Stored).await?;
        if sessions.iter().any(|session| session.target == *target) {
            return Ok(sessions);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Prepared recipient did not become visible in stored inventory".into());
        }
    }
}

pub(super) fn require_single_session_at_cwd(
    sessions: &[NativeSessionSummary],
    cwd: &Path,
    expected: &SessionRef,
) -> ProofResult<()> {
    let expected_cwd = cwd.to_string_lossy();
    let matches = sessions
        .iter()
        .filter(|session| String::from(session.working_directory.clone()) == expected_cwd)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [session] if session.target == *expected => Ok(()),
        _ => Err(format!(
            "Expected exactly one owned session at {}; observed {}",
            cwd.display(),
            matches.len()
        )
        .into()),
    }
}
