use super::board_cli_runner::{
    require_record_message_ids, result, run_board_cli, run_rejected_board_cli,
};
use crate::proof_context::{ProofContext, ProofResult};
use project_board::{Identity, Message, MessageId};
use serde_json::{Value, json};
use std::{io::Write, os::unix::fs::OpenOptionsExt};

pub(super) struct AgentExchange<'a> {
    pub alpha_identity: &'a Identity,
    pub beta_identity: &'a Identity,
    pub first_project_id: &'a str,
    pub first_board_id: &'a str,
    pub first_topic_id: &'a str,
    pub root_message: &'a Message,
    pub contribution_message: &'a Message,
}

pub(super) async fn exercise_after_agent_exchange(
    proof: &mut ProofContext,
    exchange: AgentExchange<'_>,
) -> ProofResult<()> {
    let AgentExchange {
        alpha_identity,
        beta_identity,
        first_project_id,
        first_board_id,
        first_topic_id,
        root_message,
        contribution_message,
    } = exchange;
    let alpha_actor = serde_json::to_string(alpha_identity)?;
    let beta_actor = serde_json::to_string(beta_identity)?;

    let alpha_inbox = run_board_cli(
        &proof.service_directory,
        [
            "inbox",
            "fetch",
            "--project-id",
            first_project_id,
            "--reader",
            &alpha_actor,
        ],
    )
    .await?;
    let contribution_sequence = contribution_message.activity_sequence.get();
    let inbox_records = result(&alpha_inbox, "/page/records")?
        .as_array()
        .ok_or("alpha inbox records were not an array")?;
    if !inbox_records.iter().any(|record| {
        record.pointer("/message/messageId").and_then(Value::as_str)
            == Some(contribution_message.message_id.as_str())
            && record.get("activitySequence").and_then(Value::as_u64) == Some(contribution_sequence)
    }) {
        return Err("alpha inbox omitted beta's watched-thread contribution".into());
    }
    let contribution_sequence_text = contribution_sequence.to_string();
    run_board_cli(
        &proof.service_directory,
        [
            "inbox",
            "acknowledge",
            "--scope",
            "thread",
            "--root-message-id",
            root_message.message_id.as_str(),
            "--through-activity-sequence",
            &contribution_sequence_text,
            "--actor",
            &alpha_actor,
        ],
    )
    .await?;
    let acknowledged_inbox = run_board_cli(
        &proof.service_directory,
        [
            "inbox",
            "fetch",
            "--project-id",
            first_project_id,
            "--reader",
            &alpha_actor,
        ],
    )
    .await?;
    if !result(&acknowledged_inbox, "/page/records")?
        .as_array()
        .is_some_and(Vec::is_empty)
    {
        return Err("acknowledged alpha inbox still returned unread activity".into());
    }

    run_board_cli(
        &proof.service_directory,
        [
            "thread",
            "unwatch",
            "--root-message-id",
            root_message.message_id.as_str(),
            "--actor",
            &beta_actor,
        ],
    )
    .await?;
    run_board_cli(
        &proof.service_directory,
        [
            "thread",
            "watch",
            "--root-message-id",
            root_message.message_id.as_str(),
            "--actor",
            &beta_actor,
        ],
    )
    .await?;
    run_board_cli(
        &proof.service_directory,
        [
            "thread",
            "resolve",
            "--root-message-id",
            root_message.message_id.as_str(),
            "--actor",
            &alpha_actor,
        ],
    )
    .await?;
    run_rejected_board_cli(
        &proof.service_directory,
        "threadResolved",
        [
            "message",
            "post",
            "--message-id",
            MessageId::generate().as_str(),
            "--placement",
            "thread",
            "--root-message-id",
            root_message.message_id.as_str(),
            "--text",
            "must be rejected while resolved",
            "--actor",
            &beta_actor,
        ],
    )
    .await?;
    run_board_cli(
        &proof.service_directory,
        [
            "thread",
            "unresolve",
            "--root-message-id",
            root_message.message_id.as_str(),
            "--actor",
            &alpha_actor,
        ],
    )
    .await?;

    let second_project_id = project_board::ProjectId::generate();
    let second_board_id = project_board::BoardId::generate();
    let second_topic_id = project_board::TopicId::generate();
    let cross_project_message_id = MessageId::generate();
    run_board_cli(
        &proof.service_directory,
        [
            "project",
            "create",
            "--project-id",
            second_project_id.as_str(),
            "--name",
            "Live cross-project proof",
            "--description",
            "Disposable second project",
            "--actor",
            &beta_actor,
        ],
    )
    .await?;
    run_board_cli(
        &proof.service_directory,
        [
            "create",
            "--board-id",
            second_board_id.as_str(),
            "--project-id",
            second_project_id.as_str(),
            "--name",
            "Cross-project board",
            "--actor",
            &beta_actor,
        ],
    )
    .await?;
    run_board_cli(
        &proof.service_directory,
        [
            "topic",
            "create",
            "--topic-id",
            second_topic_id.as_str(),
            "--board-id",
            second_board_id.as_str(),
            "--name",
            "Cross references",
            "--actor",
            &beta_actor,
        ],
    )
    .await?;
    let cross_project_post = run_board_cli(
        &proof.service_directory,
        [
            "message",
            "post",
            "--message-id",
            cross_project_message_id.as_str(),
            "--placement",
            "topic",
            "--topic-id",
            second_topic_id.as_str(),
            "--text",
            "Cross-project reference proof",
            "--reference-message",
            contribution_message.message_id.as_str(),
            "--reference-thread",
            root_message.message_id.as_str(),
            "--actor",
            &beta_actor,
        ],
    )
    .await?;
    let cross_project_sequence = result(&cross_project_post, "/message/activitySequence")?
        .as_u64()
        .ok_or("cross-project post omitted activity sequence")?;
    let project_inventory = run_board_cli(
        &proof.service_directory,
        ["project", "list", "--limit", "100"],
    )
    .await?;
    let project_records = result(&project_inventory, "/page/records")?
        .as_array()
        .ok_or("project inventory records were not an array")?;
    for expected_project_id in [first_project_id, second_project_id.as_str()] {
        if !project_records.iter().any(|project| {
            project.get("projectId").and_then(Value::as_str) == Some(expected_project_id)
        }) {
            return Err(format!("CLI project inventory omitted {expected_project_id}").into());
        }
    }
    let range_upper = cross_project_sequence.to_string();
    let all_project_history = run_board_cli(
        &proof.service_directory,
        [
            "message",
            "list",
            "--scope",
            "all-projects",
            "--selection",
            "range",
            "--from-activity-sequence",
            "0",
            "--to-activity-sequence",
            &range_upper,
        ],
    )
    .await?;
    require_record_message_ids(
        &all_project_history,
        &[
            root_message.message_id.as_str(),
            cross_project_message_id.as_str(),
        ],
    )?;

    run_board_cli(
        &proof.service_directory,
        [
            "archive",
            "--board-id",
            first_board_id,
            "--actor",
            &alpha_actor,
        ],
    )
    .await?;
    run_rejected_board_cli(
        &proof.service_directory,
        "archivedBoard",
        [
            "message",
            "post",
            "--message-id",
            MessageId::generate().as_str(),
            "--placement",
            "topic",
            "--topic-id",
            first_topic_id,
            "--text",
            "must be rejected after archive",
            "--actor",
            &alpha_actor,
        ],
    )
    .await?;

    let marker: Value =
        serde_json::from_slice(&std::fs::read(proof.root.join("debug-host-context.json"))?)?;
    let host_pid = marker
        .get("hostPid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0)
        .ok_or("debug Host marker omitted its PID")?;
    let persistence_state = json!({
        "hostPid": host_pid,
        "firstProjectId": first_project_id,
        "firstBoardId": first_board_id,
        "firstTopicId": first_topic_id,
        "secondProjectId": second_project_id,
        "secondBoardId": second_board_id,
        "secondTopicId": second_topic_id,
        "rootMessageId": root_message.message_id,
        "contributionMessageId": contribution_message.message_id,
        "crossProjectMessageId": cross_project_message_id,
        "rangeUpperActivitySequence": cross_project_sequence,
        "alphaIdentity": alpha_identity,
        "betaIdentity": beta_identity,
    });
    let mut state_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(proof.root.join("board-persistence-state.json"))?;
    writeln!(state_file, "{persistence_state}")?;
    proof.record(
        "boardFullCliJourneyReadyForHostRestart",
        json!({
            "firstProjectId":first_project_id,
            "secondProjectId":second_project_id,
            "rangeUpperActivitySequence":cross_project_sequence,
        }),
    )?;
    Ok(())
}
