use codex_router_keeper_protocol::{
    ChildComponent, ChildDegradation, ChildPhase, ComponentKind, DeactivateReason,
    DeactivateRefusal, EvidenceRejection, MigrationHistoryDefect, MigrationVersion,
    MigrationVersionError, NoGenerationReason, PrepareFailure, PrepareMode, PreparedStoreSchema,
    PreparedStoreSchemaError, RoleHandover, RoleHandoverVersion, RoleHandoverVersionError,
    StoreKind,
};
use serde::{Serialize, de::DeserializeOwned};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn assert_literal_round_trip<TValue>(value: &TValue, literal: &str) -> TestResult
where
    TValue: Serialize + DeserializeOwned + std::fmt::Debug + PartialEq,
{
    let actual = serde_json::to_string(value)?;
    if actual != literal {
        return Err(format!("serialized value mismatch: expected {literal}, got {actual}").into());
    }
    if serde_json::from_str::<TValue>(literal)? != *value {
        return Err(format!("literal did not recover value: {literal}").into());
    }
    Ok(())
}

fn assert_literal_json_semantic_round_trip<TValue>(value: &TValue, literal: &str) -> TestResult
where
    TValue: Serialize + DeserializeOwned + std::fmt::Debug + PartialEq,
{
    let actual_json = serde_json::to_value(value)?;
    let expected_json = serde_json::from_str::<serde_json::Value>(literal)?;
    if actual_json != expected_json {
        return Err(format!(
            "serialized JSON value mismatch: expected {expected_json}, got {actual_json}"
        )
        .into());
    }
    if serde_json::from_str::<TValue>(literal)? != *value {
        return Err(format!("literal did not recover value: {literal}").into());
    }
    Ok(())
}

#[test]
fn lifecycle_unit_variants_match_literal_camel_case_contracts() -> TestResult {
    for (value, literal) in [
        (DeactivateReason::Replacement, "\"replacement\""),
        (DeactivateReason::KeeperFullRestart, "\"keeperFullRestart\""),
        (DeactivateReason::Shutdown, "\"shutdown\""),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    for (value, literal) in [
        (NoGenerationReason::StartupPending, "\"startupPending\""),
        (NoGenerationReason::CurrentExited, "\"currentExited\""),
        (
            NoGenerationReason::RecoveryExhausted,
            "\"recoveryExhausted\"",
        ),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    for (value, literal) in [
        (ChildPhase::Granted, "\"granted\""),
        (ChildPhase::Preparing, "\"preparing\""),
        (ChildPhase::Prepared, "\"prepared\""),
        (ChildPhase::Active, "\"active\""),
        (ChildPhase::Deactivating, "\"deactivating\""),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    for (value, literal) in [
        (StoreKind::ProjectBoard, "\"projectBoard\""),
        (StoreKind::Automation, "\"automation\""),
        (StoreKind::ProviderOperations, "\"providerOperations\""),
        (StoreKind::RouterState, "\"routerState\""),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    for (value, literal) in [
        (MigrationHistoryDefect::DirtyMigration, "\"dirtyMigration\""),
        (
            MigrationHistoryDefect::ChecksumMismatch,
            "\"checksumMismatch\"",
        ),
        (
            MigrationHistoryDefect::InvalidAppliedOrder,
            "\"invalidAppliedOrder\"",
        ),
        (
            MigrationHistoryDefect::UnknownAppliedMigration,
            "\"unknownAppliedMigration\"",
        ),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    for (value, literal) in [
        (
            EvidenceRejection::ExecutableMismatch,
            "\"executableMismatch\"",
        ),
        (EvidenceRejection::DigestMismatch, "\"digestMismatch\""),
        (EvidenceRejection::BundleUnreadable, "\"bundleUnreadable\""),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    for (value, literal) in [
        (ChildComponent::Board, "\"board\""),
        (ChildComponent::Delivery, "\"delivery\""),
        (ChildComponent::Automation, "\"automation\""),
        (ChildComponent::Schedules, "\"schedules\""),
        (ChildComponent::Mcp, "\"mcp\""),
        (ChildComponent::AcpChannel, "\"acpChannel\""),
        (ChildComponent::NativeRelay, "\"nativeRelay\""),
        (ChildComponent::Providers, "\"providers\""),
        (ChildComponent::CodexTurnAdoption, "\"codexTurnAdoption\""),
        (ChildComponent::PooledCredentials, "\"pooledCredentials\""),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    Ok(())
}

#[test]
fn prepare_mode_and_deactivate_refusal_match_literal_payload_contracts() -> TestResult {
    assert_literal_round_trip(&PrepareMode::Fresh, r#"{"type":"fresh"}"#)?;
    assert_literal_round_trip(
        &PrepareMode::Replacement {
            active_degraded: vec![(ChildComponent::Board, ChildDegradation::StoreUnavailable)],
        },
        r#"{"type":"replacement","activeDegraded":[["board",{"type":"storeUnavailable"}]]}"#,
    )?;
    assert_literal_round_trip(
        &DeactivateRefusal::HandoverIncompatible {
            produced: RoleHandoverVersion::try_from(2)?,
            wanted: RoleHandoverVersion::try_from(3)?,
        },
        r#"{"type":"handoverIncompatible","produced":2,"wanted":3}"#,
    )?;
    assert_literal_round_trip(
        &DeactivateRefusal::HandoverTooLarge,
        r#"{"type":"handoverTooLarge"}"#,
    )?;
    Ok(())
}

#[test]
fn prepare_failures_match_every_declared_literal_variant() -> TestResult {
    for (value, literal) in [
        (
            PrepareFailure::StoreOpenFailed,
            r#"{"type":"storeOpenFailed"}"#,
        ),
        (
            PrepareFailure::StoreSchemaNewerThanImage {
                store: StoreKind::RouterState,
            },
            r#"{"type":"storeSchemaNewerThanImage","store":"routerState"}"#,
        ),
        (
            PrepareFailure::StoreMigrationHistoryInvalid {
                store: StoreKind::ProviderOperations,
                reason: MigrationHistoryDefect::ChecksumMismatch,
            },
            r#"{"type":"storeMigrationHistoryInvalid","store":"providerOperations","reason":"checksumMismatch"}"#,
        ),
        (
            PrepareFailure::SecretStoreUnavailable,
            r#"{"type":"secretStoreUnavailable"}"#,
        ),
        (
            PrepareFailure::ListenerGrantInvalid,
            r#"{"type":"listenerGrantInvalid"}"#,
        ),
        (
            PrepareFailure::SchemaEvidenceRejected,
            r#"{"type":"schemaEvidenceRejected"}"#,
        ),
        (PrepareFailure::FrameInvalid, r#"{"type":"frameInvalid"}"#),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    Ok(())
}

#[test]
fn degradation_and_prepared_store_schema_match_literal_contracts() -> TestResult {
    for (value, literal) in [
        (
            ChildDegradation::StoreUnavailable,
            r#"{"type":"storeUnavailable"}"#,
        ),
        (
            ChildDegradation::SchemaMismatch,
            r#"{"type":"schemaMismatch"}"#,
        ),
        (
            ChildDegradation::SchemaUnavailable,
            r#"{"type":"schemaUnavailable"}"#,
        ),
        (
            ChildDegradation::NoCurrentGeneration,
            r#"{"type":"noCurrentGeneration"}"#,
        ),
        (
            ChildDegradation::ProviderUnavailable,
            r#"{"type":"providerUnavailable"}"#,
        ),
        (
            ChildDegradation::AdoptionPending { turns: 4 },
            r#"{"type":"adoptionPending","turns":4}"#,
        ),
        (
            ChildDegradation::CredentialStoreUnavailable,
            r#"{"type":"credentialStoreUnavailable"}"#,
        ),
    ] {
        assert_literal_round_trip(&value, literal)?;
    }
    assert_literal_round_trip(&PreparedStoreSchema::Current, r#"{"type":"current"}"#)?;
    assert_literal_round_trip(
        &PreparedStoreSchema::pending(vec![MigrationVersion::try_from(1_i128)?])?,
        r#"{"type":"pending","migrations":[1]}"#,
    )?;
    assert_literal_round_trip(
        &PreparedStoreSchema::pending(vec![
            MigrationVersion::try_from(1_i128)?,
            MigrationVersion::try_from(9_i128)?,
        ])?,
        r#"{"type":"pending","migrations":[1,9]}"#,
    )?;
    Ok(())
}

#[test]
fn pending_schema_requires_a_nonempty_migration_remainder() -> TestResult {
    if PreparedStoreSchema::pending(Vec::new())
        != Err(PreparedStoreSchemaError::EmptyPendingMigrations)
    {
        return Err("pending constructor accepted an empty migration remainder".into());
    }
    if serde_json::from_str::<PreparedStoreSchema>(r#"{"type":"pending","migrations":[]}"#).is_ok()
    {
        return Err("pending schema deserializer accepted an empty remainder".into());
    }
    let invalid_direct_value = PreparedStoreSchema::Pending {
        migrations: Vec::new(),
    };
    if serde_json::to_string(&invalid_direct_value).is_ok() {
        return Err("pending schema serializer accepted an empty remainder".into());
    }
    Ok(())
}

#[test]
fn role_handover_body_stays_an_opaque_json_value() -> TestResult {
    let version = RoleHandoverVersion::try_from(2)?;
    let handover = RoleHandover {
        role: ComponentKind::AgentCollaborationServices,
        version,
        body: serde_json::json!({
            "owners": [{"turn": 7, "opaque": [null, false, {"v": "owner-defined"}]}],
            "unrecognizedOwnerShape": {"nested": [1, 2, 3]}
        }),
    };
    assert_literal_json_semantic_round_trip(
        &handover,
        r#"{"role":"agentCollaborationServices","version":2,"body":{"owners":[{"opaque":[null,false,{"v":"owner-defined"}],"turn":7}],"unrecognizedOwnerShape":{"nested":[1,2,3]}}}"#,
    )?;

    assert_literal_round_trip(
        &RoleHandover {
            role: ComponentKind::AgentCollaborationServices,
            version,
            body: serde_json::Value::Null,
        },
        r#"{"role":"agentCollaborationServices","version":2,"body":null}"#,
    )?;
    assert_literal_round_trip::<Option<RoleHandover>>(&None, "null")?;
    assert_literal_json_semantic_round_trip(
        &Some(handover),
        r#"{"role":"agentCollaborationServices","version":2,"body":{"owners":[{"opaque":[null,false,{"v":"owner-defined"}],"turn":7}],"unrecognizedOwnerShape":{"nested":[1,2,3]}}}"#,
    )?;
    Ok(())
}

#[test]
fn version_values_reject_zero_negative_overflow_and_noncanonical_json() -> TestResult {
    if RoleHandoverVersion::try_from(1_u64)?.get() != 1 {
        return Err("role handover version did not preserve one".into());
    }
    let maximum_handover_version = RoleHandoverVersion::try_from(u64::from(u32::MAX))?;
    if maximum_handover_version.get() != u32::MAX {
        return Err("maximum role handover version changed".into());
    }
    assert_literal_round_trip(&maximum_handover_version, "4294967295")?;
    if RoleHandoverVersion::try_from(0) != Err(RoleHandoverVersionError::NotPositive) {
        return Err("zero role handover version was not rejected".into());
    }
    if RoleHandoverVersion::try_from(u64::from(u32::MAX) + 1)
        != Err(RoleHandoverVersionError::Overflow)
    {
        return Err("overflowing role handover version was not rejected".into());
    }
    for literal in ["0", "-1", "1.0", "\"1\"", "4294967296"] {
        if serde_json::from_str::<RoleHandoverVersion>(literal).is_ok() {
            return Err(format!("noncanonical role handover version accepted: {literal}").into());
        }
    }
    if MigrationVersion::try_from(1_i128)?.get() != 1 {
        return Err("migration version did not preserve one".into());
    }
    let maximum_migration_version = MigrationVersion::try_from(i128::from(i64::MAX))?;
    if maximum_migration_version.get() != i64::MAX {
        return Err("maximum migration version changed".into());
    }
    assert_literal_round_trip(&maximum_migration_version, "9223372036854775807")?;
    if MigrationVersion::try_from(0_i128) != Err(MigrationVersionError::NotPositive) {
        return Err("zero migration version was not rejected".into());
    }
    if MigrationVersion::try_from(-1_i128) != Err(MigrationVersionError::NotPositive) {
        return Err("negative migration version was not rejected".into());
    }
    if MigrationVersion::try_from(i128::from(i64::MAX) + 1) != Err(MigrationVersionError::Overflow)
    {
        return Err("overflowing migration version was not rejected".into());
    }
    for literal in ["0", "-1", "1.0", "\"1\"", "9223372036854775808"] {
        if serde_json::from_str::<MigrationVersion>(literal).is_ok() {
            return Err(format!("noncanonical migration version accepted: {literal}").into());
        }
    }
    Ok(())
}

#[test]
fn enum_tags_and_struct_fields_reject_unknown_contract_data() {
    assert!(serde_json::from_str::<PrepareMode>(r#"{"type":"unknown"}"#).is_err());
    assert!(serde_json::from_str::<PrepareMode>(r#"{"type":"fresh","extra":1}"#).is_err());
    assert!(serde_json::from_str::<PrepareFailure>(r#"{"type":"unknown"}"#).is_err());
    assert!(
        serde_json::from_str::<PrepareFailure>(r#"{"type":"frameInvalid","extra":1}"#).is_err()
    );
    assert!(
        serde_json::from_str::<ChildDegradation>(
            r#"{"type":"adoptionPending","turns":1,"extra":2}"#
        )
        .is_err()
    );
    assert!(serde_json::from_str::<DeactivateRefusal>(r#"{"type":"unknown"}"#).is_err());
    assert!(
        serde_json::from_str::<DeactivateRefusal>(r#"{"type":"handoverTooLarge","extra":1}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<PreparedStoreSchema>(
            r#"{"type":"pending","migrations":[],"extra":1}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<RoleHandover>(
            r#"{"role":"agentCollaborationServices","version":1,"body":null,"extra":1}"#
        )
        .is_err()
    );
}
