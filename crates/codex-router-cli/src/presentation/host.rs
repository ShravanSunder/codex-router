//! Deterministic CLI presentation for shared-host operator responses.

use std::io::Write;

use codex_router_host::HostSnapshot;
use codex_router_host::OperatorFrame;
use codex_router_host::UpdateResult;

pub(crate) fn render_frames<W: Write>(
    stdout: &mut W,
    frames: &[OperatorFrame],
) -> std::io::Result<()> {
    for frame in frames {
        match frame {
            OperatorFrame::Progress(progress) => writeln!(stdout, "progress: {progress:?}"),
            OperatorFrame::Terminal(response) => {
                writeln!(stdout, "result: {:?}", response.classification())?;
                writeln!(stdout, "message: {}", response.message())?;
                render_snapshot(stdout, response.snapshot())
            }
        }?;
    }
    Ok(())
}

pub(crate) fn render_update_result<W: Write>(
    stdout: &mut W,
    result: &UpdateResult,
) -> std::io::Result<()> {
    match result {
        UpdateResult::NoChange => writeln!(stdout, "update_result: no change"),
        UpdateResult::FailedWithoutRestart { message } => {
            writeln!(stdout, "update_result: update failed without restart")?;
            writeln!(stdout, "message: {message}")
        }
        UpdateResult::UpdatedAndHostRestarted { snapshot } => {
            writeln!(stdout, "update_result: updated and host restarted")?;
            render_snapshot(stdout, snapshot)
        }
        UpdateResult::UpdatedButReplacementFailed {
            message,
            recovery_action,
        } => {
            writeln!(stdout, "update_result: updated but replacement host failed")?;
            writeln!(stdout, "message: {message}")?;
            writeln!(stdout, "recovery_action: {recovery_action}")
        }
    }
}

fn render_snapshot<W: Write>(stdout: &mut W, snapshot: &HostSnapshot) -> std::io::Result<()> {
    writeln!(stdout, "readiness: {:?}", snapshot.hosted_readiness())?;
    writeln!(stdout, "phase: {:?}", snapshot.phase())?;
    writeln!(stdout, "router: {:?}", snapshot.router())?;
    writeln!(stdout, "app_server: {:?}", snapshot.app_server())?;
    writeln!(stdout, "remote_control: {:?}", snapshot.remote_control())?;
    if let Some(identity) = snapshot.remote_control_identity() {
        writeln!(stdout, "remote_server_name: {}", identity.server_name())?;
        writeln!(
            stdout,
            "remote_environment_id: {}",
            identity.environment_id().unwrap_or("unassigned")
        )?;
    } else {
        writeln!(stdout, "remote_server_name: unavailable")?;
        writeln!(stdout, "remote_environment_id: unavailable")?;
    }
    writeln!(stdout, "desktop_attachment: Configured")?;
    writeln!(stdout, "desktop_relaunch: required_if_running")?;
    writeln!(
        stdout,
        "executable_relation: {:?}",
        snapshot.executable_relation()
    )?;
    writeln!(stdout, "recovery_budget: {:?}", snapshot.recovery_budget())?;
    writeln!(
        stdout,
        "last_lifecycle_outcome: {:?}",
        snapshot.last_lifecycle_outcome()
    )
}

#[cfg(test)]
mod tests {
    use codex_router_host::AppServerCondition;
    use codex_router_host::ExecutableRelation;
    use codex_router_host::HostOperation;
    use codex_router_host::HostPhase;
    use codex_router_host::HostSnapshotDimensions;
    use codex_router_host::HostTerminalResponse;
    use codex_router_host::LifecycleOutcome;
    use codex_router_host::LifecycleOutcomeClassification;
    use codex_router_host::OperatorRequest;
    use codex_router_host::RecoveryBudget;
    use codex_router_host::RemoteControlCondition;
    use codex_router_host::RouterCondition;
    use codex_router_host::TerminalClassification;

    use super::*;

    #[test]
    fn host_status_is_deterministic_complete_and_canary_free() -> std::io::Result<()> {
        let snapshot = HostSnapshot::new(HostSnapshotDimensions {
            phase: HostPhase::Steady,
            router: RouterCondition::ExternalReachable,
            app_server: AppServerCondition::NativeReady {
                running_version: "1.2.3".to_owned(),
            },
            remote_control: RemoteControlCondition::Connected,
            remote_control_identity: None,
            executable_relation: ExecutableRelation::Match,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: Some(LifecycleOutcome {
                operation: HostOperation::Start,
                classification: LifecycleOutcomeClassification::Succeeded,
            }),
        });
        let frames = [OperatorFrame::terminal(HostTerminalResponse::new(
            OperatorRequest::Status,
            TerminalClassification::Ready,
            snapshot,
            "shared Codex host status".to_owned(),
        ))];
        let mut output = Vec::new();

        render_frames(&mut output, &frames)?;

        let rendered = String::from_utf8(output).map_err(std::io::Error::other)?;
        for field in [
            "readiness:",
            "phase:",
            "router:",
            "app_server:",
            "remote_control:",
            "executable_relation:",
            "recovery_budget:",
            "last_lifecycle_outcome:",
        ] {
            assert!(rendered.contains(field), "missing {field}");
        }
        assert!(!rendered.contains("PROMPT_CANARY"));
        Ok(())
    }

    #[test]
    fn replacement_failure_renders_the_required_manual_recovery_action() -> std::io::Result<()> {
        let mut output = Vec::new();
        render_update_result(
            &mut output,
            &UpdateResult::UpdatedButReplacementFailed {
                message: "replacement unavailable".to_owned(),
                recovery_action: "codex-router host".to_owned(),
            },
        )?;
        let rendered = String::from_utf8(output).map_err(std::io::Error::other)?;
        assert!(rendered.contains("updated but replacement host failed"));
        assert!(rendered.contains("recovery_action: codex-router host"));
        Ok(())
    }
}
