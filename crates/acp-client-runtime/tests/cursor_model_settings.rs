//! R1–R4: real ACP wires and returned catalogs, without native provider state.
#![allow(clippy::expect_used)]

#[path = "cursor_model_settings/support.rs"]
mod support;

use acp_client_runtime::{
    ExternalProviderRuntimeError, ProviderConfigValue, ProviderSettingKind,
    RequestedProviderSettings,
};
use session_event_model::ConfigValueState;
use support::WireFixture;

#[cfg(feature = "test-observation")]
#[tokio::test]
async fn native_observation_entrypoint_uses_picker_wire_without_mcp_injection() {
    let fixture = WireFixture::start_without_mcp("thinking-first").await;
    fixture
        .client
        .create_session(fixture.root.path().into())
        .await
        .expect("native-path fixture session");
    fixture.client.shutdown().await;
    let requests = fixture.requests();
    assert_eq!(
        requests[0]["params"]["clientCapabilities"]["_meta"]["parameterizedModelPicker"],
        true
    );
    assert_eq!(requests[1]["params"]["mcpServers"], serde_json::json!([]));
}

#[tokio::test]
async fn cursor_initialize_advertises_parameterized_picker_on_the_wire() {
    let fixture = WireFixture::start("negotiation", true).await;
    fixture.client.shutdown().await;
    let requests = fixture.requests();
    assert_eq!(
        requests[0]["params"]["clientCapabilities"]["_meta"]["parameterizedModelPicker"],
        true
    );
}

#[tokio::test]
async fn standard_initialize_omits_cursor_picker_metadata() {
    let fixture = WireFixture::start("negotiation", false).await;
    fixture.client.shutdown().await;
    let requests = fixture.requests();
    assert!(
        requests[0]["params"]["clientCapabilities"]
            .get("_meta")
            .is_none()
    );
}

#[tokio::test]
async fn thinking_first_and_explicit_effort_win_without_companion_rewrites() {
    for scenario in ["thinking-first", "explicit-wins", "fallback"] {
        let fixture = WireFixture::start(scenario, false).await;
        let session = fixture
            .client
            .create_session(fixture.root.path().into())
            .await
            .expect("session");
        let observed = fixture
            .client
            .set_setting(session.clone(), ProviderSettingKind::Effort, "high".into())
            .await;
        fixture.client.shutdown().await;
        let observed = observed.expect("independent advertised effort must apply");
        assert_eq!(observed.effort.as_deref(), Some("high"));
        let requests = fixture.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2]["method"], "session/set_config_option");
        let config_id = if scenario == "fallback" {
            "budget"
        } else {
            "effort"
        };
        assert_eq!(
            requests[2]["params"],
            serde_json::json!({
            "sessionId":"fixture-session", "configId":config_id, "value":"high"})
        );
        let catalog = fixture
            .client
            .settings_catalog(&session)
            .await
            .expect("catalog");
        for (config_id, value) in [
            ("model", "fixture-a"),
            ("thinking", "true"),
            ("context", "300k"),
            ("fast", "false"),
        ] {
            assert_eq!(
                catalog
                    .config_options
                    .iter()
                    .find(|option| option.id == config_id)
                    .expect("companion option")
                    .current_value,
                ProviderConfigValue::Select(value.into())
            );
        }
        assert_eq!(
            catalog
                .config_options
                .iter()
                .find(|option| option.id == "thinking")
                .expect("thinking option")
                .category,
            None
        );
    }
}

#[tokio::test]
async fn ambiguous_or_thinking_only_effort_rejects_before_rpc() {
    for scenario in [
        "ambiguous",
        "thinking-only",
        "duplicate-explicit",
        "duplicate-mixed",
    ] {
        let fixture = WireFixture::start(scenario, false).await;
        let session = fixture
            .client
            .create_session(fixture.root.path().into())
            .await
            .expect("session");
        let result = fixture
            .client
            .set_setting(session, ProviderSettingKind::Effort, "high".into())
            .await;
        fixture.client.shutdown().await;
        assert!(
            matches!(result, Err(ExternalProviderRuntimeError::InvalidSetting {
            ref advertised, .. }) if advertised.is_empty()),
            "{result:?}"
        );
        assert!(
            result
                .expect_err("ambiguous selection")
                .to_string()
                .contains("unambiguous")
        );
        assert_eq!(
            fixture.requests().len(),
            2,
            "rejected setting must not reach provider"
        );
    }
}

#[tokio::test]
async fn create_refreshes_mode_dependent_model_and_model_dependent_effort_choices() {
    let fixture = WireFixture::start("dynamic", false).await;
    let result = fixture
        .client
        .create_session_with_settings(
            fixture.root.path().into(),
            RequestedProviderSettings {
                mode: Some("ask".into()),
                model: Some("fixture-b".into()),
                effort: Some("high".into()),
            },
        )
        .await;
    fixture.client.shutdown().await;
    let created = result.expect("fresh catalogs admit model then effort");
    assert_eq!(
        created.effective_settings.model.as_deref(),
        Some("fixture-b")
    );
    assert_eq!(created.effective_settings.effort.as_deref(), Some("high"));
    let requests = fixture.requests();
    assert_eq!(
        requests
            .iter()
            .skip(2)
            .map(|request| request["params"]["configId"].as_str().expect("config ID"))
            .collect::<Vec<_>>(),
        ["mode", "model", "effort"]
    );
}

#[tokio::test]
async fn provider_adjusted_companion_values_are_reported_in_full_settings_event() {
    let fixture = WireFixture::start("adjusted", false).await;
    let session = fixture
        .client
        .create_session(fixture.root.path().into())
        .await
        .expect("session");
    let result = fixture
        .client
        .set_setting(session, ProviderSettingKind::Effort, "high".into())
        .await;
    fixture.client.shutdown().await;
    assert_eq!(
        result.expect("provider reports High").effort.as_deref(),
        Some("high")
    );
    let events = fixture.sink.settings.lock().expect("events");
    let reported = events.last().expect("returned settings event");
    for (config_id, value) in [
        ("effort", "high"),
        ("context", "1m"),
        ("fast", "true"),
        ("thinking", "true"),
    ] {
        assert_eq!(
            reported
                .config
                .iter()
                .find(|config| config.id == config_id)
                .expect("reported companion")
                .value,
            ConfigValueState::Select(value.into())
        );
    }
}

#[tokio::test]
async fn load_and_live_update_refresh_effort_identity_and_invalid_values_do_not_dispatch() {
    let fixture = WireFixture::start("load-update", false).await;
    fixture
        .client
        .load_session("fixture-session".into(), fixture.root.path().into())
        .await
        .expect("load session");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let changed = fixture.sink.changed.notified();
            if fixture
                .client
                .settings_catalog("fixture-session")
                .await
                .expect("loaded catalog")
                .config_options
                .iter()
                .any(|option| option.id == "budget")
            {
                break;
            }
            changed.await;
        }
    })
    .await
    .expect("live complete catalog update");
    let rejected = fixture
        .client
        .set_setting(
            "fixture-session".into(),
            ProviderSettingKind::Effort,
            "not-offered".into(),
        )
        .await;
    assert!(matches!(
        rejected,
        Err(ExternalProviderRuntimeError::InvalidSetting { .. })
    ));
    let applied = fixture
        .client
        .set_setting(
            "fixture-session".into(),
            ProviderSettingKind::Effort,
            "high".into(),
        )
        .await;
    fixture.client.shutdown().await;
    assert_eq!(
        applied
            .expect("updated fallback effort applies")
            .effort
            .as_deref(),
        Some("high")
    );
    let requests = fixture.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["method"], "session/load");
    assert_eq!(requests[2]["params"]["configId"], "budget");
    assert_eq!(requests[2]["params"]["value"], "high");
}
