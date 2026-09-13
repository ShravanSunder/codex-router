mod durable_grading;
mod trace_evidence;

use crate::proof_context::{ProofContext, ProofResult};
use collaboration_client::board::*;
use collaboration_client::protocol::{
    MessageContent, MessageDelivery, NativeSendParams, SessionRef,
};
use serde_json::{Value, json};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, time::Duration};
use trace_evidence::{
    OperatorTrace, observed_coverage, require_complete_operation_coverage, write_session_trace,
};

pub(super) const PRIMARY_PROJECT: &str = "DX Router migration program";
pub(super) const PRIMARY_BOARD: &str = "DX Release engineering";
pub(super) const PRIMARY_TOPIC: &str = "DX Migration readiness";
pub(super) const SECONDARY_PROJECT: &str = "DX Runtime compatibility";
pub(super) const SECONDARY_BOARD: &str = "DX Compatibility review";
pub(super) const SECONDARY_TOPIC: &str = "DX Cross-project evidence";
pub(super) const REPOSITORY_ORIGIN: &str = "github.com/shravansunder/codex-router";
pub(super) const OWNER_HUMAN_ID: &str = "board-skill-dx-owner";

pub async fn exercise() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let skill_path = copy_current_skill(&proof.workspace)?;
    let fixture_path = proof.workspace.join("migration-readiness-evidence.txt");
    std::fs::write(
        &fixture_path,
        "Migration readiness evidence\nValidated batches: 17 and 25\nThe requested readiness total is their sum.\n",
    )?;
    let mut traces = Vec::new();

    let setup = proof
        .start_skill_thread("the disposable board setup and metadata operator")
        .await?;
    let setup_context = write_task_context(
        &proof,
        "setup-context.json",
        &skill_path,
        &fixture_path,
        &setup,
    )?;
    let setup_task = format!(
        "Use the communication skill and factual environment context at {}. You are explicitly authorized to create disposable test resources for this task. Create the {PRIMARY_PROJECT} project, associate it with repository origin {REPOSITORY_ORIGIN}, and prove both repository-to-project and project-to-repository discovery. Exercise project inspection, listing, and a description update while retaining its assigned name. Within it create {PRIMARY_BOARD}, inspect/list it, and update its description while retaining its assigned name. Create {PRIMARY_TOPIC}, inspect it through topic listing, and update its description while retaining its assigned name. Preserve all returned identities for later operators in a private workspace note. Do not create anything outside these named disposable resources. Finish with DX_SETUP_COMPLETE.",
        setup_context.display()
    );
    let setup_turns = run_operator(&mut proof, &setup, &setup_task, "DX_SETUP_COMPLETE").await?;
    traces.push(OperatorTrace::new("setup", setup.clone(), setup_turns));
    write_session_trace(&proof.root, &traces, &observed_coverage(&traces))?;
    let resources = durable_grading::discover_named_resources(&mut proof).await?;
    proof
        .client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic {
                topic_id: resources.primary_topic.topic_id.clone(),
            },
            actor: human_identity("dx-pagination-seed")?,
            acting_for: None,
            text: MessageText::try_from(
                "Earlier migration context retained for pagination.".to_owned(),
            )?,
            references: Vec::new().try_into()?,
        })
        .await?;

    let discussion = proof
        .start_skill_thread("the migration discussion and history operator")
        .await?;
    let discussion_context = write_task_context(
        &proof,
        "discussion-context.json",
        &skill_path,
        &fixture_path,
        &discussion,
    )?;
    let discussion_task = format!(
        "Use the communication skill and factual environment context at {}. Work only in the named disposable resources. In {PRIMARY_TOPIC}, create a top-level Migration readiness discussion that points readers to the fixture, inspect the saved message, then immediately attempt another top-level post as the same actor so the documented cooldown behavior is observed without evasion. Read the topic with a one-record page and continue its pagination if offered. You are explicitly authorized to create the {SECONDARY_PROJECT} project with {SECONDARY_BOARD} and {SECONDARY_TOPIC}. In that second topic create a main discussion, then post a thread conclusion containing the fixture total while acting for {OWNER_HUMAN_ID}; reference both a message and a thread from the primary project. Inspect the message and read message history across topic, thread, board, project, and all-project scopes, exercising latest, after-position, and explicit range modes. Use resource search to locate the primary topic by its name, and message search to find your conclusion inside its thread. Verify that searching the other topic does not include that conclusion merely because you watch its thread. Do not use resource UUIDs from the evaluator; discover resources by names and repository association. Finish with DX_DISCUSSION_COMPLETE.",
        discussion_context.display()
    );
    let discussion_turns = run_operator(
        &mut proof,
        &discussion,
        &discussion_task,
        "DX_DISCUSSION_COMPLETE",
    )
    .await?;
    traces.push(OperatorTrace::new(
        "discussion",
        discussion.clone(),
        discussion_turns,
    ));
    write_session_trace(&proof.root, &traces, &observed_coverage(&traces))?;
    let discussion_identity = board_identity(&discussion)?;
    let discussions =
        durable_grading::grade_discussion(&mut proof, &resources, &discussion_identity).await?;

    let inbox = proof
        .start_skill_thread("the watched-thread inbox and lifecycle-state operator")
        .await?;
    let inbox_identity = board_identity(&inbox)?;
    proof
        .client
        .board_thread_watch(ThreadWatchRequest {
            root_message_id: discussions.primary_root.clone(),
            actor: inbox_identity.clone(),
            acting_for: None,
        })
        .await?;
    proof
        .client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: discussions.primary_root.clone(),
            },
            actor: human_identity("dx-seed-reviewer")?,
            acting_for: None,
            text: MessageText::try_from("Seed reviewer activity for inbox catch-up.".to_owned())?,
            references: vec![ReferenceTarget::Message {
                message_id: discussions.cross_project_reply.clone(),
            }]
            .try_into()?,
        })
        .await?;
    let inbox_context = write_task_context(
        &proof,
        "inbox-context.json",
        &skill_path,
        &fixture_path,
        &inbox,
    )?;
    let inbox_task = format!(
        "Use the communication skill and factual environment context at {}. Find the Migration readiness root in {PRIMARY_TOPIC}. Inspect the thread and project thread inventory, including watched-only discovery. Exercise unwatch and watch while ending with the watch active. Catch up on the {PRIMARY_PROJECT} inbox, inspect tracked-project summaries including unread-only results, use any earlier-unwatched history guidance, process the seeded thread activity, and acknowledge only its exact thread scope and activity position. Fetch again to establish that processed activity is no longer unread. Do not post or create resources. Finish with DX_INBOX_COMPLETE.",
        inbox_context.display()
    );
    let inbox_turns = run_operator(&mut proof, &inbox, &inbox_task, "DX_INBOX_COMPLETE").await?;
    traces.push(OperatorTrace::new("inbox", inbox.clone(), inbox_turns));
    write_session_trace(&proof.root, &traces, &observed_coverage(&traces))?;
    durable_grading::grade_inbox(&mut proof, &resources, &discussions, &inbox_identity).await?;

    let lifecycle = proof
        .start_skill_thread("the board lifecycle and cleanup operator")
        .await?;
    let lifecycle_context = write_task_context(
        &proof,
        "lifecycle-context.json",
        &skill_path,
        &fixture_path,
        &lifecycle,
    )?;
    let lifecycle_task = format!(
        "Use the communication skill and factual environment context at {}. In the primary Migration readiness thread, resolve it, verify that a thread post is rejected while resolved, then mark it unresolved. Detach {REPOSITORY_ORIGIN} from {PRIMARY_PROJECT} and verify repository discovery reflects the detachment without losing discussion history. Archive {PRIMARY_BOARD}, verify it appears when archived boards are included, and verify new content is rejected while its existing messages remain readable. Do not delete or create replacement resources. Finish with DX_LIFECYCLE_COMPLETE.",
        lifecycle_context.display()
    );
    let lifecycle_turns = run_operator(
        &mut proof,
        &lifecycle,
        &lifecycle_task,
        "DX_LIFECYCLE_COMPLETE",
    )
    .await?;
    traces.push(OperatorTrace::new(
        "lifecycle",
        lifecycle.clone(),
        lifecycle_turns,
    ));
    write_session_trace(&proof.root, &traces, &observed_coverage(&traces))?;
    durable_grading::grade_lifecycle(&mut proof, &resources, &discussions).await?;

    let coverage = require_complete_operation_coverage(&traces)?;
    write_session_trace(&proof.root, &traces, &coverage)?;
    proof.record(
        "boardSkillDxComplete",
        json!({"trace":"session-trace.json","operators":4,"coveredOperations":coverage.len()}),
    )?;
    proof.client.close().await?;
    Ok(())
}

async fn run_operator(
    proof: &mut ProofContext,
    target: &SessionRef,
    task: &str,
    completion_marker: &str,
) -> ProofResult<Vec<Value>> {
    proof
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let turns = proof.turns(target).await?;
        if turns
            .iter()
            .any(|turn| crate::proof_context::agent_text(turn).contains(completion_marker))
        {
            return Ok(turns);
        }
        if turns.iter().any(|turn| {
            matches!(
                turn.get("status").and_then(Value::as_str),
                Some("completed" | "failed" | "interrupted")
            )
        }) {
            return Ok(turns);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(turns);
        }
    }
}

fn write_task_context(
    proof: &ProofContext,
    filename: &str,
    skill_path: &Path,
    fixture_path: &Path,
    session: &SessionRef,
) -> ProofResult<std::path::PathBuf> {
    let path = proof.workspace.join(filename);
    let value = json!({
        "skillPath":skill_path,"agentCollaborationExecutable":env!("CARGO_BIN_EXE_agent-collaboration"),
        "selectedDebugServiceDirectory":proof.service_directory,"selfSessionRef":session,
        "repositoryOrigin":REPOSITORY_ORIGIN,"primaryProjectName":PRIMARY_PROJECT,
        "primaryBoardName":PRIMARY_BOARD,"primaryTopicName":PRIMARY_TOPIC,
        "secondaryProjectName":SECONDARY_PROJECT,"secondaryBoardName":SECONDARY_BOARD,
        "secondaryTopicName":SECONDARY_TOPIC,"fixturePath":fixture_path,
        "actingFor":{"kind":"human","humanId":OWNER_HUMAN_ID}
    });
    write_private_json(&path, &value)?;
    Ok(path)
}

fn write_private_json(path: &Path, value: &Value) -> ProofResult<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    writeln!(file)?;
    Ok(())
}

fn copy_current_skill(workspace: &Path) -> ProofResult<std::path::PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../agent-skills/agent-collaboration")
        .canonicalize()?;
    let destination = workspace.join("agent-collaboration-skill");
    copy_directory(&source, &destination)?;
    Ok(destination.join("SKILL.md"))
}

fn copy_directory(source: &Path, destination: &Path) -> ProofResult<()> {
    std::fs::create_dir(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &destination_path)?;
        } else {
            std::fs::copy(entry.path(), destination_path)?;
        }
    }
    Ok(())
}

pub(super) fn board_identity(session: &SessionRef) -> ProofResult<Identity> {
    let session = serde_json::from_value(serde_json::to_value(session)?)?;
    Ok(Identity::Session { session })
}

pub(super) fn human_identity(value: &str) -> ProofResult<Identity> {
    Ok(Identity::Human {
        human_id: HumanId::try_from(value.to_owned())?,
    })
}
