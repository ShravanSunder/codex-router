use communication_protocol::CodexGeneration;
use communication_service::NativeGenerationGate;

#[test]
fn retirement_cancels_existing_admission_and_rejects_new_connections() {
    let gate = NativeGenerationGate::default();
    assert!(gate.acquire().is_err());
    let generation: CodexGeneration = serde_json::from_str(
        r#"{"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}"#,
    )
    .unwrap_or_else(|e| panic!("generation: {e}"));
    gate.activate(
        generation.clone(),
        "/tmp/native-owner/backend.sock".into(),
        None,
    )
    .unwrap_or_else(|e| panic!("activate: {e}"));
    let admitted = gate.acquire().unwrap_or_else(|e| panic!("admit: {e}"));
    assert_eq!(admitted.generation(), &generation);
    assert!(!admitted.retirement().is_cancelled());
    gate.retire().unwrap_or_else(|e| panic!("retire: {e}"));
    assert!(admitted.retirement().is_cancelled());
    assert!(gate.acquire().is_err());
    assert!(
        gate.activate(generation, "/tmp/native-owner/backend.sock".into(), None)
            .is_err()
    );
}
