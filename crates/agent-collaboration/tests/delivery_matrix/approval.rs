//! Scripted provider permission requests, observed at each configured approver.
use super::{
    delivery_matrix_support::PeerFixture,
    proof_context::{ProofContext, ProofResult},
    wait_for_input_marker,
};
use collaboration_client::{
    ConversationClient, ConversationClientError, ConversationCreateActor, ConversationCreateInput,
    ConversationOperationResult, ConversationPromptInput, PublicPromptContent,
    board::Identity,
    protocol::{
        ApprovalDecideParams, ConversationCreateOutcome, OperationId, RouterAccess, SessionRef,
    },
};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) async fn deliver_approval_notice(
    proof: &mut ProofContext,
    creator: &SessionRef,
    approver: &SessionRef,
    marker: &str,
    mut peer: Option<&mut PeerFixture>,
) -> ProofResult<String> {
    let provider = proof
        .client
        .list_endpoints()
        .await?
        .endpoints
        .into_iter()
        .find(|entry| String::from(entry.endpoint.endpoint_id.clone()) == "cursor-local")
        .ok_or("Scripted Cursor ACP endpoint missing")?
        .endpoint;
    let conversation = ConversationClient::connect(
        &collaboration_client::CollaborationAccess::api(&proof.service_directory),
        &provider,
    )
    .await?;
    let created = conversation
        .create(
            ConversationCreateInput {
                operation_id: OperationId::generate(),
                endpoint: provider.clone(),
                working_directory: proof.workspace.clone(),
                access: RouterAccess::WriteRestricted,
                created_by: ConversationCreateActor::Session(creator.clone()),
                approver: Some(ConversationCreateActor::Session(approver.clone())),
                generation: None,
                model: None,
                mode: None,
                effort: None,
                fork: None,
                root_message_id: None,
            },
            Duration::from_secs(60),
        )
        .await?;
    let requester = match created {
        ConversationCreateOutcome::Created { target, .. }
        | ConversationCreateOutcome::CreatedWithoutSettings { target, .. } => target,
        ConversationCreateOutcome::Pending { .. } => {
            return Err("Scripted provider create stayed pending".into());
        }
    };
    proof.record("approvalRequesterCreated", json!({"target":requester}))?;
    let service_directory = proof.service_directory.clone();
    let requester_for_prompt = requester.clone();
    let creator_for_prompt = creator.clone();
    let approver_for_prompt = approver.clone();
    let prompt = format!("Request permission for {marker} and wait for the approver decision.");
    let cancellation = CancellationToken::new();
    let _cancel_prompt_on_drop = cancellation.clone().drop_guard();
    let prompt_cancellation = cancellation.clone();
    let mut prompt_task = tokio::spawn(async move {
        let conversation = ConversationClient::connect(
            &collaboration_client::CollaborationAccess::api(&service_directory),
            &provider,
        )
        .await?;
        conversation
            .prompt(
                ConversationPromptInput {
                    operation_id: Some(OperationId::generate()),
                    target: requester_for_prompt,
                    working_directory: None,
                    requested_by: creator_for_prompt.clone(),
                    approver: Some(approver_for_prompt),
                    message: PublicPromptContent::Agent {
                        sender: creator_for_prompt,
                        text: prompt.try_into().map_err(|_| {
                            ConversationClientError::InvalidInput("invalid approval fixture prompt")
                        })?,
                    },
                    effort: None,
                    generation: None,
                },
                Duration::from_secs(120),
                prompt_cancellation,
            )
            .await
    });
    let request_id =
        wait_for_pending_request(proof, &requester, creator, approver, &mut prompt_task).await?;
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
            decision: None,
            option_id: Some("deny-once".to_owned()),
            note: None,
            acknowledge_persistent: false,
            actor: Identity::Session {
                session: serde_json::from_value(serde_json::to_value(approver)?)?,
            },
        })
        .await?;
    match tokio::time::timeout(Duration::from_secs(90), prompt_task).await {
        Ok(Ok(Ok(ConversationOperationResult::Completed { .. }))) => Ok(request_id),
        Ok(Ok(Ok(_))) => Err("Scripted provider prompt did not complete".into()),
        Ok(Ok(Err(error))) => Err(error.into()),
        Ok(Err(join_error)) => Err(join_error.into()),
        Err(elapsed) => Err(elapsed.into()),
    }
}

async fn wait_for_pending_request(
    proof: &mut ProofContext,
    requester: &SessionRef,
    creator: &SessionRef,
    approver: &SessionRef,
    prompt_task: &mut tokio::task::JoinHandle<
        Result<ConversationOperationResult, ConversationClientError>,
    >,
) -> ProofResult<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut poll = tokio::time::interval(Duration::from_millis(500));
    let provider_target = serde_json::to_value(requester)?;
    loop {
        poll.tick().await;
        if prompt_task.is_finished() {
            let result = prompt_task.await?;
            return Err(format!("Provider prompt ended before approval: {result:?}").into());
        }
        let approvals = proof.client.list_pending_approvals(true).await?;
        if let Some(approval) = approvals.approvals.iter().find(|record| {
            record.requester == *creator
                && record.approver == *approver
                && record.operation.get("target") == Some(&provider_target)
        }) {
            proof.record("approvalPending", json!({"requestId":approval.request_id}))?;
            return Ok(approval.request_id.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Scripted provider approval did not become pending".into());
        }
    }
}
