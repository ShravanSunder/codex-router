use provider_link_protocol::{
    EventSeq, EventSeqError, InteractionId, LinkEpoch, LinkIdentityError, LinkRequestId,
    LinkRequestIdError, LinkRole, ProviderHostIncarnation, ProviderId, ProviderIdError,
};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const CANONICAL_UUID: &str = "a1b2c3d4-1111-4111-8111-222222222222";
const UUID_WIRE: &str = r#""a1b2c3d4-1111-4111-8111-222222222222""#;

fn uuid_encoding_contract<TIdentity>() -> TestResult
where
    TIdentity: TryFrom<String, Error = LinkIdentityError>
        + TryFrom<Uuid, Error = LinkIdentityError>
        + Into<String>
        + Serialize
        + for<'de> Deserialize<'de>
        + Copy
        + Eq
        + Debug,
{
    for input in [
        CANONICAL_UUID,
        "A1B2C3D4-1111-4111-8111-222222222222",
        "urn:uuid:a1b2c3d4-1111-4111-8111-222222222222",
    ] {
        let identity = TIdentity::try_from(input.to_owned())?;
        let canonical: String = identity.into();
        if canonical != CANONICAL_UUID
            || serde_json::to_string(&identity)? != UUID_WIRE
            || serde_json::from_str::<TIdentity>(UUID_WIRE)? != identity
            || TIdentity::try_from(Uuid::parse_str(CANONICAL_UUID)?)? != identity
        {
            return Err("UUID canonical encoding or validated reconstruction differs".into());
        }
    }
    Ok(())
}

fn uuid_rejection_contract<TIdentity>() -> TestResult
where
    TIdentity: TryFrom<String, Error = LinkIdentityError>
        + TryFrom<Uuid, Error = LinkIdentityError>
        + for<'de> Deserialize<'de>,
{
    for input in [
        "",
        " ",
        "not-a-uuid",
        "z1b2c3d4-1111-4111-8111-222222222222",
    ] {
        if !matches!(
            TIdentity::try_from(input.to_owned()),
            Err(LinkIdentityError::MalformedUuid)
        ) {
            return Err(format!("malformed UUID was not rejected: {input:?}").into());
        }
    }
    if !matches!(
        TIdentity::try_from(Uuid::nil()),
        Err(LinkIdentityError::NilUuid)
    ) || !matches!(
        TIdentity::try_from("00000000-0000-0000-0000-000000000000".to_owned()),
        Err(LinkIdentityError::NilUuid)
    ) {
        return Err("nil UUID construction did not return the typed refusal".into());
    }
    for json in [
        r#""""#,
        r#""00000000-0000-0000-0000-000000000000""#,
        "0",
        "null",
        "true",
        "[]",
        "{}",
    ] {
        if serde_json::from_str::<TIdentity>(json).is_ok() {
            return Err(format!("UUID JSON boundary accepted {json}").into());
        }
    }
    Ok(())
}

#[test]
fn three_uuid_domains_preserve_existing_canonical_encoding() -> TestResult {
    uuid_encoding_contract::<ProviderHostIncarnation>()?;
    uuid_encoding_contract::<LinkEpoch>()?;
    uuid_encoding_contract::<InteractionId>()
}

#[test]
fn three_uuid_domains_reject_malformed_nil_and_wrong_json_shapes() -> TestResult {
    uuid_rejection_contract::<ProviderHostIncarnation>()?;
    uuid_rejection_contract::<LinkEpoch>()?;
    uuid_rejection_contract::<InteractionId>()
}

#[test]
fn fresh_domain_identities_are_nonnil_v7_without_restricting_input_versions() {
    let incarnation = ProviderHostIncarnation::fresh();
    let epoch = LinkEpoch::fresh();
    let interaction = InteractionId::fresh();
    for uuid in [
        incarnation.as_uuid(),
        epoch.as_uuid(),
        interaction.as_uuid(),
    ] {
        assert!(!uuid.is_nil());
        assert_eq!(uuid.get_version_num(), 7);
    }
}

#[test]
fn event_sequence_has_exact_positive_uint_encoding_and_checked_exhaustion() -> TestResult {
    let mut sequence = EventSeq::try_from(u64::MAX - 1)?;
    if sequence.advance()?.get() != u64::MAX {
        return Err("event sequence did not reach the maximum".into());
    }
    let before = sequence;
    if !matches!(sequence.advance(), Err(EventSeqError::Exhausted))
        || sequence != before
        || serde_json::to_string(&sequence)? != "18446744073709551615"
        || serde_json::from_str::<EventSeq>("18446744073709551615")? != sequence
        || serde_json::to_string(&EventSeq::try_from(1)?)? != "1"
        || !matches!(EventSeq::try_from(0), Err(EventSeqError::Zero))
    {
        return Err("positive event sequence encoding/exhaustion invariant differs".into());
    }
    Ok(())
}

#[test]
fn event_sequence_rejects_zero_negative_overflow_and_noninteger_json() {
    for json in [
        "0",
        "-1",
        "18446744073709551616",
        "1.5",
        r#""1""#,
        "null",
        "true",
        "[]",
        "{}",
    ] {
        assert!(
            serde_json::from_str::<EventSeq>(json).is_err(),
            "accepted {json}"
        );
    }
}

#[test]
fn request_id_includes_zero_and_the_entire_uint_domain() -> TestResult {
    for (value, wire) in [(0, "0"), (1, "1"), (u64::MAX, "18446744073709551615")] {
        let request = LinkRequestId::new(value);
        if request.get() != value
            || serde_json::to_string(&request)? != wire
            || serde_json::from_str::<LinkRequestId>(wire)? != request
            || u64::from(request) != value
        {
            return Err(format!("full-domain request identity differs at {value}").into());
        }
    }
    Ok(())
}

#[test]
fn request_id_advancement_never_wraps_or_mutates_on_exhaustion() -> TestResult {
    let mut request = LinkRequestId::new(0);
    if request.advance()?.get() != 1 {
        return Err("zero request identity did not advance to one".into());
    }
    let mut request = LinkRequestId::new(u64::MAX - 1);
    if request.advance()?.get() != u64::MAX {
        return Err("request identity did not reach the maximum".into());
    }
    let before = request;
    if !matches!(request.advance(), Err(LinkRequestIdError::Exhausted)) || request != before {
        return Err("request identity wrapped or mutated on exhaustion".into());
    }
    for json in [
        "-1",
        "18446744073709551616",
        "1.5",
        r#""0""#,
        "null",
        "true",
        "[]",
        "{}",
    ] {
        if serde_json::from_str::<LinkRequestId>(json).is_ok() {
            return Err(format!("request JSON boundary accepted {json}").into());
        }
    }
    Ok(())
}

#[test]
fn provider_identifier_preserves_supplied_bytes_without_membership_policy() -> TestResult {
    let provider = ProviderId::new("  CuRsOr/β  ")?;
    if provider.as_str() != "  CuRsOr/β  "
        || serde_json::to_string(&provider)? != r#""  CuRsOr/β  ""#
        || serde_json::from_str::<ProviderId>(r#""  CuRsOr/β  ""#)? != provider
    {
        return Err("provider name bytes or literal serialization changed".into());
    }
    for supplied in [" ", "\t", "unknown-provider", "a\nb", "\0"] {
        if ProviderId::try_from(supplied.to_owned())?.as_str() != supplied {
            return Err(format!("provider name was normalized: {supplied:?}").into());
        }
    }
    Ok(())
}

#[test]
fn provider_identifier_rejects_only_empty_strings_and_wrong_json_types() {
    assert!(matches!(ProviderId::new(""), Err(ProviderIdError::Empty)));
    for json in [r#""""#, "0", "null", "true", "[]", "{}"] {
        assert!(
            serde_json::from_str::<ProviderId>(json).is_err(),
            "accepted {json}"
        );
    }
}

#[test]
fn link_roles_use_exact_camel_case_unit_values() -> TestResult {
    for (role, wire) in [
        (LinkRole::Standby, r#""standby""#),
        (LinkRole::Active, r#""active""#),
    ] {
        if serde_json::to_string(&role)? != wire || serde_json::from_str::<LinkRole>(wire)? != role
        {
            return Err("LinkRole literal unit encoding differs".into());
        }
    }
    for json in [
        r#""Standby""#,
        r#""Active""#,
        r#""standBy""#,
        r#""unknown""#,
        "null",
        "0",
        r#"{"role":"active"}"#,
    ] {
        if serde_json::from_str::<LinkRole>(json).is_ok() {
            return Err(format!("LinkRole JSON boundary accepted {json}").into());
        }
    }
    Ok(())
}
