use codex_router_keeper::validate_native_probe_launch;
use codex_router_keeper_protocol::{
    BuildInfo, ComponentFingerprint, ComponentFingerprints, ComponentKind,
};
#[test]
fn native_receiver_refuses_wrong_role_and_projected_fingerprint() {
    let fingerprint = ComponentFingerprint::from_bytes(&[0x11; 32]).unwrap();
    let different = ComponentFingerprint::from_bytes(&[0x22; 32]).unwrap();
    let compiled = BuildInfo {
        package_version: "1.2.3".parse().unwrap(),
        fingerprints: ComponentFingerprints {
            keeper: fingerprint,
            agent_collaboration_services: fingerprint,
            agent_proxy_services: fingerprint,
            agent_provider_services: fingerprint,
        },
    };
    assert!(validate_native_probe_launch(ComponentKind::Keeper, fingerprint, &compiled).is_ok());
    assert!(validate_native_probe_launch(ComponentKind::Keeper, different, &compiled).is_err());
    assert!(
        validate_native_probe_launch(ComponentKind::AgentProxyServices, fingerprint, &compiled)
            .is_err()
    );
}
