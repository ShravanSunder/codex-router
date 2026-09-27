//! Trigger native permission routing and observe the notice at its configured approver.
use super::{
    delivery_matrix_support::PeerFixture,
    proof_context::{ProofContext, ProofResult},
    wait_for_input_marker,
};
use collaboration_client::{
    ConversationClient, ConversationCreateInput, ConversationPromptInput, PublicPromptContent,
    protocol::{
        ApprovalDecideParams, ApprovalDecision, ConversationCreateOutcome, OperationId,
        RouterAccess, SessionRef,
    },
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) async fn deliver_approval_notice(
    proof: &mut ProofContext,
    creator: &SessionRef,
    approver: &SessionRef,
    marker: &str,
    mut peer: Option<&mut PeerFixture>,
) -> ProofResult<String> {
    let conversation =
        ConversationClient::connect(&proof.service_directory, &proof.endpoint).await?;
    let created = conversation
        .create(
            ConversationCreateInput {
                operation_id: OperationId::generate(),
                endpoint: proof.endpoint.clone(),
                working_directory: proof.workspace.clone(),
                access: RouterAccess::WriteRestricted,
                created_by: creator.clone(),
                approver: Some(approver.clone()),
                generation: None,
                model: Some("gpt-5.6-luna".to_owned()),
                effort: Some("low".to_owned()),
                fork: None,
                root_message_id: None,
            },
            Duration::from_secs(60),
        )
        .await?;
    let requester = match created {
        ConversationCreateOutcome::Created { target, .. } => target,
        ConversationCreateOutcome::Pending { .. } => {
            return Err("Approval requester creation stayed pending".into());
        }
    };
    let output = proof.workspace.join(format!("approval-{marker}.txt"));
    let prompt = format!(
        "Use the command tool to run exactly `printf {marker} > {}`. This writes a proof file in the workspace root. If permission is requested, wait for the decision. Do not choose another command or path.",
        output.display(),
    );
    let service_directory = proof.service_directory.clone();
    let endpoint = proof.endpoint.clone();
    let working_directory = proof.workspace.clone();
    let requester_for_prompt = requester.clone();
    let creator_for_prompt = creator.clone();
    let approver_for_prompt = approver.clone();
    let cancellation = CancellationToken::new();
    let prompt_cancellation = cancellation.clone();
    let prompt_task = tokio::spawn(async move {
        let conversation = ConversationClient::connect(&service_directory, &endpoint).await?;
        conversation
            .prompt(
                ConversationPromptInput {
                    operation_id: None,
                    target: requester_for_prompt,
                    working_directory: Some(working_directory),
                    requested_by: creator_for_prompt.clone(),
                    approver: Some(approver_for_prompt),
                    message: PublicPromptContent::Agent {
                        sender: creator_for_prompt,
                        text: prompt.try_into().map_err(|_| {
                            collaboration_client::ConversationClientError::InvalidInput(
                                "invalid approval probe prompt",
                            )
                        })?,
                    },
                    effort: Some("low".to_owned()),
                    generation: None,
                },
                Duration::from_secs(120),
                prompt_cancellation,
            )
            .await
    });

    let request_id = wait_for_pending_request(proof, &requester, approver).await?;
    if let Some(fixture) = peer.as_mut() {
        fixture
            .expect_marker_with_timeout(&request_id, Duration::from_secs(90))
            .await?;
    } else {
        wait_for_input_marker(proof, approver, &request_id, Duration::from_secs(90)).await?;
    }
    proof
        .client
        .decide_approval(ApprovalDecideParams {
            request_id: request_id.clone(),
            decision: ApprovalDecision::Deny,
            actor: approver.clone(),
        })
        .await?;
    match tokio::time::timeout(Duration::from_secs(90), prompt_task).await {
        Ok(Ok(_settlement)) => {}
        Ok(Err(join_error)) => return Err(join_error.into()),
        Err(elapsed) => {
            cancellation.cancel();
            return Err(elapsed.into());
        }
    }
    if output.exists() {
        return Err("Denied approval probe unexpectedly wrote its workspace file".into());
    }
    Ok(request_id)
}

async fn wait_for_pending_request(
    proof: &mut ProofContext,
    requester: &SessionRef,
    approver: &SessionRef,
) -> ProofResult<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut poll = tokio::time::interval(Duration::from_millis(500));
    loop {
        poll.tick().await;
        let approvals = proof.client.list_pending_approvals(true).await?;
        if let Some(approval) = approvals
            .approvals
            .iter()
            .find(|record| record.requester == *requester && record.approver == *approver)
        {
            return Ok(approval.request_id.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Native approval request did not become pending".into());
        }
    }
}
