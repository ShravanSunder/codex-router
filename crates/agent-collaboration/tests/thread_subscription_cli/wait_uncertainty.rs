use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[tokio::test]
async fn malformed_wait_result_uses_machine_uncertainty_output() -> TestResult {
    const ROOT_MESSAGE_ID: &str = "01900000-0000-7000-8000-000000000031";

    let directory = tempfile::tempdir_in("/tmp")?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let listener = UnixListener::bind(directory.path().join("control.sock"))?;
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(serde_json::json!({
        "version":2,
        "serviceId":SERVICE_ID,
        "serviceEpoch":SERVICE_EPOCH,
        "machineLabel":"fixture-host",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"},
    }))?;
    let publication = ManifestPublication::publish(directory.path(), &manifest)?;
    let fixture = tokio::spawn(serve_malformed_wait_result(listener, digest));

    let output = run_cli(
        directory.path(),
        &[
            "board",
            "thread",
            "wait",
            "--root-message-id",
            ROOT_MESSAGE_ID,
            "--actor",
            "self",
            "--max-wait",
            "1s",
        ],
    )
    .await?;
    let request = fixture.await??;

    ensure(
        request.get("method").and_then(Value::as_str) == Some("board/threadWait"),
        "CLI did not call board/threadWait",
    )?;
    ensure(
        request
            .pointer("/params/actor/kind")
            .and_then(Value::as_str)
            == Some("session"),
        "wait request did not resolve the self actor",
    )?;
    ensure(
        request
            .pointer("/params/actor/session/sessionId")
            .and_then(Value::as_str)
            == Some(SESSION_ID),
        "wait request used a different session actor",
    )?;
    ensure(
        request
            .pointer("/params/filter/kind")
            .and_then(Value::as_str)
            == Some("roots"),
        "wait request did not retain the selected root filter",
    )?;
    ensure(
        request
            .pointer("/params/filter/rootMessageIds/0")
            .and_then(Value::as_str)
            == Some(ROOT_MESSAGE_ID),
        "wait request used a different root filter",
    )?;
    ensure(
        request
            .pointer("/params/maxWaitSeconds")
            .and_then(Value::as_u64)
            == Some(1),
        "wait request used a different deadline",
    )?;

    ensure(
        output.status.code() == Some(5),
        "unknown wait result did not use exit status 5",
    )?;
    ensure(
        output.stderr.is_empty(),
        "machine error was written to stderr",
    )?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    ensure(
        response["kind"] == "error"
            && response["error"]["kind"] == "outcomeUnknown"
            && response["error"]["stage"] == "response"
            && response["error"]["effect"] == "unknown"
            && response["error"]["nextAction"] == "inspectResource",
        "CLI omitted the typed unknown-outcome fields",
    )?;
    ensure(
        response["error"]["actor"]["session"]["sessionId"] == SESSION_ID,
        "CLI omitted the actor needed to inspect the uncertain wait",
    )?;
    ensure(
        response["error"]["filter"]["rootMessageIds"][0] == ROOT_MESSAGE_ID,
        "CLI omitted the filter needed to inspect the uncertain wait",
    )?;
    drop(publication);
    Ok(())
}

async fn serve_malformed_wait_result(
    listener: UnixListener,
    digest: String,
) -> Result<Value, TestError> {
    let (stream, _) = listener.accept().await?;
    let mut stream = BufReader::new(stream);
    let initialize = read_control_request(&mut stream).await?;
    if initialize.get("method").and_then(Value::as_str) != Some("control/initialize") {
        return Err("CLI did not initialize its Control connection".into());
    }
    let initialize_id = initialize
        .get("id")
        .cloned()
        .ok_or("initialize request omitted its ID")?;
    let initialized = serde_json::json!({
        "jsonrpc":"2.0",
        "id":initialize_id,
        "result":{
            "version":{"major":1,"minor":0},
            "serviceId":SERVICE_ID,
            "serviceEpoch":SERVICE_EPOCH,
            "controlSchemaDigest":digest,
        }
    });
    stream
        .get_mut()
        .write_all(format!("{initialized}\n").as_bytes())
        .await?;

    let wait = read_control_request(&mut stream).await?;
    let wait_id = wait
        .get("id")
        .cloned()
        .ok_or("wait request omitted its ID")?;
    let malformed = serde_json::json!({
        "jsonrpc":"2.0",
        "id":wait_id,
        "result":{"batch":{"kind":"invalid"}},
    });
    stream
        .get_mut()
        .write_all(format!("{malformed}\n").as_bytes())
        .await?;

    let mut unexpected_request = String::new();
    let bytes_read = tokio::time::timeout(
        Duration::from_secs(3),
        stream.read_line(&mut unexpected_request),
    )
    .await??;
    if bytes_read != 0 {
        return Err("CLI sent another Control request after the malformed wait result".into());
    }
    Ok(wait)
}

async fn read_control_request(
    stream: &mut BufReader<tokio::net::UnixStream>,
) -> Result<Value, TestError> {
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
}
