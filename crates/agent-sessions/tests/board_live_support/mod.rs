//! Real two-agent board collaboration through the public CLI and SDK.
use crate::proof_context::{ProofContext, ProofResult, shell_quote};
use communication_protocol::{MessageContent, MessageDelivery, NativeSendParams, SessionRef};
use project_board::{
    ActingForIdentity, BoardCreateRequest, BoardId, Description, HumanId, Identity,
    MessageListRequest, MessageListScope, MessageReferences, MessageSelection, MessageShowRequest,
    PageRequest, Placement, ProjectCreateRequest, ProjectId, ReferenceTarget, ResourceName,
    SessionRef as BoardSessionRef, ThreadShowRequest, TopicCreateRequest, TopicId,
};
use serde_json::{Value, json};

pub async fn exercise() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let alpha = proof.start_thread("Luna board researcher").await?;
    let beta = proof.start_thread("Luna board verifier").await?;
    let alpha_identity = board_identity(&alpha)?;
    let beta_identity = board_identity(&beta)?;
    let owner = ActingForIdentity::Human {
        human_id: HumanId::try_from("board-debug-acceptance-owner".to_owned())?,
    };

    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    let root_message_id = project_board::MessageId::generate();
    let contribution_message_id = project_board::MessageId::generate();
    let marker_id = root_message_id.as_str().replace('-', "");
    let finding_marker = format!("BOARD_FINDING_{marker_id}");
    let result_marker = format!("BOARD_RESULT_{marker_id}");
    let alpha_posted_marker = format!("ALPHA_POSTED_{marker_id}");
    let beta_posted_marker = format!("BETA_POSTED_{marker_id}");
    let alpha_read_marker = format!("ALPHA_READ_{marker_id}");

    let project = proof
        .client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from(format!("Debug project {marker_id}"))?,
            description: Description::try_from("Disposable live board acceptance".to_owned())?,
            actor: alpha_identity.clone(),
            acting_for: Some(owner.clone()),
        })
        .await?;
    let board = proof
        .client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id: project_id.clone(),
            name: ResourceName::try_from(format!("Debug board {marker_id}"))?,
            description: Description::try_from("Two-agent CLI collaboration".to_owned())?,
            actor: alpha_identity.clone(),
            acting_for: Some(owner.clone()),
        })
        .await?;
    let topic = proof
        .client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id: board_id.clone(),
            name: ResourceName::try_from("Verification".to_owned())?,
            description: Description::try_from("Findings checked by another agent".to_owned())?,
            actor: alpha_identity.clone(),
            acting_for: Some(owner),
        })
        .await?;
    proof.record(
        "boardResourcesCreated",
        json!({"project":project,"board":board,"topic":topic}),
    )?;

    let cli = shell_quote(env!("CARGO_BIN_EXE_agent-sessions"));
    let service_directory = shell_quote(&proof.service_directory.to_string_lossy());
    let alpha_actor = shell_quote(&serde_json::to_string(&alpha_identity)?);
    let beta_actor = shell_quote(&serde_json::to_string(&beta_identity)?);
    let finding_text = format!(
        "{finding_marker}: take the integer 37, multiply it by 3, then add 5. Post the decimal result in this root thread."
    );
    let finding_file = proof.workspace.join("board-finding.txt");
    std::fs::write(&finding_file, &finding_text)?;
    let alpha_post_command = format!(
        "{cli} board message post --message-id {} --placement topic --topic-id {} --text-file {} --actor {alpha_actor} --service-directory {service_directory} --json",
        root_message_id.as_str(),
        topic_id.as_str(),
        shell_quote(&finding_file.to_string_lossy()),
    );
    send_task(
        &mut proof,
        &alpha,
        &format!(
            "Execute this exact CLI command once:\n{alpha_post_command}\nIf it succeeds, output exactly {alpha_posted_marker}. If it fails, report the exact error and do not retry, resend, or use another communication path."
        ),
        "alphaBoardTaskAccepted",
    )
    .await?;
    let alpha_post_turns = proof.wait_for_text(&alpha, &alpha_posted_marker).await?;
    require_successful_cli_output(&alpha_post_turns, &finding_marker)?;

    let beta_read_command = format!(
        "{cli} board message show --message-id {} --service-directory {service_directory} --json",
        root_message_id.as_str(),
    );
    let beta_post_prefix = format!(
        "{cli} board message post --message-id {} --placement thread --root-message-id {} --reference-message {} --actor {beta_actor} --service-directory {service_directory} --json --text",
        contribution_message_id.as_str(),
        root_message_id.as_str(),
        root_message_id.as_str(),
    );
    send_task(
        &mut proof,
        &beta,
        &format!(
            "Run this read-only CLI command exactly once and inspect its returned message text:\n{beta_read_command}\nThe text contains an arithmetic instruction. Compute it yourself. Then run one CLI post command beginning exactly with:\n{beta_post_prefix}\nSupply one final quoted text argument whose entire value is `{result_marker}: ` followed by only the decimal result. Do not post unless the read command showed {finding_marker}. After the post succeeds, output exactly {beta_posted_marker}. If either command fails, report its exact error and do not retry or resend."
        ),
        "betaBoardTaskAccepted",
    )
    .await?;
    let beta_turns = proof.wait_for_text(&beta, &beta_posted_marker).await?;
    require_successful_cli_output(&beta_turns, &finding_marker)?;
    require_successful_cli_output(&beta_turns, &format!("{result_marker}: 116"))?;

    let alpha_read_command = format!(
        "{cli} board message list --scope thread --root-message-id {} --selection latest --service-directory {service_directory} --json",
        root_message_id.as_str(),
    );
    send_task(
        &mut proof,
        &alpha,
        &format!(
            "Execute this exact read-only CLI command once:\n{alpha_read_command}\nOnly if its result contains `{result_marker}: 116`, output exactly {alpha_read_marker}. Otherwise report what the command returned. Do not retry."
        ),
        "alphaThreadReadTaskAccepted",
    )
    .await?;
    let alpha_read_turns = proof.wait_for_text(&alpha, &alpha_read_marker).await?;
    require_successful_cli_output(&alpha_read_turns, &format!("{result_marker}: 116"))?;

    let root = proof
        .client
        .board_message_show(MessageShowRequest {
            message_id: root_message_id.clone(),
        })
        .await?;
    let contribution = proof
        .client
        .board_message_show(MessageShowRequest {
            message_id: contribution_message_id.clone(),
        })
        .await?;
    let thread_history = proof
        .client
        .board_message_list(MessageListRequest {
            scope: MessageListScope::Thread {
                root_message_id: root_message_id.clone(),
            },
            selection: MessageSelection::Latest,
            page: PageRequest::default(),
        })
        .await?;
    let alpha_thread = proof
        .client
        .board_thread_show(ThreadShowRequest {
            root_message_id: root_message_id.clone(),
            reader: Some(alpha_identity.clone()),
        })
        .await?;
    let beta_thread = proof
        .client
        .board_thread_show(ThreadShowRequest {
            root_message_id: root_message_id.clone(),
            reader: Some(beta_identity.clone()),
        })
        .await?;

    verify_sdk_observations(
        &root.message,
        &contribution.message,
        &thread_history.page.records,
        &alpha_identity,
        &beta_identity,
        &finding_text,
        &format!("{result_marker}: 116"),
        &root_message_id,
    )?;
    if alpha_thread
        .watch_status
        .as_ref()
        .is_none_or(|status| !status.watching)
        || beta_thread
            .watch_status
            .as_ref()
            .is_none_or(|status| !status.watching)
    {
        return Err("automatic thread watches were not visible for both posting agents".into());
    }
    proof.record(
        "boardAgentExchangeVerified",
        json!({
            "root":root,
            "contribution":contribution,
            "threadHistory":thread_history,
            "alphaThread":alpha_thread,
            "betaThread":beta_thread,
        }),
    )?;
    proof.client.close().await?;
    Ok(())
}

async fn send_task(
    proof: &mut ProofContext,
    target: &SessionRef,
    task: &str,
    event: &str,
) -> ProofResult<()> {
    let receipt = proof
        .client
        .send_agent_message(NativeSendParams {
            target: target.clone(),
            generation: proof.generation.clone(),
            message: MessageContent::Agent {
                sender: target.clone(),
                text: task.to_owned().try_into()?,
            },
            delivery: MessageDelivery::Auto,
            client_user_message_id: None,
        })
        .await?;
    proof.record(event, json!(receipt))?;
    Ok(())
}

fn board_identity(session: &SessionRef) -> ProofResult<Identity> {
    let session: BoardSessionRef = serde_json::from_value(serde_json::to_value(session)?)?;
    Ok(Identity::Session { session })
}

fn require_successful_cli_output(turns: &[Value], marker: &str) -> ProofResult<()> {
    let observed = turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("commandExecution"))
        .filter(|item| item.get("exitCode").and_then(Value::as_i64) == Some(0))
        .filter_map(|item| item.get("aggregatedOutput").and_then(Value::as_str))
        .any(|output| output.contains(marker));
    if !observed {
        return Err(format!(
            "agent emitted its completion marker without successful CLI output containing {marker}"
        )
        .into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_sdk_observations(
    root: &project_board::Message,
    contribution: &project_board::Message,
    thread_records: &[project_board::Message],
    alpha_identity: &Identity,
    beta_identity: &Identity,
    finding_text: &str,
    result_text: &str,
    root_message_id: &project_board::MessageId,
) -> ProofResult<()> {
    if root.actor != *alpha_identity
        || root.text.as_str() != finding_text
        || contribution.actor != *beta_identity
        || contribution.text.as_str() != result_text
        || contribution.placement
            != (Placement::Thread {
                root_message_id: root_message_id.clone(),
            })
        || contribution.references
            != MessageReferences::try_from(vec![ReferenceTarget::Message {
                message_id: root_message_id.clone(),
            }])?
        || thread_records.len() != 1
        || thread_records.first() != Some(contribution)
    {
        return Err(
            "public SDK reads did not reproduce the two agents' exact board exchange".into(),
        );
    }
    Ok(())
}
