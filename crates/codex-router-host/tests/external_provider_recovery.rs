use codex_router_host::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor,
};
use collaboration_protocol::{
    CodexGeneration, ConversationAdmissionState, ConversationCreateRequest,
    ConversationLoadRequest, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, EndpointId, EndpointRef, GenerationNumber, NonEmptyText,
    OperationId, ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities,
    ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderOperationEffect, ProviderOperationKind,
    ProviderOperationStage, ProviderReconciliationState, ProviderRequestedPolicy,
    ProviderRuntimeIdentity, ProviderTransport, ProviderWorkingDirectory, RouterAccess, SessionId,
    SessionRef, UuidIdentity,
};
use collaboration_service::{
    ProviderConversationBackend, ProviderOperationAdmission, ProviderOperationStore,
};
use std::{path::Path, path::PathBuf, process::Command, sync::Arc, time::Duration};
use tokio::sync::Mutex;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const CHILD_MODE: &str = "CODEX_ROUTER_PROVIDER_RECOVERY_CHILD";
const DATABASE_PATH: &str = "CODEX_ROUTER_PROVIDER_RECOVERY_DATABASE";
const DISPATCH_LOG_PATH: &str = "CODEX_ROUTER_PROVIDER_RECOVERY_DISPATCH_LOG";
const OPERATION_ID: &str = "CODEX_ROUTER_PROVIDER_RECOVERY_OPERATION_ID";

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

fn endpoint() -> TestResult<EndpointRef> {
    Ok(EndpointRef {
        service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?,
        endpoint_id: EndpointId::try_from("claude-recovery".to_owned())?,
    })
}

fn generation() -> TestResult<CodexGeneration> {
    Ok(CodexGeneration {
        service_epoch: UuidIdentity::try_from("1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned())?,
        generation: GenerationNumber::try_from(1)?,
    })
}

fn binding() -> TestResult<ProviderBindingIdentity> {
    Ok(ProviderBindingIdentity {
        endpoint: endpoint()?,
        binding_id: ProviderBindingId::try_from("claude-recovery-binding".to_owned())?,
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::ClaudeCode,
            runtime_name: NonEmptyText::try_from("recovery-fixture".to_owned())?,
            runtime_version: Some(NonEmptyText::try_from("1".to_owned())?),
        },
        transport: ProviderTransport::StdioAcp,
        generation: generation()?,
        capabilities: ProviderCapabilities::try_from(vec![
            ProviderCapability {
                name: ProviderCapabilityName::Create,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
            ProviderCapability {
                name: ProviderCapabilityName::Load,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
        ])?,
    })
}

fn restarted_binding() -> TestResult<ProviderBindingIdentity> {
    let mut restarted = binding()?;
    restarted.binding_id =
        ProviderBindingId::try_from("claude-recovery-binding-after-restart".to_owned())?;
    restarted.generation.generation = GenerationNumber::try_from(2)?;
    Ok(restarted)
}

fn requester() -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint: endpoint()?,
        session_id: SessionId::try_from("recovery-requester".to_owned())?,
    })
}

fn target() -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint: endpoint()?,
        session_id: SessionId::try_from("known-provider-session".to_owned())?,
    })
}

fn policy() -> ProviderRequestedPolicy {
    ProviderRequestedPolicy {
        access: RouterAccess::WriteRestricted,
    }
}

fn working_directory() -> TestResult<ProviderWorkingDirectory> {
    Ok(ProviderWorkingDirectory::try_from("/tmp".to_owned())?)
}

fn launch(script: String) -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: vec![],
    }
}

fn held_dispatch_fixture(dispatch_log: &Path, expected_method: &str) -> ExternalProviderLaunch {
    let process_id_path = dispatch_log.with_extension("pid");
    launch(format!(
        r#"
import json,os,sys
log={:?}
open({:?},'w').write(str(os.getpid()))
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'recovery-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=={:?}
open(log,'a').write(request['method']+'\n')
sys.stdin.read()
"#,
        dispatch_log.display().to_string(),
        process_id_path.display().to_string(),
        expected_method
    ))
}

fn no_replay_fixture(dispatch_log: &Path) -> ExternalProviderLaunch {
    launch(format!(
        r#"
import json,sys
log={:?}
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'reopened-fixture','version':'1'}}}}}})); sys.stdout.flush()
for line in sys.stdin:
    request=json.loads(line)
    open(log,'a').write(request.get('method','unknown')+'\n')
"#,
        dispatch_log.display().to_string()
    ))
}

async fn supervisor(
    database_path: &Path,
    runtime: ExternalProviderRuntime,
) -> TestResult<ExternalProviderSupervisor> {
    supervisor_with_binding(database_path, runtime, binding()?).await
}

async fn supervisor_with_binding(
    database_path: &Path,
    runtime: ExternalProviderRuntime,
    binding: ProviderBindingIdentity,
) -> TestResult<ExternalProviderSupervisor> {
    let store = ProviderOperationStore::open(database_path).await?;
    Ok(ExternalProviderSupervisor::new(
        vec![ExternalProviderBinding {
            identity: binding,
            runtime,
        }],
        Arc::new(Mutex::new(store)),
    )?)
}

async fn await_dispatch(dispatch_log: &Path) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if dispatch_log.exists() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "provider dispatch was not observed".into())
}

#[cfg(unix)]
#[tokio::test]
async fn crash_child_records_durable_boundary() -> TestResult {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return Ok(());
    };
    let database_path = PathBuf::from(std::env::var(DATABASE_PATH)?);
    let dispatch_log = PathBuf::from(std::env::var(DISPATCH_LOG_PATH)?);
    let operation_id = OperationId::try_from(std::env::var(OPERATION_ID)?)?;

    if mode == "admission" {
        let mut store = ProviderOperationStore::open(&database_path).await?;
        store
            .admit(ProviderOperationAdmission {
                operation_id,
                operation_kind: ProviderOperationKind::ConversationCreate,
                binding: binding()?,
                admitted_at_ms: 1,
            })
            .await?;
        std::process::exit(71);
    }

    let expected_method = if mode == "create" {
        "session/new"
    } else {
        "session/load"
    };
    let runtime =
        ExternalProviderRuntime::initialize(held_dispatch_fixture(&dispatch_log, expected_method))
            .await?;
    let backend = supervisor(&database_path, runtime).await?;
    let actor = requester()?;
    if mode == "create" {
        backend
            .create(ConversationCreateRequest {
                operation_id,
                endpoint: endpoint()?,
                generation: generation()?,
                working_directory: working_directory()?,
                created_by: actor.clone(),
                approver: actor.clone(),
                requested_policy: policy(),
            })
            .await
            .map_err(|error| format!("create admission failed: {error:?}"))?;
    } else {
        backend
            .load(ConversationLoadRequest {
                operation_id,
                target: target()?,
                generation: generation()?,
                working_directory: working_directory()?,
                requested_by: actor.clone(),
                approver: actor.clone(),
                requested_policy: policy(),
            })
            .await
            .map_err(|error| format!("load admission failed: {error:?}"))?;
    }
    await_dispatch(&dispatch_log).await?;
    std::process::exit(72);
}

#[cfg(unix)]
fn run_crash_child(
    mode: &str,
    database_path: &Path,
    dispatch_log: &Path,
    operation_id: &OperationId,
) -> TestResult {
    let status = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("crash_child_records_durable_boundary")
        .arg("--nocapture")
        .env(CHILD_MODE, mode)
        .env(DATABASE_PATH, database_path)
        .env(DISPATCH_LOG_PATH, dispatch_log)
        .env(OPERATION_ID, operation_id.as_str())
        .status()?;
    ensure_eq!(
        status.code(),
        Some(if mode == "admission" { 71 } else { 72 })
    );
    if mode != "admission" {
        assert_provider_process_exited(&dispatch_log.with_extension("pid"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn assert_provider_process_exited(process_id_path: &Path) -> TestResult {
    let process_id = std::fs::read_to_string(process_id_path)?.parse::<i32>()?;
    let process_id = rustix::process::Pid::from_raw(process_id).ok_or("invalid provider PID")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if matches!(
            rustix::process::test_kill_process(process_id),
            Err(rustix::io::Errno::SRCH)
        ) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("provider process survived the simulated Host crash".into());
        }
        std::thread::yield_now();
    }
}

#[cfg(unix)]
async fn reopened_supervisor(
    database_path: &Path,
    replay_log: &Path,
) -> TestResult<ExternalProviderSupervisor> {
    let runtime = ExternalProviderRuntime::initialize(no_replay_fixture(replay_log)).await?;
    supervisor_with_binding(database_path, runtime, restarted_binding()?).await
}

#[cfg(unix)]
#[tokio::test]
async fn host_restart_preserves_admission_without_claiming_effect() -> TestResult {
    let root = tempfile::tempdir()?;
    let database_path = root.path().join("provider-operations.sqlite");
    let dispatch_log = root.path().join("dispatch.log");
    let replay_log = root.path().join("replay.log");
    let operation_id = OperationId::generate();
    run_crash_child("admission", &database_path, &dispatch_log, &operation_id)?;

    let backend = reopened_supervisor(&database_path, &replay_log).await?;
    let shown = backend
        .show(ConversationOperationShowRequest {
            operation_id: operation_id.clone(),
        })
        .await
        .map_err(|error| format!("show failed: {error:?}"))?;
    ensure_eq!(shown.stage, ProviderOperationStage::Admitted);
    ensure_eq!(shown.effect, ProviderOperationEffect::None);
    ensure_eq!(
        shown.reconciliation,
        ProviderReconciliationState::Unresolved
    );
    ensure!(shown.target.is_none());
    ensure!(!dispatch_log.exists());
    ensure!(!replay_log.exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn crash_after_create_dispatch_reopens_unknown_and_never_replays() -> TestResult {
    verify_dispatched_crash("create", None).await
}

#[cfg(unix)]
#[tokio::test]
async fn crash_after_load_dispatch_retains_known_target_and_never_replays() -> TestResult {
    verify_dispatched_crash("load", Some(target()?)).await
}

#[cfg(unix)]
async fn verify_dispatched_crash(mode: &str, expected_target: Option<SessionRef>) -> TestResult {
    let root = tempfile::tempdir()?;
    let database_path = root.path().join("provider-operations.sqlite");
    let dispatch_log = root.path().join("dispatch.log");
    let replay_log = root.path().join("replay.log");
    let operation_id = OperationId::generate();
    run_crash_child(mode, &database_path, &dispatch_log, &operation_id)?;
    ensure_eq!(
        std::fs::read_to_string(&dispatch_log)?,
        format!("session/{mode}\n").replace("create", "new")
    );

    let backend = reopened_supervisor(&database_path, &replay_log).await?;
    let shown = backend
        .show(ConversationOperationShowRequest {
            operation_id: operation_id.clone(),
        })
        .await
        .map_err(|error| format!("show failed: {error:?}"))?;
    ensure_eq!(shown.stage, ProviderOperationStage::MayHaveDispatched);
    ensure_eq!(shown.effect, ProviderOperationEffect::Unknown);
    ensure_eq!(
        shown.reconciliation,
        ProviderReconciliationState::Unresolved
    );
    ensure_eq!(shown.target, expected_target);

    let actor = requester()?;
    let duplicate = if mode == "create" {
        backend
            .create(ConversationCreateRequest {
                operation_id: operation_id.clone(),
                endpoint: endpoint()?,
                generation: generation()?,
                working_directory: working_directory()?,
                created_by: actor.clone(),
                approver: actor.clone(),
                requested_policy: policy(),
            })
            .await
    } else {
        backend
            .load(ConversationLoadRequest {
                operation_id: operation_id.clone(),
                target: target()?,
                generation: generation()?,
                working_directory: working_directory()?,
                requested_by: actor.clone(),
                approver: actor.clone(),
                requested_policy: policy(),
            })
            .await
    }
    .map_err(|error| format!("duplicate operation failed: {error:?}"))?;
    ensure_eq!(duplicate.admission, ConversationAdmissionState::Existing);

    let reconciled = backend
        .reconcile(ConversationOperationReconcileRequest {
            operation_id: operation_id.clone(),
        })
        .await
        .map_err(|error| format!("reconcile failed: {error:?}"))?;
    ensure_eq!(reconciled.operation_id, operation_id.clone());
    ensure_eq!(reconciled.effect, ProviderOperationEffect::Unknown);
    ensure_eq!(reconciled.target, expected_target);
    ensure_eq!(
        reconciled.reconciliation,
        ProviderReconciliationState::NotReconcilable
    );

    let after_reconcile = backend
        .show(ConversationOperationShowRequest { operation_id })
        .await
        .map_err(|error| format!("show after reconcile failed: {error:?}"))?;
    ensure_eq!(
        after_reconcile.reconciliation,
        ProviderReconciliationState::NotReconcilable
    );
    ensure_eq!(after_reconcile.effect, ProviderOperationEffect::Unknown);
    let fresh_operation_id = OperationId::generate();
    let restarted_generation = restarted_binding()?.generation;
    let fresh = if mode == "create" {
        backend
            .create(ConversationCreateRequest {
                operation_id: fresh_operation_id,
                endpoint: endpoint()?,
                generation: restarted_generation,
                working_directory: working_directory()?,
                created_by: actor.clone(),
                approver: actor,
                requested_policy: policy(),
            })
            .await
    } else {
        backend
            .load(ConversationLoadRequest {
                operation_id: fresh_operation_id,
                target: target()?,
                generation: restarted_generation,
                working_directory: working_directory()?,
                requested_by: actor.clone(),
                approver: actor,
                requested_policy: policy(),
            })
            .await
    };
    let fresh = match fresh {
        Ok(value) => return Err(format!("fresh mutation unexpectedly admitted: {value:?}").into()),
        Err(failure) => failure,
    };
    ensure_eq!(
        fresh.kind,
        collaboration_protocol::ConversationOperationFailureKind::Busy
    );
    ensure_eq!(fresh.effect, ProviderOperationEffect::None);
    tokio::time::sleep(Duration::from_millis(50)).await;
    ensure!(!replay_log.exists());
    Ok(())
}
