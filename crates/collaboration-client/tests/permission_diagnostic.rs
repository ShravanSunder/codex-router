use collaboration_client::ClientError;

#[test]
fn denied_discovery_has_permission_diagnostic() {
    let error = ClientError::Discovery {
        stage: "socket-connect",
        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    };

    let diagnostic = error
        .permission_diagnostic()
        .unwrap_or_else(|| panic!("permission diagnostic"));

    assert_eq!(
        serde_json::to_value(diagnostic).unwrap_or_else(|error| panic!("encode: {error}")),
        serde_json::json!({
            "kind": "permissionDenied",
            "stage": "socketConnect",
            "message": "Request automated approval review through your tool, or ask the user to grant the required command/socket access. Retry only after access is granted.",
            "nextAction": "requestApproval"
        })
    );
}

#[test]
fn absent_and_refused_discovery_have_no_permission_diagnostic() {
    for kind in [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::ConnectionRefused,
    ] {
        let error = ClientError::Discovery {
            stage: "socket-connect",
            source: std::io::Error::from(kind),
        };
        assert!(error.permission_diagnostic().is_none());
    }
}

#[test]
fn post_discovery_transport_denial_has_no_permission_diagnostic() {
    let error = ClientError::Transport(std::io::Error::from(std::io::ErrorKind::PermissionDenied));

    assert!(error.permission_diagnostic().is_none());
}
