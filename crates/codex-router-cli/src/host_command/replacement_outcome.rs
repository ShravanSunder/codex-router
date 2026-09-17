//! Operator result classification and shared readiness observation across Host replacement.

use std::time::Duration;

use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostSnapshot;
use codex_router_host::HostTerminalResponse;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::TerminalClassification;
use codex_router_host::UpdateResult;

use super::operator_client::replacement_started_without_terminal;
use super::operator_client::send_replacement_operator_request;

pub(super) const REPLACEMENT_CONVERGENCE_DEADLINE: Duration = Duration::from_secs(40);
const REPLACEMENT_RECOVERY_ACTION: &str = "codex-router host";

pub(super) async fn complete_update_result(
    coordination_paths: &HostCoordinationPaths,
    frames: Vec<OperatorFrame>,
) -> UpdateResult {
    complete_update_result_with_deadline(
        coordination_paths,
        frames,
        REPLACEMENT_CONVERGENCE_DEADLINE,
    )
    .await
}

async fn complete_update_result_with_deadline(
    coordination_paths: &HostCoordinationPaths,
    frames: Vec<OperatorFrame>,
    replacement_deadline: Duration,
) -> UpdateResult {
    let replacement_started = frames
        .iter()
        .any(|frame| matches!(frame, OperatorFrame::Progress(_)));
    if let Some(OperatorFrame::Terminal(response)) = frames.last() {
        if replacement_started {
            return UpdateResult::UpdatedButReplacementFailed {
                message: response.message().to_owned(),
                recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
            };
        }
        return if response.classification() == TerminalClassification::Succeeded {
            UpdateResult::NoChange
        } else {
            UpdateResult::FailedWithoutRestart {
                message: response.message().to_owned(),
            }
        };
    }
    if !replacement_started {
        return UpdateResult::FailedWithoutRestart {
            message: "shared Codex host update returned no terminal result".to_owned(),
        };
    }

    match observe_replacement(coordination_paths, replacement_deadline).await {
        Ok(snapshot) => UpdateResult::UpdatedAndHostRestarted { snapshot },
        Err(message) => UpdateResult::UpdatedButReplacementFailed {
            message,
            recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
        },
    }
}

pub(crate) enum HostRestartResult {
    Restarted { snapshot: HostSnapshot },
    NotRestarted { response: HostTerminalResponse },
    ReplacementFailed { message: String },
}

impl HostRestartResult {
    pub(crate) fn failure_message(&self) -> Option<&str> {
        match self {
            Self::Restarted { .. } => None,
            Self::NotRestarted { response } => Some(response.message()),
            Self::ReplacementFailed { message } => Some(message),
        }
    }
}

pub(super) async fn complete_restart_result(
    coordination_paths: &HostCoordinationPaths,
    frames: Vec<OperatorFrame>,
) -> HostRestartResult {
    complete_restart_result_with_deadline(
        coordination_paths,
        frames,
        REPLACEMENT_CONVERGENCE_DEADLINE,
    )
    .await
}

async fn complete_restart_result_with_deadline(
    coordination_paths: &HostCoordinationPaths,
    frames: Vec<OperatorFrame>,
    deadline: Duration,
) -> HostRestartResult {
    if let Some(OperatorFrame::Terminal(response)) = frames.last() {
        return HostRestartResult::NotRestarted {
            response: response.clone(),
        };
    }
    if !replacement_started_without_terminal(&frames) {
        return HostRestartResult::ReplacementFailed {
            message: "Host returned no terminal restart result or replacement progress".to_owned(),
        };
    }
    match observe_replacement(coordination_paths, deadline).await {
        Ok(snapshot) => HostRestartResult::Restarted { snapshot },
        Err(message) => HostRestartResult::ReplacementFailed { message },
    }
}

async fn observe_replacement(
    coordination_paths: &HostCoordinationPaths,
    deadline: Duration,
) -> Result<HostSnapshot, String> {
    let replacement = send_replacement_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::AwaitHostStart,
        deadline,
    )
    .await
    .map_err(|error| format!("replacement Host did not become ready: {error}"))?;
    match replacement.last() {
        Some(OperatorFrame::Terminal(response))
            if matches!(
                response.classification(),
                TerminalClassification::Ready | TerminalClassification::LocalReadyRemoteDegraded
            ) =>
        {
            Ok(response.snapshot().clone())
        }
        Some(OperatorFrame::Terminal(response)) => Err(response.message().to_owned()),
        _ => Err("replacement Host returned no terminal readiness".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use codex_router_host::AppServerCondition;
    use codex_router_host::ExecutableRelation;
    use codex_router_host::HostPhase;
    use codex_router_host::HostProgress;
    use codex_router_host::HostSnapshot;
    use codex_router_host::HostSnapshotDimensions;
    use codex_router_host::HostTerminalResponse;
    use codex_router_host::RecoveryBudget;
    use codex_router_host::RemoteControlCondition;
    use codex_router_host::RouterCondition;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[tokio::test]
    async fn busy_restart_preserves_terminal_result_without_reconnecting() {
        let result = complete_restart_result_with_deadline(
            &unused_paths(),
            vec![OperatorFrame::busy(
                OperatorRequest::RestartHost {
                    executable: PathBuf::from("/installed/router"),
                },
                ready_snapshot(),
                "another lifecycle mutation is active".to_owned(),
            )],
            Duration::from_millis(10),
        )
        .await;
        assert!(
            matches!(result, HostRestartResult::NotRestarted { response }
            if response.classification() == TerminalClassification::Busy)
        );
    }

    #[tokio::test]
    async fn restart_progress_without_a_ready_replacement_is_failure() {
        let result = complete_restart_result_with_deadline(
            &unused_paths(),
            vec![OperatorFrame::Progress(HostProgress::ReplacementStarting)],
            Duration::from_millis(10),
        )
        .await;
        assert!(matches!(
            result,
            HostRestartResult::ReplacementFailed { .. }
        ));
    }

    #[tokio::test]
    async fn updater_no_change_and_failure_remain_distinct_without_restart() {
        for classification in [
            TerminalClassification::Succeeded,
            TerminalClassification::Failed,
        ] {
            let result = complete_update_result_with_deadline(
                &unused_paths(),
                vec![OperatorFrame::terminal(HostTerminalResponse::new(
                    OperatorRequest::UpdateCodex,
                    classification,
                    ready_snapshot(),
                    "managed updater terminal result".to_owned(),
                ))],
                Duration::from_millis(10),
            )
            .await;
            match classification {
                TerminalClassification::Succeeded => {
                    assert!(matches!(result, UpdateResult::NoChange))
                }
                _ => assert!(matches!(result, UpdateResult::FailedWithoutRestart { .. })),
            }
        }
    }

    #[tokio::test]
    async fn update_result_maps_post_change_terminal_failure_to_manual_recovery() {
        let paths = unused_paths();
        let result = complete_update_result_with_deadline(
            &paths,
            vec![
                OperatorFrame::Progress(HostProgress::ReplacementStarting),
                OperatorFrame::terminal(HostTerminalResponse::new(
                    OperatorRequest::UpdateCodex,
                    TerminalClassification::Failed,
                    ready_snapshot(),
                    "changed update teardown failed".to_owned(),
                )),
            ],
            Duration::from_millis(10),
        )
        .await;
        assert!(matches!(
            result,
            UpdateResult::UpdatedButReplacementFailed { recovery_action, .. }
                if recovery_action == REPLACEMENT_RECOVERY_ACTION
        ));
    }

    #[tokio::test]
    async fn update_result_maps_missing_replacement_endpoint_to_manual_recovery() {
        let directory =
            std::env::temp_dir().join(format!("codex-router-update-result-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("fixture directory must create");
        let paths = HostCoordinationPaths::new(
            directory.join("missing.sock"),
            directory.join("instance.lock"),
        );
        let result = complete_update_result_with_deadline(
            &paths,
            vec![OperatorFrame::Progress(HostProgress::ReplacementStarting)],
            Duration::from_millis(20),
        )
        .await;
        let _cleanup_result = std::fs::remove_dir(&directory);
        assert!(matches!(
            result,
            UpdateResult::UpdatedButReplacementFailed { recovery_action, .. }
                if recovery_action == REPLACEMENT_RECOVERY_ACTION
        ));
    }

    #[tokio::test]
    async fn update_result_maps_ready_replacement_to_updated_and_restarted()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!(
            "codex-router-update-success-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory)?;
        let paths = HostCoordinationPaths::new(
            directory.join("operator.sock"),
            directory.join("instance.lock"),
        );
        let listener = tokio::net::UnixListener::bind(paths.operator_socket())?;
        let server = tokio::spawn(async move {
            let (mut stream, _peer) = listener.accept().await?;
            let mut request = Vec::new();
            stream.read_to_end(&mut request).await?;
            let decoded = codex_router_host::decode_operator_request(&request)
                .map_err(std::io::Error::other)?;
            if decoded != OperatorRequest::AwaitHostStart {
                return Err(std::io::Error::other("unexpected replacement request"));
            }
            let response = codex_router_host::encode_operator_frame(&OperatorFrame::terminal(
                HostTerminalResponse::new(
                    OperatorRequest::AwaitHostStart,
                    TerminalClassification::Ready,
                    ready_snapshot(),
                    "replacement ready".to_owned(),
                ),
            ))
            .map_err(std::io::Error::other)?;
            stream.write_all(&response).await?;
            stream.shutdown().await
        });

        let result = complete_update_result_with_deadline(
            &paths,
            vec![OperatorFrame::Progress(HostProgress::ReplacementStarting)],
            Duration::from_secs(1),
        )
        .await;
        server.await??;
        let _socket_cleanup = std::fs::remove_file(paths.operator_socket());
        let _directory_cleanup = std::fs::remove_dir(&directory);
        if !matches!(result, UpdateResult::UpdatedAndHostRestarted { .. }) {
            return Err("ready replacement did not produce updated-and-restarted".into());
        }
        Ok(())
    }

    fn unused_paths() -> HostCoordinationPaths {
        HostCoordinationPaths::new(
            PathBuf::from("/unused/operator.sock"),
            PathBuf::from("/unused/instance.lock"),
        )
    }

    fn ready_snapshot() -> HostSnapshot {
        HostSnapshot::new(HostSnapshotDimensions {
            phase: HostPhase::Steady,
            router: RouterCondition::OwnedReachable,
            app_server: AppServerCondition::NativeReady {
                running_version: "1.2.3".to_owned(),
            },
            remote_control: RemoteControlCondition::Connected,
            remote_control_identity: None,
            executable_relation: ExecutableRelation::Match,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: None,
        })
    }
}
