use super::board_cli_runner::{require_record_message_ids, result, run_board_cli};
use crate::proof_context::ProofResult;
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

pub(super) async fn verify() -> ProofResult<()> {
    let root =
        PathBuf::from(std::env::var_os("CODEX_AUTOMATION_PROOF_ROOT").ok_or(
            "Set CODEX_AUTOMATION_PROOF_ROOT to the resumed automation-debug-host directory",
        )?)
        .canonicalize()?;
    if root.parent() != Some(std::path::Path::new("/tmp").canonicalize()?.as_path())
        || std::fs::metadata(&root)?.permissions().mode() & 0o077 != 0
    {
        return Err("Persistence proof requires the same private direct child of /tmp".into());
    }
    let state: Value =
        serde_json::from_slice(&std::fs::read(root.join("board-persistence-state.json"))?)?;
    let marker: Value =
        serde_json::from_slice(&std::fs::read(root.join("debug-host-context.json"))?)?;
    validate_restarted_host(&root, &state, &marker).await?;

    let service_directory = root.join("agent-communication");
    let first_project_id = required_string(&state, "firstProjectId")?;
    let first_board_id = required_string(&state, "firstBoardId")?;
    let second_project_id = required_string(&state, "secondProjectId")?;
    let second_board_id = required_string(&state, "secondBoardId")?;
    let root_message_id = required_string(&state, "rootMessageId")?;
    let contribution_message_id = required_string(&state, "contributionMessageId")?;
    let cross_project_message_id = required_string(&state, "crossProjectMessageId")?;
    let range_upper = state
        .get("rangeUpperActivitySequence")
        .and_then(Value::as_u64)
        .ok_or("saved range upper activity sequence missing")?
        .to_string();
    let alpha_identity = serde_json::to_string(
        state
            .get("alphaIdentity")
            .ok_or("saved alpha identity missing")?,
    )?;
    let beta_identity = serde_json::to_string(
        state
            .get("betaIdentity")
            .ok_or("saved beta identity missing")?,
    )?;

    for project_id in [first_project_id, second_project_id] {
        let shown = run_board_cli(
            &service_directory,
            ["project", "show", "--project-id", project_id],
        )
        .await?;
        if result(&shown, "/project/projectId")?.as_str() != Some(project_id) {
            return Err(
                format!("resumed Host returned a different project for {project_id}").into(),
            );
        }
    }
    let archived_board =
        run_board_cli(&service_directory, ["show", "--board-id", first_board_id]).await?;
    if result(&archived_board, "/board/state")?.as_str() != Some("archived") {
        return Err("archived board state did not survive Host restart".into());
    }
    let active_board =
        run_board_cli(&service_directory, ["show", "--board-id", second_board_id]).await?;
    if result(&active_board, "/board/state")?.as_str() != Some("active") {
        return Err("second project's active board did not survive Host restart".into());
    }
    for message_id in [
        root_message_id,
        contribution_message_id,
        cross_project_message_id,
    ] {
        let shown = run_board_cli(
            &service_directory,
            ["message", "show", "--message-id", message_id],
        )
        .await?;
        if result(&shown, "/message/messageId")?.as_str() != Some(message_id) {
            return Err(
                format!("resumed Host returned a different message for {message_id}").into(),
            );
        }
    }
    let cross_project_message = run_board_cli(
        &service_directory,
        ["message", "show", "--message-id", cross_project_message_id],
    )
    .await?;
    let references = result(&cross_project_message, "/message/references")?
        .as_array()
        .ok_or("cross-project references were not an array")?;
    if !references.iter().any(|reference| {
        reference.get("messageId").and_then(Value::as_str) == Some(contribution_message_id)
    }) || !references.iter().any(|reference| {
        reference.get("rootMessageId").and_then(Value::as_str) == Some(root_message_id)
    }) {
        return Err("cross-project references did not survive Host restart".into());
    }

    let history = run_board_cli(
        &service_directory,
        [
            "message",
            "list",
            "--scope",
            "all-projects",
            "--selection",
            "range",
            "--from-activity-sequence",
            "0",
            "--to-activity-sequence",
            &range_upper,
        ],
    )
    .await?;
    require_record_message_ids(&history, &[root_message_id, cross_project_message_id])?;
    let watched_thread = run_board_cli(
        &service_directory,
        [
            "thread",
            "show",
            "--root-message-id",
            root_message_id,
            "--reader",
            &beta_identity,
        ],
    )
    .await?;
    if result(&watched_thread, "/watchStatus/watching")?.as_bool() != Some(true) {
        return Err("explicit thread watch did not survive Host restart".into());
    }
    let alpha_inbox = run_board_cli(
        &service_directory,
        [
            "inbox",
            "fetch",
            "--project-id",
            first_project_id,
            "--reader",
            &alpha_identity,
        ],
    )
    .await?;
    if !result(&alpha_inbox, "/page/records")?
        .as_array()
        .is_some_and(Vec::is_empty)
    {
        return Err("acknowledged inbox state did not survive Host restart".into());
    }
    println!("Board persistence verified after owned isolated Host restart");
    Ok(())
}

async fn validate_restarted_host(
    root: &std::path::Path,
    state: &Value,
    marker: &Value,
) -> ProofResult<()> {
    let marker_run_directory = marker
        .get("runDirectory")
        .and_then(Value::as_str)
        .ok_or("resumed Host marker omitted its run directory")?;
    if marker.get("model").and_then(Value::as_str) != Some("gpt-5.6-luna")
        || marker.get("profile").and_then(Value::as_str) != Some("codex-router-debug")
        || marker
            .get("port")
            .and_then(Value::as_u64)
            .is_none_or(|port| port == 0 || port == 8787)
        || std::path::Path::new(marker_run_directory).canonicalize()? != root
    {
        return Err("resumed Host marker does not identify the isolated Luna proof root".into());
    }
    let prior_pid = state
        .get("hostPid")
        .and_then(Value::as_u64)
        .ok_or("saved original Host PID missing")?;
    let resumed_pid = marker
        .get("hostPid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid != prior_pid)
        .ok_or("Host PID did not change across the requested restart")?;
    let inspected = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("ps")
            .args(["-p", &resumed_pid.to_string(), "-o", "comm="])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let command = String::from_utf8(inspected.stdout)?;
    if !inspected.status.success() || !command.contains("automation-debug") {
        return Err("updated Host PID does not identify the resumed acceptance Host".into());
    }
    Ok(())
}

fn required_string<'a>(state: &'a Value, field: &str) -> ProofResult<&'a str> {
    state
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("saved persistence field {field} missing").into())
}
