//! Feature operations that moved from Control dispatchers into the application service still
//! request delivery through the seam, never a concrete client route.

#[test]
fn application_message_features_do_not_import_client_routes() {
    let features = [
        (
            "message operations",
            include_str!("../src/collaboration_application/message_operations.rs"),
        ),
        (
            "board and subscription operations",
            include_str!("../src/collaboration_application/board_operations.rs"),
        ),
        (
            "wake operations",
            include_str!("../src/collaboration_application/wake_operations.rs"),
        ),
        (
            "schedule operations",
            include_str!("../src/collaboration_application/schedule_operations.rs"),
        ),
    ];
    for (feature, source) in features {
        for forbidden in [
            "native_message_dispatch",
            "codex_app_server_delivery_route",
            "provider_acp_delivery_route",
            "claude_code_peer_delivery_route",
            "codex_native_integration",
            "NativeOperation",
            "codex/messageSend",
            "codex-local",
            "claude-local",
            "cursor-local",
        ] {
            assert!(
                !source.contains(forbidden),
                "{feature} depends on {forbidden}"
            );
        }
    }
}
