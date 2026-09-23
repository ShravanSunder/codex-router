//! Deterministic CLI presentation for shared-host operator responses.

use std::io::Write;
use std::time::Instant;

use crate::host_command::replacement_outcome::HostRestartResult;
use codex_router_host::HostProgress;
use codex_router_host::HostSnapshot;
use codex_router_host::OperatorFrame;
use codex_router_host::UpdateResult;

pub(crate) struct HostProgressPresenter {
    active: Option<(HostProgress, Instant, indicatif::ProgressBar)>,
    tty: bool,
}

impl HostProgressPresenter {
    pub(crate) fn new(tty: bool) -> Self {
        Self { active: None, tty }
    }

    pub(crate) fn accept<W: Write>(
        &mut self,
        stdout: &mut W,
        frame: &OperatorFrame,
    ) -> std::io::Result<()> {
        match frame {
            OperatorFrame::Progress(progress) => {
                if is_phase_completion(*progress) {
                    self.finish(
                        stdout,
                        if *progress == HostProgress::RemoteControlDegraded {
                            Some(
                                codex_router_host::TerminalClassification::LocalReadyRemoteDegraded,
                            )
                        } else {
                            None
                        },
                    )?;
                    if self.active.is_none() && !self.tty {
                        writeln!(stdout, "✓ {}", progress_label(*progress))?;
                    }
                    return Ok(());
                }
                if *progress == HostProgress::AppServerKilled {
                    self.finish(
                        stdout,
                        Some(codex_router_host::TerminalClassification::Failed),
                    )?;
                    if !self.tty {
                        writeln!(stdout, "⚠ {}", progress_label(*progress))?;
                    }
                    return Ok(());
                }
                self.finish(stdout, None)?;
                let spinner = indicatif::ProgressBar::new_spinner();
                spinner.enable_steady_tick(std::time::Duration::from_millis(80));
                spinner.set_message(progress_label(*progress).to_owned());
                self.active = Some((*progress, Instant::now(), spinner));
            }
            OperatorFrame::Terminal(response) => {
                self.finish(stdout, Some(response.classification()))?
            }
        }
        Ok(())
    }

    pub(crate) fn finish_success<W: Write>(&mut self, stdout: &mut W) -> std::io::Result<()> {
        self.finish(stdout, None)
    }

    fn finish<W: Write>(
        &mut self,
        stdout: &mut W,
        terminal: Option<codex_router_host::TerminalClassification>,
    ) -> std::io::Result<()> {
        let Some((progress, started, spinner)) = self.active.take() else {
            return Ok(());
        };
        let glyph = if progress == HostProgress::AppServerKilled {
            "⚠"
        } else if matches!(
            terminal,
            Some(
                codex_router_host::TerminalClassification::Failed
                    | codex_router_host::TerminalClassification::Busy
            )
        ) {
            "✗"
        } else {
            "✓"
        };
        spinner.finish_and_clear();
        if !self.tty {
            return writeln!(
                stdout,
                "{glyph} {} ({:.2?})",
                progress_label(progress),
                started.elapsed()
            );
        }
        writeln!(
            stdout,
            "{glyph} {} ({:.2?})",
            progress_label(progress),
            started.elapsed()
        )
    }
}

pub(crate) fn render_restart_result<W: Write>(
    stdout: &mut W,
    result: &HostRestartResult,
) -> std::io::Result<()> {
    match result {
        HostRestartResult::Restarted { snapshot } => {
            writeln!(
                stdout,
                "restart_result: host restarted using installed executable"
            )?;
            render_snapshot(stdout, snapshot)
        }
        HostRestartResult::NotRestarted { response } => {
            writeln!(stdout, "restart_result: host not restarted")?;
            writeln!(
                stdout,
                "result: {}",
                classification_label(response.classification())
            )?;
            writeln!(stdout, "message: {}", response.message())?;
            render_snapshot(stdout, response.snapshot())
        }
        HostRestartResult::ReplacementFailed { message } => {
            writeln!(stdout, "restart_result: replacement host failed")?;
            writeln!(stdout, "message: {message}")?;
            writeln!(
                stdout,
                "recovery_action: after the old Host exits, run codex-router host with its original root and port"
            )
        }
    }
}

pub(crate) fn render_terminal_frame<W: Write>(
    stdout: &mut W,
    frames: &[OperatorFrame],
) -> std::io::Result<()> {
    if let Some(OperatorFrame::Terminal(response)) = frames.last() {
        writeln!(
            stdout,
            "result: {}",
            classification_label(response.classification())
        )?;
        writeln!(stdout, "message: {}", response.message())?;
        render_snapshot(stdout, response.snapshot())?;
    }
    Ok(())
}

pub(crate) fn render_progress_event<W: Write>(
    stdout: &mut W,
    progress: codex_router_host::HostProgress,
) -> std::io::Result<()> {
    let mut presenter = HostProgressPresenter::new(false);
    presenter.accept(stdout, &OperatorFrame::Progress(progress))?;
    presenter.finish_success(stdout)
}

fn progress_label(progress: HostProgress) -> &'static str {
    match progress {
        codex_router_host::HostProgress::PreparingAppServer => "preparing app-server",
        codex_router_host::HostProgress::ReplacementStarting => "starting Host replacement",
        codex_router_host::HostProgress::StoppingAppServer => "stopping app-server",
        codex_router_host::HostProgress::StartingAppServer => "starting app-server",
        codex_router_host::HostProgress::WaitingForRemoteControl => "waiting for Remote Control",
        codex_router_host::HostProgress::AppServerKilled => "forced app-server shutdown",
        codex_router_host::HostProgress::StoppingRouter => "stopping router",
        codex_router_host::HostProgress::PreparingRouter => "preparing router",
        codex_router_host::HostProgress::StartingRouter => "starting router",
        codex_router_host::HostProgress::ReExecuting => "re-executing Host",
        codex_router_host::HostProgress::RouterReady => "router ready",
        codex_router_host::HostProgress::AppServerReady => "app-server ready",
        codex_router_host::HostProgress::RemoteControlReady => "Remote Control ready",
        codex_router_host::HostProgress::RemoteControlDegraded => "Remote Control degraded",
        codex_router_host::HostProgress::UpdatingAppServer => "updating app-server",
    }
}

fn is_phase_completion(progress: HostProgress) -> bool {
    matches!(
        progress,
        HostProgress::RouterReady
            | HostProgress::AppServerReady
            | HostProgress::RemoteControlReady
            | HostProgress::RemoteControlDegraded
    )
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
        UpdateResult::UpdatedAndAppServerRestarted { snapshot } => {
            writeln!(stdout, "update_result: updated and app-server restarted")?;
            render_snapshot(stdout, snapshot)
        }
        UpdateResult::UpdatedButReplacementFailed {
            message,
            recovery_action,
        } => {
            writeln!(stdout, "update_result: updated but replacement failed")?;
            writeln!(stdout, "message: {message}")?;
            writeln!(stdout, "recovery_action: {recovery_action}")
        }
    }
}

fn render_snapshot<W: Write>(stdout: &mut W, snapshot: &HostSnapshot) -> std::io::Result<()> {
    writeln!(
        stdout,
        "readiness: {}",
        readiness_label(snapshot.hosted_readiness())
    )?;
    writeln!(stdout, "phase: {}", phase_label(snapshot.phase()))?;
    writeln!(stdout, "router: {}", router_label(snapshot.router()))?;
    writeln!(
        stdout,
        "app_server: {}",
        app_server_label(snapshot.app_server())
    )?;
    writeln!(
        stdout,
        "remote_control: {}",
        remote_control_label(snapshot.remote_control())
    )?;
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
    writeln!(stdout, "desktop attachment: configured")?;
    writeln!(
        stdout,
        "desktop relaunch: restart required if already running"
    )?;
    writeln!(
        stdout,
        "executable_relation: {}",
        executable_relation_label(snapshot.executable_relation())
    )?;
    writeln!(
        stdout,
        "recovery_budget: {}",
        recovery_budget_label(snapshot.recovery_budget())
    )?;
    writeln!(
        stdout,
        "last_lifecycle_outcome: {}",
        outcome_label(snapshot.last_lifecycle_outcome())
    )
}

fn classification_label(classification: codex_router_host::TerminalClassification) -> &'static str {
    match classification {
        codex_router_host::TerminalClassification::Ready => "ready",
        codex_router_host::TerminalClassification::LocalReadyRemoteDegraded => {
            "local ready (Remote Control degraded)"
        }
        codex_router_host::TerminalClassification::Unavailable => "unavailable",
        codex_router_host::TerminalClassification::Succeeded => "succeeded",
        codex_router_host::TerminalClassification::Failed => "failed",
        codex_router_host::TerminalClassification::Busy => "busy",
    }
}

fn readiness_label(readiness: codex_router_host::HostedReadiness) -> &'static str {
    match readiness {
        codex_router_host::HostedReadiness::Ready => "ready",
        codex_router_host::HostedReadiness::LocalReadyRemoteDegraded => {
            "local ready (Remote Control degraded)"
        }
        codex_router_host::HostedReadiness::Unavailable => "unavailable",
    }
}

fn phase_label(phase: &codex_router_host::HostPhase) -> &'static str {
    match phase {
        codex_router_host::HostPhase::Starting => "starting",
        codex_router_host::HostPhase::Steady => "steady",
        codex_router_host::HostPhase::Mutating { .. } => "mutating",
        codex_router_host::HostPhase::Stopping => "stopping",
    }
}

fn router_label(router: codex_router_host::RouterCondition) -> &'static str {
    match router {
        codex_router_host::RouterCondition::ExternalReachable => "external router ready",
        codex_router_host::RouterCondition::OwnedReachable => "host-owned router ready",
        codex_router_host::RouterCondition::OwnedTransitioning => "host-owned router starting",
        codex_router_host::RouterCondition::Unavailable => "unavailable",
    }
}

fn app_server_label(app_server: &codex_router_host::AppServerCondition) -> String {
    match app_server {
        codex_router_host::AppServerCondition::NativeReady { running_version } => {
            format!("ready ({running_version})")
        }
        codex_router_host::AppServerCondition::Starting => "starting".to_owned(),
        codex_router_host::AppServerCondition::Stopping => "stopping".to_owned(),
        codex_router_host::AppServerCondition::ShutdownTimedOut => "shutdown timed out".to_owned(),
        codex_router_host::AppServerCondition::Absent => "absent".to_owned(),
        codex_router_host::AppServerCondition::Failed => "failed".to_owned(),
    }
}

fn remote_control_label(condition: codex_router_host::RemoteControlCondition) -> &'static str {
    match condition {
        codex_router_host::RemoteControlCondition::Connected => "connected",
        codex_router_host::RemoteControlCondition::Connecting => "connecting",
        codex_router_host::RemoteControlCondition::Errored => "error",
        codex_router_host::RemoteControlCondition::Disabled => "disabled",
        codex_router_host::RemoteControlCondition::Unavailable => "unavailable",
    }
}

fn executable_relation_label(relation: codex_router_host::ExecutableRelation) -> &'static str {
    match relation {
        codex_router_host::ExecutableRelation::Match => "matches installed executable",
        codex_router_host::ExecutableRelation::Drift => "differs from installed executable",
        codex_router_host::ExecutableRelation::Unknown => "unknown",
    }
}

fn recovery_budget_label(budget: codex_router_host::RecoveryBudget) -> &'static str {
    match budget {
        codex_router_host::RecoveryBudget::Available => "available",
        codex_router_host::RecoveryBudget::Consumed => "consumed",
    }
}

fn outcome_label(outcome: Option<&codex_router_host::LifecycleOutcome>) -> &'static str {
    let Some(outcome) = outcome else {
        return "none";
    };
    match outcome.classification {
        codex_router_host::LifecycleOutcomeClassification::Succeeded => "succeeded",
        codex_router_host::LifecycleOutcomeClassification::Failed => "failed",
        codex_router_host::LifecycleOutcomeClassification::Forced => "forced",
        codex_router_host::LifecycleOutcomeClassification::TimedOut => "timed out",
        codex_router_host::LifecycleOutcomeClassification::Busy => "busy",
    }
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

        render_terminal_frame(&mut output, &frames)?;

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
                recovery_action:
                    "start codex-router host with the same --router-root and --port after the old Host exits"
                        .to_owned(),
            },
        )?;
        let rendered = String::from_utf8(output).map_err(std::io::Error::other)?;
        assert!(rendered.contains("updated but replacement failed"));
        assert!(rendered.contains(
            "recovery_action: start codex-router host with the same --router-root and --port"
        ));
        Ok(())
    }

    #[test]
    fn presenter_renders_forced_shutdown_as_warning_and_failure_as_error() -> std::io::Result<()> {
        let mut presenter = HostProgressPresenter::new(false);
        let mut output = Vec::new();
        presenter.accept(
            &mut output,
            &OperatorFrame::Progress(HostProgress::AppServerKilled),
        )?;
        presenter.accept(
            &mut output,
            &OperatorFrame::Terminal(HostTerminalResponse::new(
                OperatorRequest::Status,
                TerminalClassification::Failed,
                ready_snapshot(),
                "failed".to_owned(),
            )),
        )?;
        let rendered = String::from_utf8(output).map_err(std::io::Error::other)?;
        assert!(rendered.contains("⚠ forced app-server shutdown"));
        Ok(())
    }

    #[test]
    fn presenter_completes_delayed_phases_in_tty_and_plain_modes() -> std::io::Result<()> {
        for tty in [false, true] {
            let mut presenter = HostProgressPresenter::new(tty);
            let mut output = Vec::new();
            presenter.accept(
                &mut output,
                &OperatorFrame::Progress(HostProgress::StartingRouter),
            )?;
            for _ in 0..20_000 {
                std::hint::spin_loop();
            }
            presenter.accept(
                &mut output,
                &OperatorFrame::Progress(HostProgress::AppServerReady),
            )?;
            let rendered = String::from_utf8(output).map_err(std::io::Error::other)?;
            assert!(rendered.contains("✓ starting router ("));
            assert!(rendered.contains("starting router ("));
        }
        Ok(())
    }

    #[test]
    fn streamed_progress_and_terminal_render_each_phase_once() -> std::io::Result<()> {
        let mut presenter = HostProgressPresenter::new(false);
        let mut output = Vec::new();
        presenter.accept(
            &mut output,
            &OperatorFrame::Progress(HostProgress::StartingAppServer),
        )?;
        presenter.accept(
            &mut output,
            &OperatorFrame::Progress(HostProgress::AppServerReady),
        )?;
        presenter.finish_success(&mut output)?;
        let response = HostTerminalResponse::new(
            OperatorRequest::RestartAppServer,
            TerminalClassification::Succeeded,
            ready_snapshot(),
            "app-server restarted".to_owned(),
        );
        render_terminal_frame(
            &mut output,
            &[
                OperatorFrame::Progress(HostProgress::StartingAppServer),
                OperatorFrame::Terminal(response),
            ],
        )?;
        let rendered = String::from_utf8(output).map_err(std::io::Error::other)?;
        assert_eq!(rendered.matches("starting app-server").count(), 1);
        assert_eq!(rendered.matches("app-server ready").count(), 1);
        assert_eq!(rendered.matches("result: succeeded").count(), 1);
        Ok(())
    }

    fn ready_snapshot() -> HostSnapshot {
        HostSnapshot::new(HostSnapshotDimensions {
            phase: HostPhase::Steady,
            router: RouterCondition::ExternalReachable,
            app_server: AppServerCondition::Absent,
            remote_control: RemoteControlCondition::Unavailable,
            remote_control_identity: None,
            executable_relation: ExecutableRelation::Unknown,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: None,
        })
    }
}
