use collaboration_protocol::{
    CodexGeneration, EndpointId, EndpointRef, GenerationNumber, NonEmptyText, OperationId,
    ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities, ProviderCapability,
    ProviderCapabilityEvidence, ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind,
    ProviderOperationEffect, ProviderOperationKind, ProviderOperationStage,
    ProviderRequestedPolicy, ProviderRuntimeIdentity, ProviderTransport, ProviderWorkingDirectory,
    RouterAccess, SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    ProviderOperationAdmission, ProviderOperationStore, ProviderOperationStoreError,
    ProviderSessionRecord,
};
use sqlx::{Connection, SqliteConnection};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

macro_rules! ensure {
    ($condition:expr) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "assertion failed: {} == {}: left={left:?}, right={right:?}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    }};
}

fn session_ref(endpoint_id: &str, session_id: &str) -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?,
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned())?,
        },
        session_id: SessionId::try_from(session_id.to_owned())?,
    })
}

fn provider_binding(target: &SessionRef) -> TestResult<ProviderBindingIdentity> {
    Ok(ProviderBindingIdentity {
        endpoint: target.endpoint.clone(),
        binding_id: ProviderBindingId::try_from("provider-binding-1".to_owned())?,
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::ClaudeCode,
            runtime_name: NonEmptyText::try_from("fixture-provider".to_owned())?,
            runtime_version: None,
        },
        transport: ProviderTransport::StdioAcp,
        generation: CodexGeneration {
            service_epoch: UuidIdentity::try_from(
                "1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned(),
            )?,
            generation: GenerationNumber::try_from(1)?,
        },
        capabilities: ProviderCapabilities::try_from(vec![ProviderCapability {
            name: ProviderCapabilityName::Create,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        }])?,
    })
}

#[tokio::test]
async fn provider_session_metadata_round_trips_and_rejects_invalid_stored_rows() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("provider-operations.sqlite");
    let mut store = ProviderOperationStore::open(&path).await?;
    let target = session_ref("claude-local", "provider-session-1")?;
    let record = ProviderSessionRecord {
        target: target.clone(),
        working_directory: ProviderWorkingDirectory::try_from(root.path().display().to_string())?,
        requested_policy: ProviderRequestedPolicy {
            access: RouterAccess::WriteRestricted,
        },
        created_by: session_ref("codex-local", "creator-1")?,
        approver: session_ref("codex-local", "approver-1")?,
        updated_at_ms: 10,
    };

    store.record_session(&record).await?;
    let restored = store
        .session_record(&target)
        .await?
        .ok_or("session record missing")?;
    ensure_eq!(restored, record);

    let mut raw = SqliteConnection::connect(&format!("sqlite:{}", path.display())).await?;
    sqlx::query("UPDATE provider_session_records SET working_directory = 'relative' WHERE target_session_id = 'provider-session-1'")
        .execute(&mut raw)
        .await?;
    raw.close().await?;
    ensure!(matches!(
        store.session_record(&target).await,
        Err(ProviderOperationStoreError::InvalidRecord)
    ));
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn create_settlement_and_session_record_commit_together() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("provider-operations.sqlite");
    let mut store = ProviderOperationStore::open(&path).await?;
    let target = session_ref("claude-local", "provider-session-2")?;
    let operation_id = OperationId::try_from("018f1f62-6571-7ef0-8f0c-001122334488".to_owned())?;
    store
        .admit(ProviderOperationAdmission {
            operation_id: operation_id.clone(),
            operation_kind: ProviderOperationKind::ConversationCreate,
            binding: collaboration_protocol::ConversationBindingIdentity::ExternalProvider {
                binding: provider_binding(&target)?,
            },
            admitted_at_ms: 10,
        })
        .await?;
    store.mark_may_have_dispatched(&operation_id, 11).await?;
    let mut record = ProviderSessionRecord {
        target: session_ref("cursor-local", "wrong-endpoint")?,
        working_directory: ProviderWorkingDirectory::try_from(root.path().display().to_string())?,
        requested_policy: ProviderRequestedPolicy {
            access: RouterAccess::WriteRestricted,
        },
        created_by: session_ref("codex-local", "creator-2")?,
        approver: session_ref("codex-local", "approver-2")?,
        updated_at_ms: 12,
    };

    ensure!(matches!(
        store.settle_session_operation(&operation_id, &record).await,
        Err(ProviderOperationStoreError::InvalidRecord)
    ));
    ensure_eq!(
        store
            .inspect(&operation_id)
            .await?
            .ok_or("operation missing")?
            .stage,
        ProviderOperationStage::MayHaveDispatched
    );
    record.target = target.clone();
    store
        .settle_session_operation(&operation_id, &record)
        .await?;

    let settled = store
        .inspect(&operation_id)
        .await?
        .ok_or("settled operation missing")?;
    ensure_eq!(settled.stage, ProviderOperationStage::Terminal);
    ensure_eq!(settled.effect, ProviderOperationEffect::Applied);
    ensure_eq!(settled.target, Some(target.clone()));
    ensure_eq!(store.session_record(&target).await?, Some(record));
    store.close().await?;
    Ok(())
}
