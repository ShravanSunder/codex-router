//! First-fire wait output is distinct from durable creation and native acceptance.
use communication_client::{ControlClient, WakeWaitError};
use communication_protocol::{WakeShowRequest, WakeupId};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::Path,
};
pub(crate) async fn wait(directory: &Path, wakeup_id: WakeupId, machine: bool) -> i32 {
    let result = async {
        let client = ControlClient::connect(
            directory,
            "agent-sessions-wake-wait",
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
        client
            .subscribe_wakeup(WakeShowRequest {
                wakeup_id: wakeup_id.clone(),
            })
            .await?
            .wait_until_first_fire()
            .await
    }
    .await;
    let (record, code) = match result {
        Ok(fire) => (json!({"kind":"result","result":fire}), 0),
        Err(error) => {
            let (kind, next_action, first_fire) = match &error {
                WakeWaitError::Paused { .. } => ("wakePaused", "resumeWakeup", "notRecorded"),
                WakeWaitError::Cancelled { .. } => ("wakeCancelled", "createWakeup", "notRecorded"),
                WakeWaitError::Expired { .. } => ("wakeExpired", "createWakeup", "notRecorded"),
                WakeWaitError::FinishedWithoutFiring { .. } => {
                    ("wakeFinishedWithoutFiring", "createWakeup", "notRecorded")
                }
                WakeWaitError::NotFound { .. } => {
                    ("wakeNotFound", "verifyWakeupAddress", "unknown")
                }
                _ => ("waitUnavailable", "reconnectWait", "unknown"),
            };
            (
                json!({"kind":"error","error":{"kind":kind,"stage":"waitForFirstFire","message":error.to_string(),"wakeupId":wakeup_id,"firstOccurrenceId":null,"nextAction":next_action,"effects":{"firstFire":first_fire}}}),
                if first_fire == "unknown" { 3 } else { 4 },
            )
        }
    };
    print(record, machine, code)
}
fn print(record: Value, machine: bool, code: i32) -> i32 {
    let text = if machine {
        serde_json::to_string(&record)
    } else {
        serde_json::to_string_pretty(&record)
    };
    match text {
        Ok(text) if writeln!(io::stdout(), "{text}").is_ok() => code,
        _ => 5,
    }
}
