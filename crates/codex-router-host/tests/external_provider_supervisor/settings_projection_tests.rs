use super::*;

#[cfg(unix)]
#[tokio::test]
async fn provider_create_projects_effective_partial_and_invalid_settings() -> TestResult {
    for mode in [
        "success",
        "partial",
        "partial_first",
        "invalid",
        "invalid_no_close",
    ] {
        let root = tempfile::tempdir()?;
        let provider_endpoint = endpoint("cursor-local")?;
        let creator = actor(provider_endpoint.clone(), "creator")?;
        let script = SETTINGS_CREATE_FIXTURE.replace("__MODE__", mode);
        let runtime = ExternalProviderRuntime::initialize(launch(&script)).await?;
        let backend = supervisor(
            &root,
            vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
        )
        .await?;
        let mut models = backend
            .provider_model_catalog(&provider_endpoint)
            .ok_or("provider model watch missing")?;
        let default_models = models.borrow().clone();
        ensure_eq!(
            default_models.as_slice(),
            &[collaboration_service::ProviderModelEntry::try_new(
                "provider-default".into(),
                "provider default".into(),
                "The provider selects its default model".into(),
            )?]
        );
        let operation_id = OperationId::generate();
        operation(
            backend
                .create(ConversationCreateRequest {
                    settings: Some(ProviderRequestedSettings {
                        mode: (!mode.starts_with("invalid")).then(|| "ask".to_owned()),
                        model: Some(
                            if mode.starts_with("invalid") {
                                "wrong"
                            } else {
                                "b"
                            }
                            .to_owned(),
                        ),
                        effort: None,
                    }),
                    operation_id: operation_id.clone(),
                    endpoint: provider_endpoint.clone(),
                    generation: Some(generation()?),
                    working_directory: working_directory()?,
                    created_by: (creator.clone()).into(),
                    approver: (creator.clone()).into(),
                    requested_policy: policy(),
                })
                .await,
        )?;
        match mode {
            "success" => {
                let settled = operation(wait(&backend, operation_id).await)?;
                let ConversationOperationWaitOutput::Available {
                    settlement:
                        ConversationOperationSettlement::Created {
                            target,
                            effective_settings,
                            ..
                        },
                } = settled.output
                else {
                    return Err("expected created settlement".into());
                };
                ensure_eq!(effective_settings.mode.as_deref(), Some("ask"));
                ensure_eq!(effective_settings.model.as_deref(), Some("b"));
                tokio::time::timeout(std::time::Duration::from_secs(2), models.changed()).await??;
                ensure_eq!(models.borrow().len(), 2);
                let advertised_models = models.borrow().clone();
                ensure_eq!(
                    advertised_models.as_slice(),
                    &[
                        collaboration_service::ProviderModelEntry::try_new(
                            "a".into(),
                            "a".into(),
                            "Advertised by the provider".into()
                        )?,
                        collaboration_service::ProviderModelEntry::try_new(
                            "b".into(),
                            "b".into(),
                            "Advertised by the provider".into()
                        )?,
                    ]
                );
                let inspected = backend
                    .inspect_session(ProviderSessionInspectRequest { target })
                    .await
                    .map_err(|error| error.message)?;
                ensure_eq!(
                    inspected.capabilities.auth_status,
                    session_event_model::ProviderAuthStatus::NotReported
                );
                ensure_eq!(
                    inspected
                        .settings_catalog
                        .as_ref()
                        .and_then(|catalog| catalog.current_mode.as_deref()),
                    Some("ask")
                );
            }
            "partial" => {
                let settled = operation(wait(&backend, operation_id).await)?;
                let ConversationOperationWaitOutput::Available {
                    settlement:
                        ConversationOperationSettlement::CreatedWithoutSettings {
                            target,
                            applied,
                            failed,
                            not_applied,
                        },
                } = settled.output
                else {
                    return Err("expected partial settings settlement".into());
                };
                ensure_eq!(applied.len(), 1);
                ensure_eq!(failed.len(), 1);
                ensure!(not_applied.is_empty());
                let mut store =
                    ProviderOperationStore::open(&root.path().join("provider-operations.sqlite"))
                        .await?;
                ensure!(store.session_record(&target).await?.is_some());
                let wrong_actor = actor(provider_endpoint.clone(), "stranger")?;
                let denied = backend
                    .settings_set(ProviderSettingsSetRequest {
                        target: target.clone(),
                        actor: (wrong_actor).into(),
                        setting: ProviderSettingName::Model,
                        value: "b".into(),
                    })
                    .await
                    .expect_err("actor must be creator or Approver");
                ensure_eq!(denied.kind, ProviderSettingsFailureKind::WrongActor);
                let accepted = backend
                    .settings_accept(ProviderSettingsAcceptRequest {
                        target: target.clone(),
                        actor: (creator.clone()).into(),
                    })
                    .await
                    .map_err(|error| error.message)?;
                ensure_eq!(accepted.target, target);
                ensure_eq!(accepted.effective_settings.mode.as_deref(), Some("ask"));
            }
            "partial_first" => {
                let settled = operation(wait(&backend, operation_id).await)?;
                let ConversationOperationWaitOutput::Available {
                    settlement:
                        ConversationOperationSettlement::CreatedWithoutSettings {
                            applied,
                            failed,
                            not_applied,
                            ..
                        },
                } = settled.output
                else {
                    return Err("expected partial-first settings settlement".into());
                };
                ensure!(applied.is_empty());
                ensure_eq!(failed.len(), 1);
                ensure_eq!(failed[0].setting, ProviderSettingName::Mode);
                ensure_eq!(not_applied.len(), 1);
                ensure_eq!(not_applied[0].setting, ProviderSettingName::Model);
                ensure_eq!(not_applied[0].value.as_str(), "b");
            }
            "invalid" | "invalid_no_close" => {
                let failure = wait(&backend, operation_id.clone())
                    .await
                    .expect_err("invalid setting");
                ensure_eq!(
                    failure.kind,
                    ConversationOperationFailureKind::InvalidSetting
                );
                let failure_message = String::from(failure.message.clone());
                let detail = failure
                    .invalid_setting
                    .ok_or("missing invalid-setting detail")?;
                ensure_eq!(detail.setting, ProviderSettingName::Model);
                ensure_eq!(detail.value, "wrong");
                ensure_eq!(detail.advertised, vec!["a".to_owned(), "b".to_owned()]);
                let expected_disposition = if mode == "invalid" {
                    collaboration_protocol::InvalidSettingSessionDisposition::Closed
                } else {
                    collaboration_protocol::InvalidSettingSessionDisposition::RemainsCreated
                };
                ensure_eq!(detail.session_disposition, expected_disposition);
                let target = failure.target.ok_or("invalid setting target missing")?;
                let shown = operation(
                    backend
                        .show(ConversationOperationShowRequest {
                            operation_id: operation_id.clone(),
                        })
                        .await,
                )?;
                ensure_eq!(shown.target.as_ref(), Some(&target));
                ensure_eq!(shown.stage, ProviderOperationStage::Terminal);
                ensure_eq!(shown.effect, ProviderOperationEffect::Applied);
                ensure_eq!(shown.reconciliation, ProviderReconciliationState::Confirmed);
                ensure_eq!(
                    failure_message,
                    format!(
                        "invalid provider setting model=\"wrong\"; advertised: \"a\", \"b\"; {}",
                        if mode == "invalid" {
                            "new Session was closed"
                        } else {
                            "new Session remains created and idle"
                        }
                    )
                );
                let mut store =
                    ProviderOperationStore::open(&root.path().join("provider-operations.sqlite"))
                        .await?;
                ensure_eq!(
                    store.session_record(&target).await?.is_some(),
                    mode == "invalid_no_close"
                );
            }
            _ => return Err("unknown fixture case".into()),
        }
        backend
            .shutdown()
            .await
            .map_err(|message| message.to_owned())?;
    }
    Ok(())
}
