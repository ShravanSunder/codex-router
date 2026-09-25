//! Load a recorded provider session only after checking live peer ownership.
use crate::{
    ExternalProviderRuntimeError, ExternalProviderSupervisor, LiveSessionOwnership,
    LiveSessionOwnershipCheck, ProviderSessionActivity,
};
use collaboration_protocol::SessionRef;
use collaboration_service::ProviderOperationStore;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProviderSessionLoadOutcome {
    Ready,
    MissingRecord,
    LiveElsewhere,
    Rejected {
        reason: ProviderSessionLoadRejection,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderSessionLoadRejection {
    SessionNotFound { code: i64 },
    ProviderRejected { code: i64 },
}

impl ProviderSessionLoadRejection {
    #[must_use]
    pub(crate) fn code(self) -> i64 {
        match self {
            Self::SessionNotFound { code } | Self::ProviderRejected { code } => code,
        }
    }

    #[must_use]
    pub(crate) fn safe_detail(self) -> String {
        match self {
            Self::SessionNotFound { .. } => {
                "this session never started a turn and did not survive the provider restart; create a new conversation".to_owned()
            }
            Self::ProviderRejected { code } => {
                format!("provider rejected the ACP operation (code {code})")
            }
        }
    }
}

pub(crate) async fn ensure_provider_session_loaded(
    supervisor: &ExternalProviderSupervisor,
    store: &Arc<Mutex<ProviderOperationStore>>,
    ownership: &dyn LiveSessionOwnershipCheck,
    target: &SessionRef,
) -> ProviderSessionLoadOutcome {
    let Some(runtime) = supervisor.runtime_for(&target.endpoint) else {
        return unavailable("provider runtime is unavailable");
    };
    let provider_session_id = String::from(target.session_id.clone());
    match runtime.session_activity(provider_session_id.clone()).await {
        Ok(ProviderSessionActivity::Idle | ProviderSessionActivity::Running) => {
            return ProviderSessionLoadOutcome::Ready;
        }
        Ok(ProviderSessionActivity::NotLoaded) => {}
        Err(error) => return unavailable(error.to_string()),
    }
    let record = match store.lock().await.session_record(target).await {
        Ok(Some(record)) => record,
        Ok(None) => return ProviderSessionLoadOutcome::MissingRecord,
        Err(error) => return unavailable(error.to_string()),
    };
    match ownership.check(target).await {
        Ok(LiveSessionOwnership::NotLive) => {}
        Ok(LiveSessionOwnership::LiveWritable | LiveSessionOwnership::LiveUnsupported) => {
            return ProviderSessionLoadOutcome::LiveElsewhere;
        }
        Err(error) => return unavailable(error.to_string()),
    }
    match runtime
        .load_session(
            provider_session_id,
            PathBuf::from(String::from(record.working_directory)),
        )
        .await
    {
        Ok(()) => ProviderSessionLoadOutcome::Ready,
        Err(ExternalProviderRuntimeError::ProviderSessionNotFound { code }) => {
            ProviderSessionLoadOutcome::Rejected {
                reason: ProviderSessionLoadRejection::SessionNotFound { code },
            }
        }
        Err(ExternalProviderRuntimeError::AuthenticationRequired { code })
        | Err(ExternalProviderRuntimeError::ProviderRejected { code }) => {
            ProviderSessionLoadOutcome::Rejected {
                reason: ProviderSessionLoadRejection::ProviderRejected { code },
            }
        }
        Err(error) => unavailable(error.to_string()),
    }
}

fn unavailable(reason: impl Into<String>) -> ProviderSessionLoadOutcome {
    ProviderSessionLoadOutcome::Unavailable {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime};
    use collaboration_protocol::{
        CodexGeneration, EndpointId, EndpointRef, GenerationNumber, ProviderBindingId,
        ProviderBindingIdentity, ProviderCapabilities, ProviderCapability,
        ProviderCapabilityEvidence, ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind,
        ProviderRequestedPolicy, ProviderRuntimeIdentity, ProviderTransport,
        ProviderWorkingDirectory, RouterAccess, SessionId, UuidIdentity,
    };
    use collaboration_service::{ProviderOperationStore, ProviderSessionRecord};
    use std::{path::PathBuf, sync::Arc};

    struct FixtureOwnership(LiveSessionOwnership);

    impl LiveSessionOwnershipCheck for FixtureOwnership {
        fn check<'a>(
            &'a self,
            _target: &'a SessionRef,
        ) -> collaboration_service::DeliveryFuture<'a, LiveSessionOwnership> {
            let result = self.0;
            Box::pin(async move { Ok(result) })
        }
    }

    fn fixture_launch(event_path: &std::path::Path, fail_load: bool) -> ExternalProviderLaunch {
        let script = format!(
            r#"
import json,sys
first=json.loads(sys.stdin.readline())
assert first['method']=='initialize'
print(json.dumps({{'jsonrpc':'2.0','id':first['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}}}}}})); sys.stdout.flush()
line=sys.stdin.readline()
if line:
 request=json.loads(line)
 assert request['method']=='session/load', request['method']
 assert request['params']['sessionId']=='fixture-session'
 open({:?},'w').write(request['method'])
 if {fail_load}:
  print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'error':{{'code':-32001,'message':'fixture load refused'}}}})); sys.stdout.flush()
 else:
  print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
            event_path.display().to_string(),
            fail_load = if fail_load { "True" } else { "False" }
        );
        ExternalProviderLaunch {
            executable: PathBuf::from("/usr/bin/python3"),
            arguments: vec!["-c".to_owned(), script],
            environment: Vec::new(),
        }
    }

    fn target() -> SessionRef {
        SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned(),
                )
                .expect("service"),
                endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint"),
            },
            session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
        }
    }

    fn binding(target: &SessionRef) -> ProviderBindingIdentity {
        ProviderBindingIdentity {
            endpoint: target.endpoint.clone(),
            binding_id: ProviderBindingId::try_from("fixture-binding".to_owned()).expect("binding"),
            runtime: ProviderRuntimeIdentity {
                provider: ProviderKind::ClaudeCode,
                runtime_name: "fixture-agent".to_owned().try_into().expect("runtime name"),
                runtime_version: None,
            },
            transport: ProviderTransport::StdioAcp,
            generation: CodexGeneration {
                service_epoch: UuidIdentity::try_from(
                    "1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned(),
                )
                .expect("epoch"),
                generation: GenerationNumber::try_from(1).expect("generation"),
            },
            capabilities: ProviderCapabilities::try_from(vec![ProviderCapability {
                name: ProviderCapabilityName::Load,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            }])
            .expect("capabilities"),
        }
    }

    #[tokio::test]
    async fn recorded_session_loads_without_creating_a_replacement() {
        let root = tempfile::tempdir().expect("fixture root");
        let event_path = root.path().join("provider-method.txt");
        let target = target();
        let store = Arc::new(tokio::sync::Mutex::new(
            ProviderOperationStore::open(&root.path().join("operations.sqlite"))
                .await
                .expect("store"),
        ));
        store
            .lock()
            .await
            .record_session(&ProviderSessionRecord {
                target: target.clone(),
                working_directory: ProviderWorkingDirectory::try_from(
                    root.path().display().to_string(),
                )
                .expect("cwd"),
                requested_policy: ProviderRequestedPolicy {
                    access: RouterAccess::WriteRestricted,
                },
                created_by: target.clone(),
                approver: target.clone(),
                updated_at_ms: 1,
            })
            .await
            .expect("record");
        let runtime = ExternalProviderRuntime::initialize(fixture_launch(&event_path, false))
            .await
            .expect("runtime");
        let supervisor = ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding(&target),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor");

        let result = ensure_provider_session_loaded(
            &supervisor,
            &store,
            &FixtureOwnership(LiveSessionOwnership::NotLive),
            &target,
        )
        .await;

        assert!(
            matches!(result, ProviderSessionLoadOutcome::Ready),
            "{result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(event_path).expect("method"),
            "session/load"
        );
        supervisor.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn live_peer_prevents_provider_load() {
        let root = tempfile::tempdir().expect("fixture root");
        let event_path = root.path().join("provider-method.txt");
        let target = target();
        let store = Arc::new(tokio::sync::Mutex::new(
            ProviderOperationStore::open(&root.path().join("operations.sqlite"))
                .await
                .expect("store"),
        ));
        store
            .lock()
            .await
            .record_session(&ProviderSessionRecord {
                target: target.clone(),
                working_directory: ProviderWorkingDirectory::try_from(
                    root.path().display().to_string(),
                )
                .expect("cwd"),
                requested_policy: ProviderRequestedPolicy {
                    access: RouterAccess::WriteRestricted,
                },
                created_by: target.clone(),
                approver: target.clone(),
                updated_at_ms: 1,
            })
            .await
            .expect("record");
        let runtime = ExternalProviderRuntime::initialize(fixture_launch(&event_path, false))
            .await
            .expect("runtime");
        let supervisor = ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding(&target),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor");

        let result = ensure_provider_session_loaded(
            &supervisor,
            &store,
            &FixtureOwnership(LiveSessionOwnership::LiveWritable),
            &target,
        )
        .await;

        assert!(matches!(result, ProviderSessionLoadOutcome::LiveElsewhere));
        assert!(!event_path.exists());
        supervisor.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn rejected_load_preserves_provider_code_without_creating_replacement() {
        let root = tempfile::tempdir().expect("fixture root");
        let event_path = root.path().join("provider-method.txt");
        let target = target();
        let store = Arc::new(tokio::sync::Mutex::new(
            ProviderOperationStore::open(&root.path().join("operations.sqlite"))
                .await
                .expect("store"),
        ));
        store
            .lock()
            .await
            .record_session(&ProviderSessionRecord {
                target: target.clone(),
                working_directory: ProviderWorkingDirectory::try_from(
                    root.path().display().to_string(),
                )
                .expect("cwd"),
                requested_policy: ProviderRequestedPolicy {
                    access: RouterAccess::WriteRestricted,
                },
                created_by: target.clone(),
                approver: target.clone(),
                updated_at_ms: 1,
            })
            .await
            .expect("record");
        let runtime = ExternalProviderRuntime::initialize(fixture_launch(&event_path, true))
            .await
            .expect("runtime");
        let supervisor = ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding(&target),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor");

        let result = ensure_provider_session_loaded(
            &supervisor,
            &store,
            &FixtureOwnership(LiveSessionOwnership::NotLive),
            &target,
        )
        .await;

        assert!(
            matches!(&result, ProviderSessionLoadOutcome::Rejected { reason: ProviderSessionLoadRejection::ProviderRejected { code } } if *code == -32001),
            "{result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(event_path).expect("method"),
            "session/load"
        );
        supervisor.shutdown().await.expect("shutdown");
    }
}
