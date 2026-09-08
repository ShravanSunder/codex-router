use agent_automation::ScheduleId;

#[test]
fn preserves_exact_valid_identity_in_serialized_requests() -> Result<(), Box<dyn std::error::Error>>
{
    // Arrange: canonical v7, RFC variant, supplied by another client.
    let text = "019f0000-0000-7000-8000-000000000001";
    // Act: parse and serialize through the domain type.
    let identity = ScheduleId::try_from(text.to_owned())?;
    let encoded = serde_json::to_string(&identity)?;
    let decoded: ScheduleId = serde_json::from_str(&encoded)?;
    // Assert: identity is preserved, never silently replaced.
    if decoded.as_str() != text {
        return Err("serialized identity changed".into());
    }
    Ok(())
}

#[test]
fn rejects_wrong_version_variant_and_noncanonical_spelling() {
    // Arrange: values must not become persistent automation identities.
    let invalid = [
        "019f0000-0000-4000-8000-000000000001",
        "019f0000-0000-7000-c000-000000000001",
        "019F0000-0000-7000-8000-000000000001",
        "019f0000000070008000000000000001",
        "not-an-id",
    ];
    // Act / Assert: parsing rejects rather than normalizing or generating an ID.
    for value in invalid {
        assert!(
            ScheduleId::try_from(value.to_owned()).is_err(),
            "accepted {value}"
        );
    }
}

#[test]
fn deserialization_cannot_bypass_identity_validation() {
    // Arrange: a human-readable wire value with a non-v7 identity.
    let wire = "\"019f0000-0000-4000-8000-000000000001\"";
    // Act / Assert: serde uses the same validation boundary.
    assert!(serde_json::from_str::<ScheduleId>(wire).is_err());
}
