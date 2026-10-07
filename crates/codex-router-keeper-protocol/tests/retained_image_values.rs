use codex_router_keeper_protocol::{BuildInfo, ComponentFingerprint, ComponentKind, SlotImage};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn fingerprints_have_exact_byte_and_hex_boundaries() -> TestResult {
    let literal = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let fingerprint = ComponentFingerprint::from_hex(literal)?;
    if fingerprint.to_hex() != literal
        || fingerprint.as_bytes()[0] != 1
        || fingerprint.as_bytes()[31] != 239
    {
        return Err("literal digest encoding changed".into());
    }
    if ComponentFingerprint::from_bytes(&[1; 31]).is_ok()
        || ComponentFingerprint::from_bytes(&[1; 33]).is_ok()
    {
        return Err("fingerprint length accepted".into());
    }
    for invalid in ["", "abc", &"g".repeat(64)] {
        if ComponentFingerprint::from_hex(invalid).is_ok() {
            return Err("invalid hex accepted".into());
        }
    }
    if serde_json::to_string(&fingerprint)? != format!("\"{literal}\"") {
        return Err("fingerprint wire changed".into());
    }
    Ok(())
}
#[test]
fn all_four_roles_and_full_build_info_are_required() -> TestResult {
    for (kind, wire) in [
        (ComponentKind::Keeper, "keeper"),
        (
            ComponentKind::AgentCollaborationServices,
            "agentCollaborationServices",
        ),
        (ComponentKind::AgentProxyServices, "agentProxyServices"),
        (
            ComponentKind::AgentProviderServices,
            "agentProviderServices",
        ),
    ] {
        if serde_json::to_string(&kind)? != format!("\"{wire}\"") {
            return Err("role wire changed".into());
        }
    }
    let digest = "11".repeat(32);
    let wire = format!(
        r#"{{"packageVersion":"1.2.3-beta.1+build.9","fingerprints":{{"keeper":"{digest}","agentCollaborationServices":"2222222222222222222222222222222222222222222222222222222222222222","agentProxyServices":"3333333333333333333333333333333333333333333333333333333333333333","agentProviderServices":"4444444444444444444444444444444444444444444444444444444444444444"}}}}"#
    );
    let info: BuildInfo = serde_json::from_str(&wire)?;
    if info.fingerprints.keeper.as_bytes().first() != Some(&0x11)
        || info
            .fingerprints
            .agent_collaboration_services
            .as_bytes()
            .first()
            != Some(&0x22)
        || info.fingerprints.agent_proxy_services.as_bytes().first() != Some(&0x33)
        || info.fingerprints.agent_provider_services.as_bytes().first() != Some(&0x44)
    {
        return Err("four literal component fields were conflated".into());
    }
    if info.package_version.to_string() != "1.2.3-beta.1+build.9" {
        return Err("semver metadata lost".into());
    }
    for invalid in [
        wire.replace("1.2.3-beta.1+build.9", "1.2"),
        wire.replace(
            &format!(",\"agentProviderServices\":\"{}\"", "44".repeat(32)),
            "",
        ),
        wire.replace(&digest, &"g".repeat(64)),
    ] {
        if serde_json::from_str::<BuildInfo>(&invalid).is_ok() {
            return Err("invalid build-info accepted".into());
        }
    }
    Ok(())
}
#[test]
fn slot_records_validate_structure_without_opening_missing_files() -> TestResult {
    let record = SlotImage::new(
        "/private/tmp/does-not-exist/retained-router".into(),
        [7; 32],
        10,
        20,
    )?;
    let wire = serde_json::to_string(&record)?;
    let reconstructed: SlotImage = serde_json::from_str(&wire)?;
    if record != reconstructed
        || record.file_sha256() != &[7; 32]
        || record.device() != 10
        || record.inode() != 20
    {
        return Err("pure record changed".into());
    }
    for invalid in [
        "relative",
        "/",
        "/private/../tmp/file",
        "/private//tmp/file",
        "/private/tmp/file/",
    ] {
        if SlotImage::new(invalid.into(), [7; 32], 10, 20).is_ok() {
            return Err("invalid recorded path accepted".into());
        }
    }
    if serde_json::from_str::<SlotImage>(&wire.replace("[7,7,7", "[7,7")).is_ok() {
        return Err("short digest wire accepted".into());
    }
    Ok(())
}
