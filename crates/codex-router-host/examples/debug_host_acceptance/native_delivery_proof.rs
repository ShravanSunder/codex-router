//! Real CLI queue, active steering and exact interruption against fresh Luna threads.
use super::{
    cli_event_listener::{CliEventListener, run_json_command},
    owned_thread_registry::OwnedThreadRegistry,
};
use codex_native_integration::NativeProtocolConnection;
use communication_client::ControlClient;
use serde_json::{Value, json};
use std::{error::Error, path::Path};

pub struct DeliveryProofRequest<'a> {
    pub executable: &'a Path,
    pub directory: &'a Path,
    pub cwd: &'a Path,
}
pub async fn run_delivery_proof(
    native: &mut NativeProtocolConnection,
    owned: &mut OwnedThreadRegistry,
    request: DeliveryProofRequest<'_>,
) -> Result<(), Box<dyn Error>> {
    let control = ControlClient::connect(
        request.directory,
        "native-delivery-proof",
        env!("CARGO_PKG_VERSION"),
    )
    .await?;
    let service = control.identity().service_id.clone();
    let first = owned.create(native, request.cwd).await?;
    let second = owned.create(native, request.cwd).await?;
    // Materialize before the public listener explicitly attaches by native resume.
    for thread in [&first, &second] {
        let receipt = owned
            .submit_text(
                native,
                thread,
                "Do not use tools. Reply exactly DELIVERY_READY.",
            )
            .await?;
        if owned.observe_text(native, receipt).await?.trim() != "DELIVERY_READY" {
            return Err("delivery preparation mismatch".into());
        }
        println!(
            "{}",
            json!({"kind":"ownedDeliveryThreadCreated","threadId":thread,"model":"gpt-5.6-luna"})
        );
    }
    let sender =
        json!({"endpoint":{"serviceId":service,"endpointId":"codex-local"},"sessionId":second});
    let target =
        json!({"endpoint":{"serviceId":service,"endpointId":"codex-local"},"sessionId":first});
    let mut listener =
        CliEventListener::attach(request.executable, request.directory, &first).await?;
    let cli = DeliveryCommand {
        executable: request.executable,
        directory: request.directory,
        sender: &sender,
        target: &target,
        generation: listener.generation().clone(),
    };
    let started = cli.send("auto", "Do not use tools. Print a numbered list from 1 through 10000, one number per line, until interrupted. Do not stop early.").await?;
    let turn = required_text(&started, "/acceptance/turnId")?.to_owned();
    listener
        .wait_for(|event| is_turn_event(event, "item/agentMessage/delta", &turn))
        .await?;
    let queued = cli
        .send(
            "queue",
            "Do not use tools. Reply exactly BUSY_QUEUE_RESULT.",
        )
        .await?;
    let queue_id = required_text(&queued, "/acceptance/submissionId")?.to_owned();
    assert_acceptance(&queued, "queueAccepted")?;
    require_queued(native, &first, &queue_id).await?;
    require_active_turn(native, &first, &turn).await?;
    let steered = cli
        .send(
            "steer",
            "Continue the numbered list through 10000 without tools; do not stop early.",
        )
        .await?;
    assert_acceptance(&steered, "steerAccepted")?;
    if required_text(&steered, "/acceptance/turnId")? != turn {
        return Err("steer changed turn identity".into());
    }
    let automatic = cli
        .send(
            "auto",
            "Continue the same long numbered list without tools until interrupted.",
        )
        .await?;
    assert_acceptance(&automatic, "steerAccepted")?;
    if required_text(&automatic, "/acceptance/turnId")? != turn {
        return Err("auto did not steer exact active turn".into());
    }
    let interruption = run_json_command(
        request.executable,
        &[
            "turn".into(),
            "interrupt".into(),
            "--endpoint".into(),
            "codex-local".into(),
            "--session".into(),
            first.clone(),
            "--turn".into(),
            turn.clone(),
            "--service-directory".into(),
            request.directory.to_string_lossy().into_owned(),
            "--json".into(),
        ],
    )
    .await?;
    if interruption.get("kind").and_then(Value::as_str) != Some("interruptCompleted") {
        return Err("exact interrupt receipt missing".into());
    }
    let terminal = listener
        .wait_for(|event| {
            event.get("method").and_then(Value::as_str) == Some("turn/completed")
                && event.pointer("/params/turn/id").and_then(Value::as_str) == Some(turn.as_str())
        })
        .await?;
    if terminal
        .pointer("/params/turn/status")
        .and_then(Value::as_str)
        != Some("interrupted")
    {
        return Err("native turn was not observed interrupted".into());
    }
    require_queued(native, &first, &queue_id).await?;
    let interrupted_queue = cli
        .send(
            "queue",
            "Do not use tools. Reply exactly INTERRUPTED_QUEUE_RESULT.",
        )
        .await?;
    assert_acceptance(&interrupted_queue, "queueAccepted")?;
    let interrupted_id = required_text(&interrupted_queue, "/acceptance/submissionId")?.to_owned();
    require_queued(native, &first, &interrupted_id).await?;
    let snapshot = native.inspect_thread(&first).await?;
    if snapshot.pointer("/status/type").and_then(Value::as_str) == Some("active") {
        return Err("queued input unexpectedly activated interrupted work".into());
    }
    // Explicit test cleanup of precisely acknowledged proof items, after proving
    // interruption preserved them. This is not service-side compensation or replay.
    for id in [&queue_id, &interrupted_id] {
        native
            .request(
                "thread/queue/delete",
                json!({"threadId":first,"queuedSubmissionId":id}),
            )
            .await?;
    }
    listener.close().await?;
    println!(
        "{}",
        json!({"kind":"busyDeliveryPassed","threadId":first,"turnId":turn,"queueId":queue_id,"interruptedQueueId":interrupted_id,"exactSteer":true,"autoSteer":true,"interrupted":true,"queuedItemsCleaned":true})
    );

    let mut idle_listener =
        CliEventListener::attach(request.executable, request.directory, &second).await?;
    let idle_cli = DeliveryCommand {
        executable: request.executable,
        directory: request.directory,
        sender: &target,
        target: &sender,
        generation: idle_listener.generation().clone(),
    };
    let idle_queue = idle_cli
        .send(
            "queue",
            "Do not use tools. Reply with exactly IDLE_QUEUE_RESULT.",
        )
        .await?;
    assert_acceptance(&idle_queue, "queueAccepted")?;
    let completed = idle_listener
        .wait_for(|event| {
            event.get("method").and_then(Value::as_str) == Some("item/completed")
                && event.pointer("/params/item/type").and_then(Value::as_str)
                    == Some("agentMessage")
                && event
                    .pointer("/params/item/text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.trim() == "IDLE_QUEUE_RESULT")
        })
        .await?;
    let idle_turn = required_text(&completed, "/params/turnId")?.to_owned();
    let terminal = idle_listener
        .wait_for(|event| {
            event.get("method").and_then(Value::as_str) == Some("turn/completed")
                && event.pointer("/params/turn/id").and_then(Value::as_str)
                    == Some(idle_turn.as_str())
        })
        .await?;
    if terminal
        .pointer("/params/turn/status")
        .and_then(Value::as_str)
        != Some("completed")
    {
        return Err("idle queued turn did not complete".into());
    }
    idle_listener.close().await?;
    control.close().await?;
    println!(
        "{}",
        json!({"kind":"idleQueuePassed","threadId":second,"turnId":idle_turn,"acceptance":idle_queue.get("acceptance"),"result":"IDLE_QUEUE_RESULT"})
    );
    Ok(())
}

struct DeliveryCommand<'a> {
    executable: &'a Path,
    directory: &'a Path,
    sender: &'a Value,
    target: &'a Value,
    generation: Value,
}
impl DeliveryCommand<'_> {
    async fn send(&self, delivery: &str, text: &str) -> Result<Value, Box<dyn Error>> {
        let result = run_json_command(
            self.executable,
            &[
                "message".into(),
                "send".into(),
                "--from".into(),
                self.sender.to_string(),
                "--to".into(),
                self.target.to_string(),
                "--delivery".into(),
                delivery.into(),
                "--text".into(),
                text.into(),
                "--service-directory".into(),
                self.directory.to_string_lossy().into_owned(),
                "--expected-service-epoch".into(),
                required_text(&self.generation, "/serviceEpoch")?.to_owned(),
                "--expected-generation".into(),
                self.generation
                    .get("generation")
                    .and_then(Value::as_u64)
                    .ok_or("missing generation number")?
                    .to_string(),
                "--json".into(),
            ],
        )
        .await?;
        if result.get("target") != Some(self.target)
            || result.get("generation") != Some(&self.generation)
        {
            return Err("delivery receipt scope changed".into());
        }
        Ok(result)
    }
}
fn required_text<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, Box<dyn Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| "required proof receipt field missing".into())
}
fn assert_acceptance(value: &Value, expected: &str) -> Result<(), Box<dyn Error>> {
    if required_text(value, "/acceptance/kind")? != expected {
        return Err("unexpected native acceptance kind".into());
    }
    Ok(())
}
fn is_turn_event(event: &Value, method: &str, turn: &str) -> bool {
    event.get("method").and_then(Value::as_str) == Some(method)
        && event.pointer("/params/turnId").and_then(Value::as_str) == Some(turn)
}
async fn require_queued(
    native: &mut NativeProtocolConnection,
    thread: &str,
    id: &str,
) -> Result<(), Box<dyn Error>> {
    let listed = native
        .request("thread/queue/list", json!({"threadId":thread,"limit":100}))
        .await?;
    if !listed
        .get("data")
        .and_then(Value::as_array)
        .is_some_and(|rows| {
            rows.iter()
                .any(|row| row.get("id").and_then(Value::as_str) == Some(id))
        })
    {
        return Err("acknowledged proof queue item not observed pending".into());
    }
    Ok(())
}
async fn require_active_turn(
    native: &mut NativeProtocolConnection,
    thread: &str,
    turn: &str,
) -> Result<(), Box<dyn Error>> {
    let result = native
        .request(
            "thread/read",
            json!({"threadId":thread,"includeTurns":true}),
        )
        .await?;
    if !result
        .pointer("/thread/turns")
        .and_then(Value::as_array)
        .is_some_and(|turns| {
            turns.iter().any(|entry| {
                entry.get("id").and_then(Value::as_str) == Some(turn)
                    && entry.get("status").and_then(Value::as_str) == Some("inProgress")
            })
        })
    {
        return Err("busy proof target no longer has its original active turn".into());
    }
    Ok(())
}
