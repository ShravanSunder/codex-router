//! Existing provider API preserves authority and exposes returned companion values.
use super::*;

#[cfg(unix)]
#[tokio::test]
async fn cursor_effort_setting_reports_adjusted_catalog_without_public_api_expansion() -> TestResult
{
    let root = tempfile::tempdir()?;
    let receipt = root.path().join("wire.json");
    let provider_endpoint = endpoint("cursor-local")?;
    let creator = actor(provider_endpoint.clone(), "creator")?;
    let mut provider_launch = launch(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../acp-client-runtime/tests/cursor_model_settings/peer.py"
    )));
    provider_launch
        .arguments
        .extend(["adjusted".into(), receipt.to_string_lossy().into_owned()]);
    let runtime = ExternalProviderRuntime::initialize_with_mcp_http(
        provider_launch,
        acp_client_runtime::ProviderModelPicker::CursorParameterized,
        "fixture",
        "http://127.0.0.1:1/mcp",
    )
    .await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let operation_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                settings: None,
                operation_id: operation_id.clone(),
                endpoint: provider_endpoint.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: creator.clone().into(),
                approver: creator.clone().into(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let settled = operation(wait(&backend, operation_id).await)?;
    let ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Created { target, .. },
    } = settled.output
    else {
        return Err("expected created Session".into());
    };
    let rejected = backend
        .settings_set(ProviderSettingsSetRequest {
            target: target.clone(),
            actor: actor(provider_endpoint, "stranger")?.into(),
            setting: ProviderSettingName::Effort,
            value: "high".into(),
        })
        .await
        .expect_err("only creator or Approver may change effort");
    ensure_eq!(rejected.kind, ProviderSettingsFailureKind::WrongActor);
    let applied = backend
        .settings_set(ProviderSettingsSetRequest {
            target: target.clone(),
            actor: creator.into(),
            setting: ProviderSettingName::Effort,
            value: "high".into(),
        })
        .await
        .map_err(|error| error.message)?;
    ensure_eq!(
        applied.effective_settings.model.as_deref(),
        Some("fixture-a")
    );
    ensure_eq!(applied.effective_settings.effort.as_deref(), Some("high"));
    let inspected = backend
        .inspect_session(ProviderSessionInspectRequest { target })
        .await
        .map_err(|error| error.message)?;
    let catalog = inspected
        .settings_catalog
        .ok_or("returned catalog missing")?;
    for (config_id, value) in [
        ("model", "fixture-a"),
        ("effort", "high"),
        ("thinking", "true"),
        ("context", "1m"),
        ("fast", "true"),
    ] {
        let option = catalog
            .config_options
            .iter()
            .find(|option| option.id == config_id)
            .ok_or("reported option absent")?;
        ensure_eq!(
            &option.current_value,
            &collaboration_protocol::ProviderConfigValueView::Select {
                value: value.into(),
            }
        );
        if config_id == "thinking" {
            ensure!(option.category.is_none());
        }
    }
    backend.shutdown().await.map_err(str::to_owned)?;
    let requests: Vec<Value> = serde_json::from_slice(&std::fs::read(receipt)?)?;
    ensure_eq!(requests.len(), 3);
    ensure_eq!(
        &requests[0]["params"]["clientCapabilities"]["_meta"]["parameterizedModelPicker"],
        &json!(true)
    );
    ensure_eq!(&requests[2]["method"], &json!("session/set_config_option"));
    ensure_eq!(
        &requests[2]["params"],
        &json!({
            "sessionId":"fixture-session", "configId":"effort", "value":"high"
        })
    );
    Ok(())
}
