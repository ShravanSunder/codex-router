use super::*;
use crate::fake_api_support::{FakeCollaborationApi, FakeReply};

#[tokio::test]
async fn malformed_wait_result_uses_machine_uncertainty_output() -> TestResult {
    const ROOT_MESSAGE_ID: &str = "01900000-0000-7000-8000-000000000031";

    let mut fixture = FakeCollaborationApi::new(SERVICE_ID, SERVICE_EPOCH)?;
    // The wait answers with a result the CLI cannot decode; no other call may follow it.
    let fixture_task = fixture.serve_then_watch(
        vec![FakeReply::Result(
            serde_json::json!({"batch":{"kind":"invalid"}}),
        )],
        Duration::from_secs(3),
    );

    let output = run_cli(
        fixture.directory(),
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
    let mut watched = fixture_task.await??;
    ensure(
        watched.further_call.is_none(),
        "CLI sent another call after the malformed wait result",
    )?;
    let request = watched.calls.pop().ok_or("the CLI made no tool call")?;

    ensure(
        request.get("tool").and_then(Value::as_str) == Some("board_thread_wait"),
        "CLI did not call board_thread_wait",
    )?;
    ensure(
        request
            .pointer("/arguments/actor/kind")
            .and_then(Value::as_str)
            == Some("session"),
        "wait request did not resolve the self actor",
    )?;
    ensure(
        request
            .pointer("/arguments/actor/session/sessionId")
            .and_then(Value::as_str)
            == Some(SESSION_ID),
        "wait request used a different session actor",
    )?;
    ensure(
        request
            .pointer("/arguments/filter/kind")
            .and_then(Value::as_str)
            == Some("roots"),
        "wait request did not retain the selected root filter",
    )?;
    ensure(
        request
            .pointer("/arguments/filter/rootMessageIds/0")
            .and_then(Value::as_str)
            == Some(ROOT_MESSAGE_ID),
        "wait request used a different root filter",
    )?;
    ensure(
        request
            .pointer("/arguments/maxWaitSeconds")
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
    Ok(())
}
