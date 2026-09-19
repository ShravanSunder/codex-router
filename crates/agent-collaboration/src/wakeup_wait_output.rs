//! First-fire wait output is distinct from durable creation and native acceptance.
use collaboration_client::protocol::{WakeShowRequest, WakeupId};
use collaboration_client::{ControlClient, WakeWaitFailureKind};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::Path,
};
pub(crate) async fn wait(directory: &Path, wakeup_id: WakeupId, machine: bool) -> i32 {
    let result = async {
        let client = ControlClient::connect(
            directory,
            "agent-collaboration-wake-wait",
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
        Ok(fire) => (crate::endpoint_commands::result_envelope(json!(fire)), 0),
        Err(error) => {
            let failure = error.into_operation_failure();
            let code = if matches!(
                failure.kind,
                WakeWaitFailureKind::Unavailable
                    | WakeWaitFailureKind::NotFound
                    | WakeWaitFailureKind::Connection
            ) {
                3
            } else {
                4
            };
            (json!({"kind":"error","error":failure}), code)
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
