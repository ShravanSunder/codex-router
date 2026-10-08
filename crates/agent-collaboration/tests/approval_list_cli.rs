use serde_json::{Value, json};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use tokio_util::sync::CancellationToken;

struct AcceptedNoticeDelivery;

impl collaboration_service::SessionMessageDelivery for AcceptedNoticeDelivery {
    fn deliver<'a>(
        &'a self,
        _: collaboration_service::layer_zero::DeliveryRequest,
        _: &'a dyn collaboration_service::AttemptEvidenceSink,
    ) -> collaboration_service::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        Box::pin(async {
            Ok(collaboration_protocol::DeliveryReceipt {
                outcome: collaboration_protocol::DeliveryOutcome::Started,
                reachability: Some(collaboration_protocol::SessionReachability::CodexAppServer),
                client: None,
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: collaboration_service::AttemptReconciliationContext,
    ) -> collaboration_service::DeliveryFuture<'_, collaboration_service::AttemptReconciliation>
    {
        Box::pin(async { Ok(collaboration_service::AttemptReconciliation::StillUnknown) })
    }
}

#[tokio::test]
async fn provider_pending_approval_uses_legacy_cli_shape_and_safe_decision() {
    use std::sync::Arc;
    let directory = tempfile::tempdir_in("/tmp").expect("private fixture directory");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private fixture permissions");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let typed_service_id: collaboration_protocol::UuidIdentity =
        service_id.to_owned().try_into().expect("service ID");
    let endpoint = collaboration_protocol::EndpointRef {
        service_id: typed_service_id.clone(),
        endpoint_id: "claude-local".to_owned().try_into().expect("endpoint ID"),
    };
    let broker = collaboration_service::ServiceInteractionBroker::load(
        typed_service_id,
        collaboration_service::NativeControlBackend {
            endpoint: endpoint.clone(),
            gate: collaboration_service::NativeGenerationGate::default(),
            codex_home: directory.path().to_owned(),
        },
        directory.path().join("approval-routes.json"),
    )
    .await
    .expect("broker");
    broker
        .install_session_delivery(Arc::new(AcceptedNoticeDelivery))
        .expect("notice route");
    let automation_store = Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&directory.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch)
        .expect("service identity")
        .with_automation_store(Arc::clone(&automation_store))
        .with_approval_broker(Arc::clone(&broker));
    let requester: message_board::SessionRef = serde_json::from_value(json!({
        "endpoint":endpoint,"sessionId":"provider-session"
    }))
    .expect("provider requester");
    let prompting_requester: collaboration_protocol::SessionRef = serde_json::from_value(json!({
        "endpoint":endpoint,"sessionId":"prompting-session"
    }))
    .expect("prompting requester");
    let approver: message_board::SessionRef = serde_json::from_value(json!({
        "endpoint":endpoint,"sessionId":"approver-session"
    }))
    .expect("session approver");
    let request = serde_json::from_value(json!({
        "requestId":"provider-cli-approval","title":"Run command",
        "options":[
            {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}},
            {"optionId":"allow-always","label":"Always allow","choice":{"effect":"allow","scope":{"persistent":{"where_stored":"provider settings"}}}}
        ]
    })).expect("approval request");
    let receiver = broker
        .request_typed_approval(
            requester,
            message_board::Identity::Session {
                session: approver.clone(),
            },
            request,
            CancellationToken::new(),
            CancellationToken::new(),
            Some(collaboration_service::TypedApprovalLegacyContext {
                operation_id: collaboration_protocol::OperationId::generate(),
                target: serde_json::from_value(
                    json!({"endpoint":endpoint,"sessionId":"provider-session"}),
                )
                .expect("target"),
                generation: serde_json::from_value(json!({"serviceEpoch":epoch,"generation":1}))
                    .expect("generation"),
                requested_by: prompting_requester.clone().into(),
            }),
        )
        .await
        .expect("pending provider approval");
    let served = collaboration_mcp::test_support::ServedCollaborationApi::start(
        directory.path(),
        collaboration_service::CollaborationApplication::new(identity),
    )
    .await
    .expect("serve collaboration API");
    let listed = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "approval",
            "list",
            "--pending",
            "--json",
            "--service-directory",
        ])
        .arg(directory.path())
        .output()
        .await
        .expect("CLI list");
    assert_eq!(
        listed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let output: Value = serde_json::from_slice(&listed.stdout).expect("CLI output");
    let row = &output["result"]["record"]["approvals"][0];
    assert_eq!(
        row["requestId"], "provider-cli-approval",
        "CLI output: {output}"
    );
    assert_eq!(row["approver"], json!(approver));
    assert_eq!(row["requester"], json!(prompting_requester));
    assert!(row["expiresAt"].is_string());
    assert!(row.get("optionsOrigin").is_none());
    let decided = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "approval",
            "decide",
            "--request-id",
            "provider-cli-approval",
            "--allow",
            "--actor",
        ])
        .arg(serde_json::to_string(&approver).expect("actor JSON"))
        .args(["--json", "--service-directory"])
        .arg(directory.path())
        .output()
        .await
        .expect("CLI decision");
    assert_eq!(
        decided.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&decided.stderr)
    );
    assert!(matches!(receiver.await.expect("agent resolution"),
        collaboration_service::TypedApprovalResolution::Selected(selected)
            if selected.option_id.as_str() == "allow-once"));
    served.stop().await.expect("collaboration API stops");
}

#[tokio::test]
async fn approval_list_rejection_preserves_rejected_kind_and_exit_four() {
    let root = std::path::PathBuf::from(format!("/tmp/approval-list-cli-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let identity =
        collaboration_service::ServiceIdentity::new(service_id, epoch).expect("service identity");
    let served = collaboration_mcp::test_support::ServedCollaborationApi::start(
        &root,
        collaboration_service::CollaborationApplication::new(identity),
    )
    .await
    .expect("serve collaboration API");
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["approval", "list", "--json", "--service-directory"])
        .arg(&root)
        .output()
        .await
        .expect("approval list output");
    served.stop().await.expect("collaboration API stops");
    std::fs::remove_dir(&root).expect("fixture cleanup");

    assert_eq!(
        output.status.code(),
        Some(4),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let record: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    assert_eq!(record["kind"], "error");
    assert_eq!(record["error"]["kind"], "rejected");
    assert_eq!(record["error"]["effect"], "none");
}
