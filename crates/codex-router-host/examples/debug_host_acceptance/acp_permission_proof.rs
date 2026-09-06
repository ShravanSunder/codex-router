//! Real ACP cancellation correlated with the native command's declined outcome.
use super::owned_thread_registry::OwnedThreadRegistry;
use codex_native_integration::NativeProtocolConnection;
use serde_json::{Value, json};
use std::{
    error::Error,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};
use tokio::process::Command;

pub struct PermissionProofRequest<'a> {
    pub directory: &'a Path,
    pub sdk: &'a Path,
    pub cwd: &'a Path,
    pub competing_responder: bool,
}

pub async fn run_permission_proof(
    native: &mut NativeProtocolConnection,
    owned: &mut OwnedThreadRegistry,
    request: PermissionProofRequest<'_>,
) -> Result<(), Box<dyn Error>> {
    let PermissionProofRequest {
        directory,
        sdk,
        cwd,
        competing_responder,
    } = request;
    let target = owned.create_with_user_review(native, cwd).await?;
    println!(
        "{}",
        json!({"kind":"ownedPermissionThreadCreated","threadId":target,"model":"gpt-5.6-luna","approvalsReviewer":"user","approvalPolicy":"untrusted"})
    );
    let preparation = owned
        .submit_text(
            native,
            &target,
            "Do not use tools. Reply exactly READY_FOR_PERMISSION.",
        )
        .await?;
    if owned.observe_text(native, preparation).await?.trim() != "READY_FOR_PERMISSION" {
        return Err("permission preparation failed".into());
    }
    // The unique executable avoids reusing a cached approval for another command.
    // Even if accidentally executed, it can only create this run's private marker.
    let proof_root = cwd
        .join("tmp")
        .join(format!("permission-proof-{}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&proof_root)?;
    let marker = proof_root.join("execution-marker");
    let executable = proof_root.join("permission-command");
    let marker_quoted = marker
        .to_str()
        .ok_or("non-UTF8 marker path")?
        .replace('\'', "'\\''");
    let mut fixture = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&executable)?;
    writeln!(
        fixture,
        "#!/bin/sh\nprintf '%s\\n' 'unexpected-execution' > '{marker_quoted}'"
    )?;
    drop(fixture);
    owned.require_owned(&target)?;
    let mut command = Command::new("node");
    command
        .kill_on_drop(true)
        .arg(cwd.join("scripts/proof-tools/acp-permission-proof.mjs"))
        .arg(sdk)
        .arg(directory.join("codex-acp.sock"))
        .arg(&target)
        .arg(cwd)
        .arg(&executable);
    let observation = observe_declined_command(native, &target, &executable, competing_responder);
    let (output, item_id) = tokio::time::timeout(Duration::from_secs(120), async {
        tokio::try_join!(
            async { Ok::<_, Box<dyn Error>>(command.output().await?) },
            observation
        )
    })
    .await??;
    if !output.status.success() {
        return Err("independent ACP permission client failed; no replay".into());
    }
    let receipt: Value = serde_json::from_slice(&output.stdout)?;
    if receipt.get("kind").and_then(Value::as_str) != Some("independentAcpPermissionCancelled")
        || receipt.get("sessionId").and_then(Value::as_str) != Some(target.as_str())
        || receipt.get("toolCallId").and_then(Value::as_str) != Some(item_id.as_str())
        || marker.exists()
    {
        return Err(
            "ACP cancellation did not match native declined command and absent marker".into(),
        );
    }
    println!(
        "{}",
        json!({"kind":"ownedAcpPermissionPassed","threadId":target,"itemId":item_id,"nativeStatus":"declined","markerAbsent":true,"competingResponseSubmitted":competing_responder,"winner":"notClaimed","clientReceipt":receipt})
    );
    Ok(())
}

async fn observe_declined_command(
    native: &mut NativeProtocolConnection,
    target: &str,
    executable: &Path,
    competing_responder: bool,
) -> Result<String, Box<dyn Error>> {
    let mut submitted_item = None;
    loop {
        let event = native.next_message().await?;
        let Some(params) = event.get("params") else {
            continue;
        };
        if params.get("threadId").and_then(Value::as_str) != Some(target) {
            continue;
        }
        if competing_responder
            && event.get("method").and_then(Value::as_str)
                == Some("item/commandExecution/requestApproval")
        {
            let command = params
                .get("command")
                .and_then(Value::as_str)
                .ok_or("native callback omitted command")?;
            if !command.contains(executable.to_str().ok_or("non-UTF8 executable")?)
                || submitted_item.is_some()
            {
                return Err("competing callback did not name the one owned fixture".into());
            }
            if let Some(Value::Array(decisions)) = params.get("availableDecisions")
                && !decisions.contains(&json!("cancel"))
            {
                return Err("native callback does not offer cancel".into());
            }
            submitted_item = Some(
                params
                    .get("itemId")
                    .and_then(Value::as_str)
                    .ok_or("native callback omitted item identity")?
                    .to_owned(),
            );
            native
                .submit_callback_response(
                    event
                        .get("id")
                        .ok_or("native callback omitted request ID")?
                        .clone(),
                    json!({"decision":"cancel"}),
                )
                .await?;
        }
        // Single-responder mode observes only; competing mode submits cancel without
        // interpreting socket-write success as acknowledgement that its decision won.
        if event.get("method").and_then(Value::as_str) == Some("item/completed")
            && params.pointer("/item/type").and_then(Value::as_str) == Some("commandExecution")
        {
            if params.pointer("/item/status").and_then(Value::as_str) != Some("declined") {
                return Err("native permission command was not declined".into());
            }
            if competing_responder
                && submitted_item.as_deref() != params.pointer("/item/id").and_then(Value::as_str)
            {
                return Err(
                    "competing response was not submitted for the completed command".into(),
                );
            }
            return params
                .pointer("/item/id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "native declined item lacked identity".into());
        }
    }
}
