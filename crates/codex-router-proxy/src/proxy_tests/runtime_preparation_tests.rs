use super::*;
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener};

fn candidate_config(root: &Path, address: LoopbackBindAddress) -> LoopbackRouterRuntimeConfig {
    LoopbackRouterRuntimeConfig::new_tokenless(
        address,
        UpstreamEndpoint::new("http://127.0.0.1:1/v1").expect("fixture upstream"),
        root.join("state.sqlite"),
        root.join("secrets"),
    )
}

#[tokio::test]
async fn replacement_prepare_preserves_absent_state_database() {
    let root = tempfile::tempdir().expect("isolated fixture root");
    let config = candidate_config(
        root.path(),
        LoopbackBindAddress::new("127.0.0.1", 0).expect("loopback"),
    );
    let state_path = root.path().join("state.sqlite");
    let credentials = codex_router_secret_store::test_support::open_encrypted_credential_store(
        root.path().join("secrets"),
    )
    .expect("external Keychain stand-in");
    assert!(!state_path.exists());
    let affinity = load_or_create_router_affinity_hash_secret(&credentials)
        .expect("fixture affinity")
        .secret()
        .clone();
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp(config.bind_address().socket_addr(), gate)
        .await
        .expect("fixture granted listener");
    let candidate = LoopbackRouterRuntime::prepare(config, credentials, affinity, listener, gate)
        .await
        .expect("candidate");
    drop(candidate);
    eprintln!(
        "preparation snapshot: state_before=absent state_after_exists={}",
        state_path.exists()
    );
    assert!(
        !state_path.exists(),
        "Prepare must not create or migrate shared state"
    );
}

#[tokio::test]
async fn supplied_listener_preparation_does_not_rebind_held_port() {
    let root = tempfile::tempdir().expect("isolated fixture root");
    let supplier = TcpListener::bind("127.0.0.1:0").expect("keeper-style held listener");
    let address = supplier.local_addr().expect("bound address");
    let config = candidate_config(
        root.path(),
        LoopbackBindAddress::new("127.0.0.1", address.port()).expect("loopback"),
    );
    let credentials = codex_router_secret_store::test_support::open_encrypted_credential_store(
        root.path().join("secrets"),
    )
    .expect("external Keychain stand-in");
    let affinity = load_or_create_router_affinity_hash_secret(&credentials)
        .expect("fixture affinity")
        .secret()
        .clone();
    let gate = DescriptorGate::global();
    let supplied_fd = gate
        .duplicate(std::os::fd::AsFd::as_fd(&supplier))
        .await
        .expect("gated duplicate");
    let listener = OwnedListener::from_tcp_owned(supplied_fd, address, gate)
        .await
        .expect("supplied association");
    let candidate =
        LoopbackRouterRuntime::prepare(config, credentials, affinity, listener, gate).await;
    eprintln!(
        "held listener preparation: address={address} candidate_error={:?}",
        candidate.as_ref().err()
    );
    if let Ok(runtime) = &candidate {
        assert_eq!(runtime.local_addr(), address);
    }
    assert!(
        candidate.is_ok(),
        "candidate must adopt the supplied listener instead of rebinding"
    );
    assert_eq!(supplier.local_addr().expect("supplier survives"), address);
}
