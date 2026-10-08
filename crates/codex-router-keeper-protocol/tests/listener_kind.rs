use codex_router_keeper_protocol::ListenerKind;
use collaboration_protocol::EndpointId;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn exact_listener_variants_and_existing_endpoint_roundtrip() -> TestResult {
    for (kind, wire) in [
        (
            ListenerKind::CollaborationControl,
            r#"{"kind":"collaborationControl"}"#,
        ),
        (ListenerKind::NativeRelay, r#"{"kind":"nativeRelay"}"#),
        (ListenerKind::AcpChannel, r#"{"kind":"acpChannel"}"#),
        (ListenerKind::McpHttp, r#"{"kind":"mcpHttp"}"#),
        (ListenerKind::ProxyHttp, r#"{"kind":"proxyHttp"}"#),
        (ListenerKind::ProviderLink, r#"{"kind":"providerLink"}"#),
        (
            ListenerKind::RouterSessionFace {
                endpoint: EndpointId::try_from("claude-local".to_owned())?,
            },
            r#"{"kind":"routerSessionFace","endpoint":"claude-local"}"#,
        ),
    ] {
        if (serde_json::to_string(&kind)?) != (wire) {
            return Err("listener_kind scenario assertion failed".into());
        }
        if (serde_json::from_str::<ListenerKind>(wire)?) != (kind) {
            return Err("listener_kind scenario assertion failed".into());
        }
    }
    Ok(())
}
#[test]
fn malformed_endpoint_and_unknown_listener_shapes_are_rejected() {
    for endpoint in ["", "Claude-local", "../escape", "1-local", &"a".repeat(65)] {
        let wire = serde_json::json!({"kind":"routerSessionFace", "endpoint":endpoint});
        assert!(serde_json::from_value::<ListenerKind>(wire).is_err());
    }
    for wire in [
        r#"{"kind":"operator"}"#,
        r#"{"kind":"routerSessionFace"}"#,
        r#"{"kind":"proxyHttp","endpoint":"claude-local"}"#,
    ] {
        assert!(serde_json::from_str::<ListenerKind>(wire).is_err());
    }
}
