//! Deterministic CLI presentation for shared-host operator responses.

use std::io::Write;

use codex_router_host::HostSnapshot;
use codex_router_host::OperatorFrame;

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

fn render_snapshot<W: Write>(stdout: &mut W, snapshot: &HostSnapshot) -> std::io::Result<()> {
    writeln!(stdout, "readiness: {:?}", snapshot.hosted_readiness())?;
    writeln!(stdout, "phase: {:?}", snapshot.phase())?;
    writeln!(stdout, "router: {:?}", snapshot.router())?;
    writeln!(stdout, "app_server: {:?}", snapshot.app_server())?;
    writeln!(stdout, "remote_control: {:?}", snapshot.remote_control())?;
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
}
