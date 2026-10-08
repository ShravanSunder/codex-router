use codex_router_keeper_protocol::{
    ChildComponent, ChildDegradation, ChildPhase, ChildSnapshot, ChildSnapshotError,
    ComponentFingerprint, GenerationId, ListenerKind,
};
use serde::{Serialize, de::DeserializeOwned};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GENERATION: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":7}"#;

fn fingerprint() -> Result<ComponentFingerprint, codex_router_keeper_protocol::FingerprintError> {
    ComponentFingerprint::from_hex(FINGERPRINT)
}

fn generation() -> Result<GenerationId, serde_json::Error> {
    serde_json::from_str(GENERATION)
}

fn assert_literal_round_trip<TValue>(value: &TValue, literal: &str) -> TestResult
where
    TValue: Serialize + DeserializeOwned + std::fmt::Debug + PartialEq,
{
    let actual = serde_json::to_string(value)?;
    if actual != literal {
        return Err(
            format!("serialized snapshot mismatch: expected {literal}, got {actual}").into(),
        );
    }
    if serde_json::from_str::<TValue>(literal)? != *value {
        return Err(format!("snapshot literal did not recover value: {literal}").into());
    }
    Ok(())
}

#[test]
fn complete_snapshot_literal_preserves_tuple_identities_and_optional_values() -> TestResult {
    let snapshot = ChildSnapshot::new(
        ChildPhase::Granted,
        fingerprint()?,
        None,
        Some(ListenerKind::ProxyHttp),
        vec![
            (ChildComponent::Board, ChildDegradation::StoreUnavailable),
            (
                ChildComponent::CodexTurnAdoption,
                ChildDegradation::AdoptionPending { turns: 3 },
            ),
        ],
    )?;
    assert_literal_round_trip(
        &snapshot,
        r#"{"phase":"granted","fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","committedGeneration":null,"pendingListenerRequest":{"kind":"proxyHttp"},"degraded":[["board",{"type":"storeUnavailable"}],["codexTurnAdoption",{"type":"adoptionPending","turns":3}]]}"#,
    )?;
    Ok(())
}

#[test]
fn preparing_snapshot_literal_preserves_present_generation_and_listener() -> TestResult {
    let snapshot = ChildSnapshot::new(
        ChildPhase::Preparing,
        fingerprint()?,
        Some(generation()?),
        Some(ListenerKind::RouterSessionFace {
            endpoint: collaboration_protocol::EndpointId::try_from("claude-local".to_owned())?,
        }),
        Vec::new(),
    )?;
    assert_literal_round_trip(
        &snapshot,
        r#"{"phase":"preparing","fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","committedGeneration":{"epoch":"11111111-2222-4333-8444-555555555555","number":7},"pendingListenerRequest":{"kind":"routerSessionFace","endpoint":"claude-local"},"degraded":[]}"#,
    )?;
    Ok(())
}

#[test]
fn pending_listener_request_is_allowed_only_for_granted_and_preparing() -> TestResult {
    let fingerprint = fingerprint()?;
    for phase in [ChildPhase::Granted, ChildPhase::Preparing] {
        for pending_listener_request in [None, Some(ListenerKind::ProxyHttp)] {
            let snapshot = ChildSnapshot::new(
                phase,
                fingerprint,
                None,
                pending_listener_request,
                Vec::new(),
            )?;
            if snapshot.committed_generation.is_some() {
                return Err("absent committed generation was not preserved".into());
            }
        }
    }
    for phase in [
        ChildPhase::Prepared,
        ChildPhase::Active,
        ChildPhase::Deactivating,
    ] {
        let snapshot = ChildSnapshot::new(phase, fingerprint, None, None, Vec::new())?;
        if snapshot.committed_generation.is_some() {
            return Err("absent committed generation was not preserved".into());
        }
        if ChildSnapshot::new(
            phase,
            fingerprint,
            None,
            Some(ListenerKind::ProxyHttp),
            Vec::new(),
        ) != Err(ChildSnapshotError::PendingListenerRequestOutsidePrepare)
        {
            return Err("pending listener request was accepted outside prepare".into());
        }
    }
    Ok(())
}

#[test]
fn serde_rejects_pending_listener_outside_prepare_and_unknown_snapshot_fields() {
    let invalid = r#"{"phase":"active","fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","committedGeneration":null,"pendingListenerRequest":{"kind":"proxyHttp"},"degraded":[]}"#;
    assert!(serde_json::from_str::<ChildSnapshot>(invalid).is_err());
    let unknown = r#"{"phase":"active","fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","committedGeneration":null,"pendingListenerRequest":null,"degraded":[],"provider":"unexpected"}"#;
    assert!(serde_json::from_str::<ChildSnapshot>(unknown).is_err());
}

#[test]
fn serializer_rejects_invalid_public_field_combinations() -> TestResult {
    let snapshot = ChildSnapshot {
        phase: ChildPhase::Prepared,
        fingerprint: fingerprint()?,
        committed_generation: None,
        pending_listener_request: Some(ListenerKind::ProxyHttp),
        degraded: Vec::new(),
    };
    if serde_json::to_string(&snapshot).is_ok() {
        return Err("serializer accepted a pending request outside prepare".into());
    }
    Ok(())
}
