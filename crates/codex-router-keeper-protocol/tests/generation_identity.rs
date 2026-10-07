use codex_router_keeper_protocol::{GenerationId, GenerationNumber, KeeperEpoch};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn generation_wire_roundtrip_preserves_epoch_and_nonzero_number() -> TestResult {
    let literal = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":7}"#;
    let generation: GenerationId = serde_json::from_str(literal)?;
    if generation.number.get() != 7 || serde_json::to_string(&generation)? != literal {
        return Err("generation wire changed identity".into());
    }
    let fresh = KeeperEpoch::fresh();
    if fresh.as_uuid().is_nil() {
        return Err("fresh epoch was nil".into());
    }
    Ok(())
}
#[test]
fn malformed_nil_zero_and_unknown_identity_fields_are_rejected() {
    for wire in [
        r#"{"epoch":"00000000-0000-0000-0000-000000000000","number":1}"#,
        r#"{"epoch":"invalid","number":1}"#,
        r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":0}"#,
        r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":1,"role":"keeper"}"#,
    ] {
        assert!(serde_json::from_str::<GenerationId>(wire).is_err());
    }
}
#[test]
fn generation_advance_checks_exhaustion_without_changing_last_number() -> TestResult {
    let mut number = GenerationNumber::try_from(7)?;
    if number.advance()?.get() != 8 {
        return Err("generation failed to advance".into());
    }
    let mut last = GenerationNumber::try_from(u64::MAX)?;
    if last.advance().is_ok() || last.get() != u64::MAX {
        return Err("generation exhaustion changed identity".into());
    }
    if GenerationNumber::try_from(0).is_ok() {
        return Err("zero generation accepted".into());
    }
    Ok(())
}
