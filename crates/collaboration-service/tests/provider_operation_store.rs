use collaboration_protocol::{
    CodexGeneration, EndpointId, EndpointRef, GenerationNumber, NonEmptyText, OperationId,
    ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities, ProviderCapability,
    ProviderCapabilityEvidence, ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind,
    ProviderOperationEffect, ProviderOperationKind, ProviderOperationStage,
    ProviderReconciliationState, ProviderRuntimeIdentity, ProviderTransport, SessionId, SessionRef,
    UuidIdentity,
};
use collaboration_service::{
    ProviderOperationAdmission, ProviderOperationAdmissionResult, ProviderOperationStore,
    ProviderOperationStoreError,
};
use sqlx::{Connection, Row, SqliteConnection};
use std::{collections::HashSet, path::PathBuf};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn assert_send<T: Send>(_: T) {}

#[test]
fn open_future_is_send_for_host_composition() {
    let path = PathBuf::from("provider-operations.sqlite");
    assert_send(ProviderOperationStore::open(&path));
}

#[allow(dead_code)]
fn store_operation_futures_are_send<'a>(
    store: &'a mut ProviderOperationStore,
    operation_id: &'a OperationId,
    target: &'a SessionRef,
    protected: &'a HashSet<OperationId>,
    admission: ProviderOperationAdmission,
) {
    assert_send(store.admit(admission));
    assert_send(store.inspect(operation_id));
    assert_send(store.mark_may_have_dispatched(operation_id, 1));
    assert_send(store.record_target(operation_id, target, 1));
    assert_send(store.record_terminal(
        operation_id,
        ProviderOperationEffect::Unknown,
        ProviderReconciliationState::Unresolved,
        1,
    ));
    assert_send(store.prune_terminal_before(1, protected));
}

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

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> TestResult<Self> {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random)?;
        Ok(Self {
            path: std::env::temp_dir().join(format!(
                "provider-operation-{label}-{}.sqlite",
                u64::from_le_bytes(random)
            )),
        })
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-shm", "-wal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn operation_id(value: &str) -> TestResult<OperationId> {
    Ok(OperationId::try_from(value.to_owned())?)
}

fn endpoint() -> TestResult<EndpointRef> {
    Ok(EndpointRef {
        service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?,
        endpoint_id: EndpointId::try_from("claude-local".to_owned())?,
    })
}

fn binding() -> TestResult<ProviderBindingIdentity> {
    Ok(ProviderBindingIdentity {
        endpoint: endpoint()?,
        binding_id: ProviderBindingId::try_from("claude-bridge-1".to_owned())?,
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::ClaudeCode,
            runtime_name: NonEmptyText::try_from("claude-agent-acp".to_owned())?,
            runtime_version: Some(NonEmptyText::try_from("1.2.3".to_owned())?),
        },
        transport: ProviderTransport::StdioAcp,
        generation: CodexGeneration {
            service_epoch: UuidIdentity::try_from(
                "1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned(),
            )?,
            generation: GenerationNumber::try_from(7)?,
        },
        capabilities: ProviderCapabilities::try_from(vec![
            ProviderCapability {
                name: ProviderCapabilityName::Prompt,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
            ProviderCapability {
                name: ProviderCapabilityName::CallerDetach,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::RouterQualified,
            },
        ])?,
    })
}

fn target() -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint: endpoint()?,
        session_id: SessionId::try_from("provider-conversation-1".to_owned())?,
    })
}

fn admission(id: &str, admitted_at_ms: i64) -> TestResult<ProviderOperationAdmission> {
    Ok(ProviderOperationAdmission {
        operation_id: operation_id(id)?,
        operation_kind: ProviderOperationKind::ConversationPrompt,
        binding: binding()?,
        admitted_at_ms,
    })
}

fn admitted_record(
    result: ProviderOperationAdmissionResult,
) -> TestResult<collaboration_service::ProviderOperationRecord> {
    match result {
        ProviderOperationAdmissionResult::Admitted(record) => Ok(record),
        ProviderOperationAdmissionResult::Existing(_) => {
            Err("fixture operation ID unexpectedly existed".into())
        }
    }
}

#[tokio::test]
async fn admission_dispatch_and_terminal_states_survive_reopen() -> TestResult {
    let database = TestDatabase::new("crash-order")?;
    let mut store = ProviderOperationStore::open(&database.path).await?;
    let admitted = store
        .admit(admission("018f1f62-6571-7ef0-8f0c-001122334455", 1_000)?)
        .await?;
    let admitted = admitted_record(admitted)?;
    ensure_eq!(admitted.stage, ProviderOperationStage::Admitted);
    ensure_eq!(admitted.effect, ProviderOperationEffect::None);
    let admission_only = admitted_record(
        store
            .admit(admission("018f1f62-6571-7ef0-8f0c-001122334456", 1_050)?)
            .await?,
    )?;

    store
        .mark_may_have_dispatched(&admitted.operation_id, 1_100)
        .await?;
    store.close().await?;

    let mut store = ProviderOperationStore::open(&database.path).await?;
    let reopened = store
        .inspect(&admitted.operation_id)
        .await?
        .ok_or("dispatch-marked operation was not retained")?;
    ensure_eq!(reopened.stage, ProviderOperationStage::MayHaveDispatched);
    ensure_eq!(reopened.effect, ProviderOperationEffect::Unknown);
    ensure_eq!(reopened.binding, binding()?);
    let no_dispatch = store
        .inspect(&admission_only.operation_id)
        .await?
        .ok_or("admission-only operation was not retained")?;
    ensure_eq!(no_dispatch.stage, ProviderOperationStage::Admitted);
    ensure_eq!(no_dispatch.effect, ProviderOperationEffect::None);

    store
        .record_target(&admitted.operation_id, &target()?, 1_200)
        .await?;
    store
        .record_terminal(
            &admitted.operation_id,
            ProviderOperationEffect::Applied,
            ProviderReconciliationState::Confirmed,
            1_300,
        )
        .await?;
    store.close().await?;

    let mut store = ProviderOperationStore::open(&database.path).await?;
    let terminal = store
        .inspect(&admitted.operation_id)
        .await?
        .ok_or("terminal operation was not retained")?;
    ensure_eq!(terminal.stage, ProviderOperationStage::Terminal);
    ensure_eq!(terminal.effect, ProviderOperationEffect::Applied);
    ensure_eq!(terminal.target, Some(target()?));
    ensure_eq!(
        terminal.reconciliation_state,
        ProviderReconciliationState::Confirmed
    );
    Ok(())
}

#[tokio::test]
async fn backwards_clock_transitions_remain_readable_and_monotonic() -> TestResult {
    let database = TestDatabase::new("clock-rollback")?;
    let mut store = ProviderOperationStore::open(&database.path).await?;
    let operation_id = operation_id("018f1f62-6571-7ef0-8f0c-001122334499")?;
    admitted_record(
        store
            .admit(admission(operation_id.as_str(), 10_000)?)
            .await?,
    )?;

    let dispatched = store.mark_may_have_dispatched(&operation_id, 9_000).await?;
    ensure_eq!(dispatched.dispatched_at_ms, Some(10_000));
    ensure_eq!(dispatched.updated_at_ms, 10_000);
    let terminal = store
        .record_terminal(
            &operation_id,
            ProviderOperationEffect::Unknown,
            ProviderReconciliationState::Unresolved,
            8_000,
        )
        .await?;
    ensure_eq!(terminal.terminal_at_ms, Some(10_000));
    ensure_eq!(terminal.updated_at_ms, 10_000);
    ensure!(store.inspect(&operation_id).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn duplicate_operation_id_is_inspection_only() -> TestResult {
    let database = TestDatabase::new("duplicate")?;
    let mut store = ProviderOperationStore::open(&database.path).await?;
    let first = store
        .admit(admission("018f1f62-6571-7ef0-8f0c-001122334455", 1_000)?)
        .await?;
    let first = admitted_record(first)?;

    let mut changed = admission("018f1f62-6571-7ef0-8f0c-001122334455", 9_000)?;
    changed.operation_kind = ProviderOperationKind::ConversationCreate;
    let duplicate = store.admit(changed).await?;
    ensure_eq!(
        duplicate,
        ProviderOperationAdmissionResult::Existing(first.clone())
    );
    ensure_eq!(store.inspect(&first.operation_id).await?, Some(first));
    Ok(())
}

#[tokio::test]
async fn malformed_stored_enum_and_closed_binding_are_rejected() -> TestResult {
    let database = TestDatabase::new("malformed-enum")?;
    let mut store = ProviderOperationStore::open(&database.path).await?;
    let admitted = store
        .admit(admission("018f1f62-6571-7ef0-8f0c-001122334455", 1_000)?)
        .await?;
    let record = admitted_record(admitted)?;
    store.close().await?;

    let mut connection =
        SqliteConnection::connect(&format!("sqlite:{}", database.path.display())).await?;
    sqlx::query("UPDATE provider_operations SET effect='invented' WHERE operation_id=?")
        .bind(String::from(record.operation_id.clone()))
        .execute(&mut connection)
        .await?;
    connection.close().await?;

    let mut store = ProviderOperationStore::open(&database.path).await?;
    ensure!(matches!(
        store.inspect(&record.operation_id).await,
        Err(ProviderOperationStoreError::InvalidRecord)
    ));
    store.close().await?;

    let mut connection =
        SqliteConnection::connect(&format!("sqlite:{}", database.path.display())).await?;
    sqlx::query(
        r#"UPDATE provider_operations SET effect='none',binding_json='{"unexpected":true}'
           WHERE operation_id=?"#,
    )
    .bind(String::from(record.operation_id.clone()))
    .execute(&mut connection)
    .await?;
    connection.close().await?;

    let mut store = ProviderOperationStore::open(&database.path).await?;
    ensure!(matches!(
        store.inspect(&record.operation_id).await,
        Err(ProviderOperationStoreError::InvalidRecord)
    ));
    Ok(())
}

#[tokio::test]
async fn retention_is_bounded_and_excludes_unresolved_and_protected_operations() -> TestResult {
    let database = TestDatabase::new("retention")?;
    let mut store = ProviderOperationStore::open(&database.path).await?;
    let unresolved = terminal_record(
        &mut store,
        "018f1f62-6571-7ef0-8f0c-001122334401",
        ProviderReconciliationState::Unresolved,
        100,
    )
    .await?;
    let protected = terminal_record(
        &mut store,
        "018f1f62-6571-7ef0-8f0c-001122334402",
        ProviderReconciliationState::Confirmed,
        100,
    )
    .await?;
    let unknown_not_reconcilable = terminal_record_with_effect(
        &mut store,
        "018f1f62-6571-7ef0-8f0c-001122334499",
        ProviderOperationEffect::Unknown,
        ProviderReconciliationState::NotReconcilable,
        100,
    )
    .await?;
    for suffix in 403..=1_405 {
        terminal_record(
            &mut store,
            &format!("018f1f62-6571-7ef0-8f0c-{suffix:012}"),
            ProviderReconciliationState::Confirmed,
            100,
        )
        .await?;
    }

    let protected_ids = HashSet::from([protected.clone()]);
    ensure_eq!(
        store.prune_terminal_before(200, &protected_ids).await?,
        1_000
    );
    ensure!(store.inspect(&unresolved).await?.is_some());
    ensure!(store.inspect(&protected).await?.is_some());
    ensure!(store.inspect(&unknown_not_reconcilable).await?.is_some());
    ensure_eq!(store.prune_terminal_before(200, &protected_ids).await?, 3);
    ensure_eq!(store.prune_terminal_before(200, &protected_ids).await?, 0);
    Ok(())
}

async fn terminal_record(
    store: &mut ProviderOperationStore,
    id: &str,
    reconciliation: ProviderReconciliationState,
    terminal_at_ms: i64,
) -> Result<OperationId, ProviderOperationStoreError> {
    let effect = if reconciliation == ProviderReconciliationState::Unresolved {
        ProviderOperationEffect::Unknown
    } else {
        ProviderOperationEffect::Applied
    };
    terminal_record_with_effect(store, id, effect, reconciliation, terminal_at_ms).await
}

async fn terminal_record_with_effect(
    store: &mut ProviderOperationStore,
    id: &str,
    effect: ProviderOperationEffect,
    reconciliation: ProviderReconciliationState,
    terminal_at_ms: i64,
) -> Result<OperationId, ProviderOperationStoreError> {
    let input = admission(id, 1).map_err(|_| ProviderOperationStoreError::InvalidIdentity)?;
    let record = match store.admit(input).await? {
        ProviderOperationAdmissionResult::Admitted(record) => record,
        ProviderOperationAdmissionResult::Existing(_) => {
            return Err(ProviderOperationStoreError::TransitionConflict);
        }
    };
    store
        .mark_may_have_dispatched(&record.operation_id, 2)
        .await?;
    store
        .record_terminal(&record.operation_id, effect, reconciliation, terminal_at_ms)
        .await?;
    Ok(record.operation_id)
}

#[tokio::test]
async fn uncertainty_matches_stable_endpoint_provider_scope_across_binding_restart() -> TestResult {
    let database = TestDatabase::new("stable-uncertainty")?;
    let mut store = ProviderOperationStore::open(&database.path).await?;
    let record = admitted_record(
        store
            .admit(admission("018f1f62-6571-7ef0-8f0c-001122334488", 1)?)
            .await?,
    )?;
    store
        .mark_may_have_dispatched(&record.operation_id, 2)
        .await?;
    store
        .record_target(&record.operation_id, &target()?, 3)
        .await?;
    let mut restarted = binding()?;
    restarted.binding_id = ProviderBindingId::try_from("claude-bridge-restarted".to_owned())?;
    restarted.generation.generation = GenerationNumber::try_from(99)?;
    ensure!(
        store
            .has_blocking_uncertainty(&restarted, Some(&target()?))
            .await?
    );
    ensure!(!store.has_blocking_uncertainty(&restarted, None).await?);
    Ok(())
}

#[tokio::test]
async fn migration_reopens_populated_database_and_schema_is_metadata_only() -> TestResult {
    let database = TestDatabase::new("schema")?;
    let mut store = ProviderOperationStore::open(&database.path).await?;
    let admitted = store
        .admit(admission("018f1f62-6571-7ef0-8f0c-001122334455", 1_000)?)
        .await?;
    let record = admitted_record(admitted)?;
    store.close().await?;

    let mut store = ProviderOperationStore::open(&database.path).await?;
    ensure!(store.inspect(&record.operation_id).await?.is_some());
    store.close().await?;

    let mut connection =
        SqliteConnection::connect(&format!("sqlite:{}", database.path.display())).await?;
    let columns = sqlx::query("PRAGMA table_info(provider_operations)")
        .fetch_all(&mut connection)
        .await?
        .into_iter()
        .map(|row| row.try_get::<String, _>("name"))
        .collect::<Result<Vec<_>, _>>()?;
    ensure_eq!(
        columns,
        [
            "operation_id",
            "operation_kind",
            "binding_json",
            "target_service_id",
            "target_endpoint_id",
            "target_session_id",
            "stage",
            "effect",
            "reconciliation_state",
            "admitted_at_ms",
            "dispatched_at_ms",
            "terminal_at_ms",
            "updated_at_ms",
        ]
    );
    let forbidden = [
        "prompt",
        "payload",
        "payload_hash",
        "reply",
        "result",
        "tool_output",
        "error",
        "canonical_request",
        "transcript",
    ];
    ensure!(
        columns
            .iter()
            .all(|column| !forbidden.contains(&column.as_str()))
    );
    sqlx::query("ALTER TABLE provider_operations ADD COLUMN payload TEXT")
        .execute(&mut connection)
        .await?;
    connection.close().await?;
    ensure!(matches!(
        ProviderOperationStore::open(&database.path).await,
        Err(ProviderOperationStoreError::InvalidRecord)
    ));
    Ok(())
}
