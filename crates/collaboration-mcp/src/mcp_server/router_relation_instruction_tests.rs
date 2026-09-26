use super::CollaborationMcpServer;
use collaboration_protocol::RouterExecutableRelation;
use rmcp::ServerHandler;
use std::sync::Arc;

#[tokio::test]
async fn initialize_instructions_report_injected_drift_relation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (_sender, relation_receiver) =
        tokio::sync::watch::channel(RouterExecutableRelation::Drift {
            running_version: "0.1.36".to_owned(),
            installed_version: Some("0.1.37".to_owned()),
        });
    let server = CollaborationMcpServer::with_lifecycle(
        directory.path().to_owned(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        relation_receiver,
    );
    let initialize = serde_json::to_value(server.get_info()).expect("initialize response");
    assert_eq!(
        initialize["instructions"].as_str(),
        Some(
            "⚠ Router Host is stale (running 0.1.36, installed 0.1.37); run `codex-router host restart`"
        )
    );
}

#[tokio::test]
async fn initialize_instructions_omit_warning_for_injected_match_relation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (_sender, relation_receiver) = tokio::sync::watch::channel(RouterExecutableRelation::Match);
    let server = CollaborationMcpServer::with_lifecycle(
        directory.path().to_owned(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        relation_receiver,
    );
    let initialize = serde_json::to_value(server.get_info()).expect("initialize response");
    assert!(initialize["instructions"].is_null());
}
