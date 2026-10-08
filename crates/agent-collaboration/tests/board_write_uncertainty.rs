//! A lost API response after transmission preserves inspect-before-retry evidence.
use serde_json::{Value, json};

mod fake_api_support;
use fake_api_support::{FakeCollaborationApi, FakeReply};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const HUMAN_ACTOR: &str = r#"{"kind":"human","humanId":"uncertainty-proof"}"#;
type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn supplied_create_id_survives_response_loss_without_automatic_replay() -> TestResult<()> {
    let supplied_project_id = collaboration_client::board::ProjectId::generate();
    let proof = run_lost_project_create(Some(supplied_project_id.as_str()), true).await?;

    if proof.transmitted_project_id != supplied_project_id.as_str() {
        return Err("CLI transmitted a different caller-supplied project ID".into());
    }
    validate_uncertain_project(&proof.output, supplied_project_id.as_str())?;
    if proof.second_connection_accepted {
        return Err("CLI reconnected after the transmitted write lost its response".into());
    }
    Ok(())
}

#[tokio::test]
async fn generated_create_id_is_reported_after_response_loss_without_automatic_replay()
-> TestResult<()> {
    let proof = run_lost_project_create(None, true).await?;
    let _generated_id =
        collaboration_client::board::ProjectId::try_from(proof.transmitted_project_id.clone())?;

    validate_uncertain_project(&proof.output, &proof.transmitted_project_id)?;
    if proof.second_connection_accepted {
        return Err("CLI reconnected after the transmitted write lost its response".into());
    }
    Ok(())
}

#[tokio::test]
async fn human_output_uses_stable_uncertainty_kind_action_and_resource() -> TestResult<()> {
    let project_id = collaboration_client::board::ProjectId::generate();
    let proof = run_lost_project_create(Some(project_id.as_str()), false).await?;
    if proof.output.status.code() != Some(5) || !proof.output.stdout.is_empty() {
        return Err("human uncertainty output used the wrong stream or exit status".into());
    }
    let stderr = String::from_utf8(proof.output.stderr)?;
    for expected in [
        "Error: outcomeUnknown",
        "Next action: inspectResource",
        project_id.as_str(),
    ] {
        if !stderr.contains(expected) {
            return Err(format!("human uncertainty output omitted {expected}").into());
        }
    }
    if proof.second_connection_accepted {
        return Err("CLI reconnected after the transmitted write lost its response".into());
    }
    Ok(())
}

#[tokio::test]
async fn human_output_uses_stable_known_rejection_kind_and_action() -> TestResult<()> {
    let output = run_known_project_rejection().await?;
    if output.status.code() != Some(4) || !output.stdout.is_empty() {
        return Err("human rejection output used the wrong stream or exit status".into());
    }
    let stderr = String::from_utf8(output.stderr)?;
    for expected in [
        "Error: nameConflict",
        "Next action: selectDifferentName",
        "Choose another project name.",
    ] {
        if !stderr.contains(expected) {
            return Err(format!("human rejection output omitted {expected}").into());
        }
    }
    Ok(())
}

struct LostWriteProof {
    output: std::process::Output,
    transmitted_project_id: String,
    second_connection_accepted: bool,
}

async fn run_lost_project_create(
    project_id: Option<&str>,
    machine_output: bool,
) -> TestResult<LostWriteProof> {
    let mut fixture = FakeCollaborationApi::new(SERVICE_ID, SERVICE_EPOCH)?;
    // The stand-in reads the complete write, then closes without answering: only the
    // response is uncertain. It keeps listening briefly to prove the CLI does not replay it.
    let served = fixture.serve_then_watch(
        vec![FakeReply::Disconnect],
        std::time::Duration::from_millis(500),
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command.args([
        "board",
        "project",
        "create",
        "--name",
        "Uncertain write proof",
        "--actor",
        HUMAN_ACTOR,
        "--service-directory",
    ]);
    command.arg(fixture.directory());
    if machine_output {
        command.arg("--json");
    }
    if let Some(project_id) = project_id {
        command.args(["--project-id", project_id]);
    }
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(3), command.output()).await??;
    let watched = served.await??;
    let write = watched.calls.first().ok_or("the CLI made no write")?;
    assert_eq!(
        write.get("tool").and_then(Value::as_str),
        Some("board_project_create")
    );
    assert_eq!(
        write.pointer("/arguments/name").and_then(Value::as_str),
        Some("Uncertain write proof")
    );
    assert_eq!(
        write
            .pointer("/arguments/actor/kind")
            .and_then(Value::as_str),
        Some("human")
    );
    let transmitted_project_id = write
        .pointer("/arguments/projectId")
        .and_then(Value::as_str)
        .ok_or("write omitted projectId")?
        .to_owned();
    Ok(LostWriteProof {
        output,
        transmitted_project_id,
        second_connection_accepted: watched.further_call.is_some(),
    })
}

async fn run_known_project_rejection() -> TestResult<std::process::Output> {
    let mut fixture = FakeCollaborationApi::new(SERVICE_ID, SERVICE_EPOCH)?;
    let served = fixture.serve(vec![FakeReply::Error(json!({
        "code":-32050,"message":"Board operation rejected","data":{
            "kind":"nameConflict","stage":"admission",
            "message":"Choose another project name.","nextAction":"selectDifferentName",
            "details":{"kind":"none"}}
    }))]);
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "board",
            "project",
            "create",
            "--name",
            "Known rejection",
            "--actor",
            HUMAN_ACTOR,
            "--service-directory",
        ])
        .arg(fixture.directory())
        .output()
        .await?;
    served.await??;
    Ok(output)
}

fn validate_uncertain_project(
    output: &std::process::Output,
    expected_project_id: &str,
) -> TestResult<()> {
    let response: Value = serde_json::from_slice(&output.stdout)?;
    let valid = output.status.code() == Some(5)
        && output.stderr.is_empty()
        && response.get("kind").and_then(Value::as_str) == Some("error")
        && response.pointer("/error/kind").and_then(Value::as_str) == Some("outcomeUnknown")
        && response.pointer("/error/stage").and_then(Value::as_str) == Some("inspection")
        && response
            .pointer("/error/nextAction")
            .and_then(Value::as_str)
            == Some("inspectResource")
        && response
            .pointer("/error/details/kind")
            .and_then(Value::as_str)
            == Some("resource")
        && response
            .pointer("/error/details/resource/kind")
            .and_then(Value::as_str)
            == Some("project")
        && response
            .pointer("/error/details/resource/projectId")
            .and_then(Value::as_str)
            == Some(expected_project_id);
    if !valid {
        return Err(format!(
            "CLI omitted inspect-before-retry evidence: status {:?}, response {response}",
            output.status.code()
        )
        .into());
    }
    Ok(())
}
