//! A lost Control response after transmission preserves inspect-before-retry evidence.
use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const HUMAN_ACTOR: &str = r#"{"kind":"human","humanId":"uncertainty-proof"}"#;
type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn supplied_create_id_survives_response_loss_without_automatic_replay() -> TestResult<()> {
    let supplied_project_id = project_board::ProjectId::generate();
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
    let _generated_id = project_board::ProjectId::try_from(proof.transmitted_project_id.clone())?;

    validate_uncertain_project(&proof.output, &proof.transmitted_project_id)?;
    if proof.second_connection_accepted {
        return Err("CLI reconnected after the transmitted write lost its response".into());
    }
    Ok(())
}

#[tokio::test]
async fn human_output_uses_stable_uncertainty_kind_action_and_resource() -> TestResult<()> {
    let project_id = project_board::ProjectId::generate();
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
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "board-write-uncertainty-{}",
        project_board::ProjectId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let listener = tokio::net::UnixListener::bind(root.join("control.sock"))?;
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(json!({
        "version":1,
        "serviceId":SERVICE_ID,
        "serviceEpoch":SERVICE_EPOCH,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
    }))?;
    let publication = communication_service::ManifestPublication::publish(&root, &manifest)?;
    let fixture_digest = digest.clone();
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut stream = BufReader::new(stream);
        let initialize = read_request(&mut stream).await?;
        assert_eq!(
            initialize.get("method").and_then(Value::as_str),
            Some("control/initialize")
        );
        let request_id = initialize
            .get("id")
            .cloned()
            .ok_or("initialize ID missing")?;
        let response = json!({
            "jsonrpc":"2.0",
            "id":request_id,
            "result":{
                "version":{"major":1,"minor":0},
                "serviceId":SERVICE_ID,
                "serviceEpoch":SERVICE_EPOCH,
                "controlSchemaDigest":fixture_digest,
            }
        });
        stream
            .get_mut()
            .write_all(format!("{response}\n").as_bytes())
            .await?;

        let write = read_request(&mut stream).await?;
        assert_eq!(
            write.get("method").and_then(Value::as_str),
            Some("board/projectCreate")
        );
        assert_eq!(
            write.pointer("/params/name").and_then(Value::as_str),
            Some("Uncertain write proof")
        );
        assert_eq!(
            write.pointer("/params/actor/kind").and_then(Value::as_str),
            Some("human")
        );
        let transmitted_project_id = write
            .pointer("/params/projectId")
            .and_then(Value::as_str)
            .ok_or("write omitted projectId")?
            .to_owned();

        // The peer has consumed the complete write frame. Closing now makes only the response
        // uncertain. Keep listening briefly to prove the CLI does not reconnect and replay it.
        drop(stream);
        let second_connection_accepted = matches!(
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept(),).await,
            Ok(Ok(_))
        );
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
            transmitted_project_id,
            second_connection_accepted,
        ))
    });

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"));
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
    command.arg(&root);
    if machine_output {
        command.arg("--json");
    }
    if let Some(project_id) = project_id {
        command.args(["--project-id", project_id]);
    }
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(3), command.output()).await??;
    let (transmitted_project_id, second_connection_accepted) = fixture.await??;

    drop(publication);
    std::fs::remove_file(root.join("control.sock"))?;
    std::fs::remove_dir(root)?;
    Ok(LostWriteProof {
        output,
        transmitted_project_id,
        second_connection_accepted,
    })
}

async fn read_request(stream: &mut BufReader<tokio::net::UnixStream>) -> TestResult<Value> {
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
}

async fn run_known_project_rejection() -> TestResult<std::process::Output> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "board-known-rejection-{}",
        project_board::ProjectId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let listener = tokio::net::UnixListener::bind(root.join("control.sock"))?;
    let digest = format!("sha256:{}", "b".repeat(64));
    let manifest = serde_json::from_value(json!({
        "version":1,"serviceId":SERVICE_ID,"serviceEpoch":SERVICE_EPOCH,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
    }))?;
    let publication = communication_service::ManifestPublication::publish(&root, &manifest)?;
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut stream = BufReader::new(stream);
        let initialize = read_request(&mut stream).await?;
        let initialize_id = initialize
            .get("id")
            .cloned()
            .ok_or("initialize ID missing")?;
        let initialized = json!({"jsonrpc":"2.0","id":initialize_id,"result":{
            "version":{"major":1,"minor":0},"serviceId":SERVICE_ID,
            "serviceEpoch":SERVICE_EPOCH,"controlSchemaDigest":digest}});
        stream
            .get_mut()
            .write_all(format!("{initialized}\n").as_bytes())
            .await?;
        let write = read_request(&mut stream).await?;
        let write_id = write.get("id").cloned().ok_or("write ID missing")?;
        let rejected = json!({"jsonrpc":"2.0","id":write_id,"error":{
            "code":-32050,"message":"Board operation rejected","data":{
                "kind":"nameConflict","stage":"admission",
                "message":"Choose another project name.","nextAction":"selectDifferentName",
                "details":{"kind":"none"}}}});
        stream
            .get_mut()
            .write_all(format!("{rejected}\n").as_bytes())
            .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
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
        .arg(&root)
        .output()
        .await?;
    fixture.await??;
    drop(publication);
    std::fs::remove_file(root.join("control.sock"))?;
    std::fs::remove_dir(root)?;
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
