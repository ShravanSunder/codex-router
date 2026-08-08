//! Owner-visible update result classification across foreground replacement.

use std::time::Duration;

use codex_router_host::HostCoordinationPaths;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::TerminalClassification;
use codex_router_host::UpdateResult;

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

    match send_replacement_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::AwaitHostStart,
        replacement_deadline,
    )
    .await
    {
        Ok(replacement) => match replacement.last() {
            Some(OperatorFrame::Terminal(response))
                if matches!(
                    response.classification(),
                    TerminalClassification::Ready
                        | TerminalClassification::LocalReadyRemoteDegraded
                ) =>
            {
                UpdateResult::UpdatedAndHostRestarted {
                    snapshot: response.snapshot().clone(),
                }
            }
            Some(OperatorFrame::Terminal(response)) => UpdateResult::UpdatedButReplacementFailed {
                message: response.message().to_owned(),
                recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
            },
            _ => UpdateResult::UpdatedButReplacementFailed {
                message: "replacement host returned no terminal readiness".to_owned(),
                recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
            },
        },
        Err(_) => UpdateResult::UpdatedButReplacementFailed {
            message: "updated Codex but replacement host did not become ready".to_owned(),
            recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
        },
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
            executable_relation: ExecutableRelation::Match,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: None,
        })
    }
}
