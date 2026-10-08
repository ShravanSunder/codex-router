//! Feature modules request delivery through the seam, never a concrete client route.

#[test]
fn message_features_do_not_import_client_routes() {
    let features = [
        (
            "message send",
            include_str!("../src/collaboration_application/message_operations.rs"),
        ),
        (
            "timed wake",
            include_str!("../src/wakeup_delivery_sender.rs"),
        ),
        (
            "subscription push",
            include_str!("../src/subscription_delivery/subscription_push.rs"),
        ),
        (
            "interaction notice",
            include_str!("../src/interaction_broker.rs"),
        ),
        (
            "subscription admission",
            include_str!("../src/collaboration_application/board_operations.rs"),
        ),
        (
            "direct message push",
            include_str!("../src/subscription_delivery/direct_message_push.rs"),
        ),
        (
            "scheduled run worker",
            include_str!("../src/scheduled_run_worker.rs"),
        ),
        (
            "run reconciliation",
            include_str!("../src/run_reconciliation.rs"),
        ),
        (
            "schedule activation",
            include_str!("../src/schedule_activation.rs"),
        ),
        (
            "schedule preparation",
            include_str!("../src/schedule_preparation.rs"),
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
