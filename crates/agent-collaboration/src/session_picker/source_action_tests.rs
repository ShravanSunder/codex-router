//! Captured source/action/attempt claims; no SSH or native process is substituted here.
use super::{
    picker_machine_controls::{MachineChoicePurpose, PickerMachineStage},
    picker_model::SessionsPickerModel,
    test_support::picker_request,
};
use crate::sessions::source_ssh_protocol::{SourceReadError, SourceRejection};
use crate::sessions::{RouterRegistryRead, router_connection_registry::RouterConnectionRegistry};

pub(super) fn source_model() -> SessionsPickerModel {
    let mut request = picker_request();
    request.router_registry = RouterRegistryRead::Ready(RouterConnectionRegistry::parse(r#"{"version":1,"routers":[{"name":"Source fixture","connection":{"kind":"ssh","serviceId":"00000000-0000-4000-8000-000000000001","destination":"fixture-source","executable":"/fixture/agent-sessions","serviceDirectory":"/fixture/service","clientProfile":"router"},"defaultRemoteCwd":"/source/only"}]}"#).unwrap());
    SessionsPickerModel::new(request, 120)
}

#[test]
fn source_action_new_captures_directory_and_cancel_invalidates_completion() {
    let mut model = source_model();
    model.machine_controls.stage = PickerMachineStage::Choosing {
        purpose: MachineChoicePurpose::NewSession,
        focus: 1,
        notice: None,
    };
    assert!(model.begin_configured_new());
    let captured = model.source_action.pending.clone().unwrap();
    assert!(model.begin_configured_new());
    assert_eq!(model.source_action.pending.as_ref(), Some(&captured));
    model.cancel_source_action();
    assert!(
        model
            .complete_source_action(&captured, Err(SourceReadError::ReadUnavailable))
            .is_none()
    );
    assert!(model.source_action.error.is_none());
    assert!(matches!(
        model.machine_controls.stage,
        PickerMachineStage::Choosing {
            purpose: MachineChoicePurpose::NewSession,
            focus: 1,
            ..
        }
    ));
    assert!(model.begin_configured_new());
    assert_ne!(
        model.source_action.pending.as_ref().unwrap().attempt,
        captured.attempt
    );
}

#[test]
fn source_action_forbidden_native_options_do_not_enter_checking() {
    let mut model = source_model();
    model.request.native_arguments = vec!["--cd".into(), "/caller".into()];
    model.machine_controls.stage = PickerMachineStage::Choosing {
        purpose: MachineChoicePurpose::NewSession,
        focus: 1,
        notice: None,
    };
    assert!(model.begin_configured_new());
    assert!(model.source_action.pending.is_none());
    assert_eq!(
        model.source_action.error,
        Some(SourceReadError::Rejected(
            SourceRejection::ConflictingArguments
        ))
    );
}

pub(super) fn prepared(
    request: &super::source_action_state::PendingSourceAction,
) -> crate::sessions::source_ssh_protocol::PreparedSourceLaunch {
    use crate::sessions::source_ssh_protocol::PreparedSourceLaunch;
    PreparedSourceLaunch {
        action: request.action.clone(),
        endpoint: collaboration_client::protocol::EndpointRef { service_id: request.profile.service_id.clone(), endpoint_id: "codex-local".to_owned().try_into().unwrap() },
        generation: serde_json::from_value(serde_json::json!({"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1})).unwrap(),
        schema_digest: format!("sha256:{}", "a".repeat(64)).try_into().unwrap(),
        working_directory: "/source/only".to_owned().try_into().unwrap(),
        client_profile: crate::sessions::router_connection_registry::SourceClientProfile::Router,
    }
}

#[test]
fn source_action_completion_requires_current_full_tuple_and_publishes_once() {
    let mut model = source_model();
    model.machine_controls.stage = PickerMachineStage::Choosing {
        purpose: MachineChoicePurpose::NewSession,
        focus: 1,
        notice: None,
    };
    assert!(model.begin_configured_new());
    let captured = model.source_action.pending.clone().unwrap();
    let mut wrong_attempt = captured.clone();
    wrong_attempt.attempt += 1;
    assert!(
        model
            .complete_source_action(&wrong_attempt, Ok(prepared(&captured)))
            .is_none()
    );
    let mut wrong_source = captured.clone();
    wrong_source.profile.name = "Another source".to_owned().try_into().unwrap();
    assert!(
        model
            .complete_source_action(&wrong_source, Ok(prepared(&captured)))
            .is_none()
    );
    assert!(matches!(
        model.complete_source_action(&captured, Ok(prepared(&captured))),
        Some(super::SessionsPickerOutcome::PreparedSourceSession(_))
    ));
    assert!(
        model
            .complete_source_action(&captured, Ok(prepared(&captured)))
            .is_none()
    );
}

#[test]
fn source_action_failed_prepare_retains_chooser_and_fixed_error() {
    let mut model = source_model();
    model.machine_controls.stage = PickerMachineStage::Choosing {
        purpose: MachineChoicePurpose::NewSession,
        focus: 1,
        notice: None,
    };
    model.begin_configured_new();
    let captured = model.source_action.pending.clone().unwrap();
    assert!(
        model
            .complete_source_action(
                &captured,
                Err(SourceReadError::Rejected(
                    SourceRejection::DirectoryUnavailable
                ))
            )
            .is_none()
    );
    assert_eq!(
        model.source_action.error,
        Some(SourceReadError::Rejected(
            SourceRejection::DirectoryUnavailable
        ))
    );
    assert!(matches!(
        model.machine_controls.stage,
        PickerMachineStage::Choosing {
            purpose: MachineChoicePurpose::NewSession,
            focus: 1,
            ..
        }
    ));
}

#[test]
fn source_action_resume_and_fork_freeze_full_source_model_and_cwd() {
    let mut model = source_model();
    let RouterRegistryRead::Ready(registry) = &model.request.router_registry else {
        panic!("fixture registry");
    };
    let profile = registry.routers.first().unwrap().clone();
    let record = model.request.records.first_mut().unwrap();
    record.identity = crate::sessions::SessionPickerIdentity::HostedCodex(
        collaboration_client::protocol::SessionRef {
            endpoint: collaboration_client::protocol::EndpointRef {
                service_id: profile.service_id.clone(),
                endpoint_id: "observed-source".to_owned().try_into().unwrap(),
            },
            session_id: "source-thread".to_owned().try_into().unwrap(),
        },
    );
    record.source_context = Some(super::PickerSourceContext::ConfiguredHosted(profile));
    record.provenance = crate::sessions::SessionRowProvenance::ObservedHosted;
    record.cwd = Some("/source/frozen".to_owned());
    record.model = Some("gpt-5.6-sol".to_owned());
    let identity = record.identity.clone();
    model.replace_records(super::test_support::observed_records(
        model.request.records.clone(),
    ));
    assert!(model.focus_visible_identity(&identity));
    assert!(model.begin_source_resume());
    let captured = model.source_action.pending.clone().unwrap();
    assert_eq!(captured.source_directory.as_deref(), Some("/source/frozen"));
    model.request.records.first_mut().unwrap().model = Some("replacement".to_owned());
    assert!(
        matches!(&captured.action, crate::sessions::source_ssh_protocol::SourceSessionAction::Resume { target, stored_model } if String::from(target.session_id.clone()) == "source-thread" && stored_model.model.as_deref() == Some("gpt-5.6-sol"))
    );
    model.cancel_source_action();
    model.open_fork_confirmation();
    assert!(model.begin_source_fork());
    assert!(matches!(
        model.source_action.pending.as_ref().unwrap().action,
        crate::sessions::source_ssh_protocol::SourceSessionAction::Fork { .. }
    ));
}

#[test]
fn source_action_rejects_wrong_reply_and_unobserved_rows() {
    let mut model = source_model();
    model.machine_controls.stage = PickerMachineStage::Choosing {
        purpose: MachineChoicePurpose::NewSession,
        focus: 1,
        notice: None,
    };
    model.begin_configured_new();
    let captured = model.source_action.pending.clone().unwrap();
    let mut reply = prepared(&captured);
    reply.action = crate::sessions::source_ssh_protocol::SourceSessionAction::New {
        directory: "/wrong-action".to_owned().try_into().unwrap(),
    };
    assert!(model.complete_source_action(&captured, Ok(reply)).is_none());
    assert_eq!(
        model.source_action.error,
        Some(SourceReadError::InvalidReply)
    );
    let RouterRegistryRead::Ready(registry) = &model.request.router_registry else {
        panic!("fixture");
    };
    let profile = registry.routers.first().unwrap().clone();
    let row = model.request.records.first_mut().unwrap();
    row.source_context = Some(super::PickerSourceContext::ConfiguredHosted(profile));
    let identity = row.identity.clone();
    model.replace_records(super::test_support::observed_records(
        model.request.records.clone(),
    ));
    assert!(model.focus_visible_identity(&identity));
    assert!(model.begin_source_resume());
    assert!(
        model.source_action.pending.is_none(),
        "local identity must not become configured hosted"
    );
}

#[tokio::test]
async fn source_action_worker_drops_canceled_read_without_publishing() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct DropRead(Arc<AtomicBool>);
    impl Drop for DropRead {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let mut model = source_model();
    model.machine_controls.stage = PickerMachineStage::Choosing {
        purpose: MachineChoicePurpose::NewSession,
        focus: 1,
        notice: None,
    };
    model.begin_configured_new();
    let request = model.source_action.pending.clone().unwrap();
    let (sender, receiver) = tokio::sync::watch::channel(Some(request));
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let started = Arc::new(std::sync::Mutex::new(Some(started)));
    let dropped = Arc::new(AtomicBool::new(false));
    let preparer: super::source_action_worker::SourceActionPreparer = {
        let dropped = dropped.clone();
        Arc::new(move |_, _| {
            let guard = DropRead(dropped.clone());
            let started = started.lock().unwrap().take().unwrap();
            Box::pin(async move {
                let _guard = guard;
                started.send(()).unwrap();
                std::future::pending().await
            })
        })
    };
    let (done, done_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        super::source_action_worker::run_source_preparation(receiver, preparer, |_, _| {
            panic!("canceled read cannot publish")
        })
        .await;
        done.send(()).unwrap();
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        started_rx.await.unwrap();
        sender.send_replace(None);
        drop(sender);
        done_rx.await.unwrap();
        task.await.unwrap();
    })
    .await
    .expect("cancellation must drop held preparation");
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn source_action_default_local_http_and_cross_machine_fork_do_not_prepare() {
    let mut local = SessionsPickerModel::new(picker_request(), 120);
    assert!(!local.begin_source_resume());
    assert_eq!(
        local.start_new_action(),
        Some(super::SessionsPickerOutcome::StartNewSession)
    );
    let mut model = source_model();
    let mut request = model.request.clone();
    request.router_registry = RouterRegistryRead::Ready(RouterConnectionRegistry::parse(r#"{"version":1,"routers":[{"name":"HTTP source","connection":{"kind":"remote","serviceId":"00000000-0000-4000-8000-000000000001","mcpUrl":"https://fixture.invalid/mcp"},"defaultRemoteCwd":"/source/only"}]}"#).unwrap());
    model = SessionsPickerModel::new(request, 120);
    model.machine_controls.stage = PickerMachineStage::Choosing {
        purpose: MachineChoicePurpose::NewSession,
        focus: 1,
        notice: None,
    };
    assert!(
        !model.begin_configured_new(),
        "HTTP remains unqualified for native launch"
    );
    model.open_fork_confirmation();
    if let Some(confirmation) = model.fork_confirmation.as_mut() {
        confirmation
            .destinations
            .push(super::picker_fork_confirmation::ForkDestination {
                label: "Other source".to_owned(),
                availability:
                    super::picker_fork_confirmation::ForkAvailability::HistoryRemainsOnSource,
            });
        confirmation.focused_destination = confirmation.destinations.len() - 1;
    }
    assert!(!model.begin_source_fork());
    assert!(model.source_action.pending.is_none());
}

#[test]
fn source_action_fork_changed_directory_requires_explicit_new_attempt_and_matching_reply() {
    use crate::sessions::source_ssh_protocol::SourceSessionAction;
    let mut model = source_model();
    let RouterRegistryRead::Ready(registry) = &model.request.router_registry else {
        panic!("fixture registry");
    };
    let profile = registry.routers.first().unwrap().clone();
    let record = model.request.records.first_mut().unwrap();
    let target = collaboration_client::protocol::SessionRef {
        endpoint: collaboration_client::protocol::EndpointRef {
            service_id: profile.service_id.clone(),
            endpoint_id: "codex-local".to_owned().try_into().unwrap(),
        },
        session_id: "frozen-source-parent".to_owned().try_into().unwrap(),
    };
    record.identity = crate::sessions::SessionPickerIdentity::HostedCodex(target.clone());
    record.source_context = Some(super::PickerSourceContext::ConfiguredHosted(profile));
    record.provenance = crate::sessions::SessionRowProvenance::ObservedHosted;
    record.cwd = Some("/tmp/source-parent".to_owned());
    let identity = record.identity.clone();
    model.replace_records(super::test_support::observed_records(
        model.request.records.clone(),
    ));
    assert!(model.focus_visible_identity(&identity));
    model.open_fork_confirmation();
    assert!(model.begin_source_fork());
    let first = model.source_action.pending.clone().unwrap();
    assert!(matches!(first.action, SourceSessionAction::Fork { .. }));
    let mut verified = prepared(&first);
    verified.working_directory = "/private/tmp/source-parent".to_owned().try_into().unwrap();
    assert!(
        model
            .complete_source_action(&first, Ok(verified.clone()))
            .is_none()
    );
    assert!(
        model.source_action.pending.is_none(),
        "changed directory must not start an automatic retry"
    );
    let popup = model.fork_confirmation.as_ref().unwrap();
    assert_eq!(popup.directory_label, "/private/tmp/source-parent");
    assert_eq!(popup.directory_origin, "verified source session");
    assert_eq!(
        popup.notice,
        Some("Source directory changed; confirm the verified directory.")
    );
    assert!(
        model.begin_source_fork(),
        "second explicit confirmation starts a fresh read"
    );
    let second = model.source_action.pending.clone().unwrap();
    assert_ne!(second.attempt, first.attempt);
    assert_eq!(second.action, first.action);
    assert_eq!(
        second.source_directory.as_deref(),
        Some("/private/tmp/source-parent")
    );
    assert!(
        model
            .complete_source_action(&first, Ok(verified.clone()))
            .is_none()
    );
    assert_eq!(model.source_action.pending.as_ref(), Some(&second));
    let Some(super::SessionsPickerOutcome::PreparedSourceSession(outcome)) =
        model.complete_source_action(&second, Ok(verified.clone()))
    else {
        panic!("matching explicitly confirmed directory must prepare fork");
    };
    assert_eq!(
        outcome.launch.working_directory.as_str(),
        "/private/tmp/source-parent"
    );
    assert_eq!(outcome.launch.action.target(), Some(&target));
    assert!(
        model
            .complete_source_action(&second, Ok(verified))
            .is_none()
    );
}

#[test]
fn source_action_fork_reply_cannot_publish_after_popup_was_removed() {
    let mut model = source_model();
    model.open_fork_confirmation();
    let mut captured = super::source_action_state::PendingSourceAction {
        attempt: 7,
        profile: match &model.request.router_registry {
            RouterRegistryRead::Ready(registry) => registry.routers.first().unwrap().clone(),
            _ => panic!("fixture"),
        },
        action: crate::sessions::source_ssh_protocol::SourceSessionAction::New {
            directory: "/source/only".to_owned().try_into().unwrap(),
        },
        source_directory: Some("/source/only".to_owned()),
    };
    captured.action = crate::sessions::source_ssh_protocol::SourceSessionAction::Fork {
        target: collaboration_client::protocol::SessionRef {
            endpoint: collaboration_client::protocol::EndpointRef {
                service_id: captured.profile.service_id.clone(),
                endpoint_id: "codex-local".to_owned().try_into().unwrap(),
            },
            session_id: "source-parent".to_owned().try_into().unwrap(),
        },
        stored_model: Default::default(),
    };
    model.source_action.pending = Some(captured.clone());
    model.fork_confirmation = None;
    assert!(
        model
            .complete_source_action(&captured, Ok(prepared(&captured)))
            .is_none()
    );
    assert!(model.source_action.pending.is_none());
}
