//! Opt-in proof that an agent can recover from the combined-argument CLI diagnostic.
#[allow(dead_code)]
#[path = "automation_live_support/proof_context.rs"]
mod proof_context;

use communication_protocol::{MessageContent, MessageDelivery, NativeSendParams};
use project_board::*;
use proof_context::{ProofContext, ProofResult};
use serde_json::{Value, json};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, time::Duration};

const PROJECT_NAME: &str = "DX Diagnostic recovery project";
const REPOSITORY_ORIGIN: &str = "github.com/shravansunder/codex-router";

#[tokio::test]
#[ignore = "requires an explicitly launched isolated debug Host and one fresh Luna skill session"]
async fn luna_recovers_from_combined_board_argument_diagnostic() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let skill_path = copy_current_skill(&proof.workspace)?;
    let setup_actor = Identity::Human {
        human_id: HumanId::try_from("dx-recovery-fixture".to_owned())?,
    };
    let project_id = ProjectId::generate();
    proof
        .client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from(PROJECT_NAME.to_owned())?,
            description: Description::try_from(
                "Existing project for CLI diagnostic recovery".to_owned(),
            )?,
            actor: setup_actor.clone(),
            acting_for: None,
        })
        .await?;
    proof
        .client
        .board_repository_attach(RepositoryAttachRequest {
            project_id,
            repository: RepositoryRef::Origin {
                normalized_origin: NormalizedOrigin::try_from(REPOSITORY_ORIGIN.to_owned())?,
            },
            actor: setup_actor,
            acting_for: None,
        })
        .await?;
    let agent = proof
        .start_skill_thread("the board CLI diagnostic recovery evaluator")
        .await?;
    let context_path = proof.workspace.join("recovery-context.json");
    write_private_json(
        &context_path,
        &json!({
            "skillPath":skill_path,
            "agentSessionsExecutable":env!("CARGO_BIN_EXE_agent-sessions"),
            "selectedDebugServiceDirectory":proof.service_directory,
            "selfSessionRef":agent,
            "repositoryOrigin":REPOSITORY_ORIGIN,
            "projectName":PROJECT_NAME,
        }),
    )?;
    let diagnostic = "The board command was passed as one argument. Pass each word as a separate argument, for example: agent-sessions board project list --json";
    let task = format!(
        "Use the communication skill and factual context at {}. Your earlier attempt to discover the project produced this diagnostic: `{diagnostic}`. Recover from that error and inspect the existing project named {PROJECT_NAME}, discovering it through repository origin {REPOSITORY_ORIGIN}. Do not create or modify resources. Explain what you found and finish with DX_RECOVERY_COMPLETE.",
        context_path.display()
    );
    proof
        .client
        .send_agent_message(NativeSendParams {
            target: agent.clone(),
            generation: proof.generation.clone(),
            message: MessageContent::Agent {
                sender: agent.clone(),
                text: task.try_into()?,
            },
            delivery: MessageDelivery::Auto,
            client_user_message_id: None,
        })
        .await?;
    let turns = wait_for_terminal_turn(&mut proof, &agent).await?;
    write_private_json(
        &proof.root.join("recovery-session-trace.json"),
        &json!({"session":agent,"turns":turns}),
    )?;
    let successful_project_list = turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("commandExecution")
                && item.get("exitCode").and_then(Value::as_i64) == Some(0)
        })
        .any(|item| {
            let command = item.get("command").map(all_strings).unwrap_or_default();
            let output = item
                .get("aggregatedOutput")
                .map(all_strings)
                .unwrap_or_default();
            command.contains(" board project list ")
                && output.contains(&PROJECT_NAME.to_lowercase())
        });
    if !successful_project_list {
        return Err("Luna did not recover to a successful board project list containing the existing project".into());
    }
    proof.client.close().await?;
    Ok(())
}

async fn wait_for_terminal_turn(
    proof: &mut ProofContext,
    agent: &communication_protocol::SessionRef,
) -> ProofResult<Vec<Value>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let turns = proof.turns(agent).await?;
        if turns.iter().any(|turn| {
            crate::proof_context::agent_text(turn).contains("DX_RECOVERY_COMPLETE")
                || matches!(
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

fn copy_current_skill(workspace: &Path) -> ProofResult<std::path::PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../agent-skills/agent-communication")
        .canonicalize()?;
    let destination = workspace.join("agent-communication-skill");
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

fn all_strings(value: &Value) -> String {
    match value {
        Value::String(text) => format!(" {text} "),
        Value::Array(values) => values.iter().map(all_strings).collect(),
        Value::Object(values) => values.values().map(all_strings).collect(),
        _ => String::new(),
    }
    .to_lowercase()
}
