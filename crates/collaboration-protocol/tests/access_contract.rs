use collaboration_protocol::{
    RouterAccess, SettingsObservation, SettingsObservationSource, SettingsUnavailableReason,
};

#[test]
fn observed_and_unavailable_settings_round_trip_without_wire_change() {
    let observed_json = serde_json::json!({
        "kind": "observed",
        "source": "threadStart",
        "observedAt": "2026-09-20T00:00:00Z",
        "routerAccess": "workspace-write",
        "nativeSandbox": {"type":"workspaceWrite"},
        "permissionProfile": {"network":{"enabled":false}},
        "approvalPolicy": "on-request",
        "approvalsReviewer": "auto_review"
    });
    let observed: SettingsObservation =
        serde_json::from_value(observed_json.clone()).expect("observed settings decode");
    assert_eq!(
        serde_json::to_value(observed).expect("observed settings encode"),
        observed_json
    );

    let unavailable = SettingsObservation::Unavailable {
        reason: SettingsUnavailableReason::ThreadReadOmitsSettings,
    };
    assert_eq!(
        serde_json::to_value(unavailable).expect("unavailable settings encode"),
        serde_json::json!({"kind":"unavailable","reason":"threadReadOmitsSettings"})
    );

    let constructed = SettingsObservation::Observed {
        source: SettingsObservationSource::ThreadStart,
        observed_at: "2026-09-20T00:00:00Z".to_owned(),
        router_access: Some(RouterAccess::WorkspaceWrite),
        native_sandbox: None,
        permission_profile: None,
        approval_policy: Box::new(None),
        approvals_reviewer: Box::new(None),
    };
    assert_eq!(
        serde_json::to_value(constructed).expect("constructed settings encode"),
        serde_json::json!({
            "kind":"observed",
            "source":"threadStart",
            "observedAt":"2026-09-20T00:00:00Z",
            "routerAccess":"workspace-write",
            "nativeSandbox":null,
            "permissionProfile":null,
            "approvalPolicy":null,
            "approvalsReviewer":null
        })
    );
}

#[test]
fn settings_observation_rejects_unknown_fields() {
    let error = serde_json::from_value::<SettingsObservation>(serde_json::json!({
        "kind":"unavailable",
        "reason":"threadReadOmitsSettings",
        "invented":true
    }));
    assert!(error.is_err());
}
