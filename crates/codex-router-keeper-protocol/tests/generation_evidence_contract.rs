use codex_router_keeper_protocol::{
    GenerationCurrentPayload, GenerationEvidence, GenerationEvidenceError, GenerationPreparation,
    GenerationSchemaAvailability,
};
use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const READY_EVIDENCE: &str = r#"{"executable":{"recorded_path":"/absent/codex-router","content_digest":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31]},"schema":{"availability":"ready","schema_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","schema_bundle_dir":"/absent/schema-bundle"}}"#;
const UNAVAILABLE_EVIDENCE: &str = r#"{"executable":{"recorded_path":"/absent/codex-router","content_digest":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31]},"schema":{"availability":"unavailable","reason":"exportFailed"}}"#;
const CURRENT_PAYLOAD: &str = r#"{"generation":{"epoch":"11111111-2222-4333-8444-555555555555","number":7},"alias":"/run/codex/gen-11111111-7.sock","evidence":{"executable":{"recorded_path":"/absent/codex-router","content_digest":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31]},"schema":{"availability":"ready","schema_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","schema_bundle_dir":"/absent/schema-bundle"}},"server_display_name":null}"#;
const CURRENT_PAYLOAD_WITH_NAME: &str = r#"{"generation":{"epoch":"11111111-2222-4333-8444-555555555555","number":7},"alias":"/run/codex/gen-11111111-7.sock","evidence":{"executable":{"recorded_path":"/absent/codex-router","content_digest":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31]},"schema":{"availability":"ready","schema_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","schema_bundle_dir":"/absent/schema-bundle"}},"server_display_name":"north\n\"east\""}"#;

#[test]
fn generation_evidence_preserves_literal_ready_and_unavailable_shapes() -> TestResult {
    for literal in [READY_EVIDENCE, UNAVAILABLE_EVIDENCE] {
        let evidence = serde_json::from_str::<GenerationEvidence>(literal)?;
        if serde_json::to_string(&evidence)? != literal {
            return Err("generation evidence wire differs from its literal contract".into());
        }
    }
    Ok(())
}

#[test]
fn ready_directory_shape_is_validated_without_filesystem_observation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let absent_bundle = directory.path().join("not-created");
    if absent_bundle.exists() {
        return Err("schema bundle fixture unexpectedly exists".into());
    }
    let absent_bundle = absent_bundle
        .to_str()
        .ok_or("temporary path is not UTF-8")?;
    let literal = READY_EVIDENCE.replace(
        r#""/absent/schema-bundle""#,
        &serde_json::to_string(absent_bundle)?,
    );
    let evidence = serde_json::from_str::<GenerationEvidence>(&literal)?;
    if serde_json::to_string(&evidence)? != literal {
        return Err("absent bundle path wire differs from its literal contract".into());
    }
    Ok(())
}

#[test]
fn generation_evidence_rejects_missing_or_unknown_availability_and_invalid_paths() -> TestResult {
    let valid_evidence = serde_json::from_str::<GenerationEvidence>(READY_EVIDENCE)?;
    let schema_digest = match valid_evidence.schema() {
        GenerationSchemaAvailability::Ready { schema_digest, .. } => *schema_digest,
        GenerationSchemaAvailability::Unavailable { .. } => {
            return Err("ready literal unexpectedly decoded as unavailable".into());
        }
    };
    for path in [
        PathBuf::from("relative/schema-bundle"),
        PathBuf::from("/tmp/../schema-bundle"),
        PathBuf::from("/tmp//schema-bundle"),
        PathBuf::from("/tmp/schema-bundle/"),
        PathBuf::from(OsString::from_vec(b"/tmp/schema-bundle\0hidden".to_vec())),
    ] {
        let error = match GenerationEvidence::new(
            valid_evidence.executable().clone(),
            GenerationSchemaAvailability::Ready {
                schema_digest,
                schema_bundle_dir: path,
            },
        ) {
            Ok(_) => return Err("invalid schema bundle directory path was accepted".into()),
            Err(error) => error,
        };
        if error != GenerationEvidenceError::InvalidSchemaBundleDirectoryPath {
            return Err(format!("unexpected schema evidence error: {error:?}").into());
        }
    }

    let malformed = [
        READY_EVIDENCE.replace(r#""availability":"ready","#, ""),
        READY_EVIDENCE.replace(r#""availability":"ready""#, r#""availability":"future""#),
        READY_EVIDENCE.replace(r#""/absent/schema-bundle""#, r#""relative/schema-bundle""#),
        READY_EVIDENCE.replace(r#""/absent/schema-bundle""#, r#""/tmp/../schema-bundle""#),
        READY_EVIDENCE.replace(r#""/absent/schema-bundle""#, r#""/tmp//schema-bundle""#),
        READY_EVIDENCE.replace(r#""/absent/schema-bundle""#, r#""/tmp/schema-bundle/""#),
        READY_EVIDENCE.replace(
            r#""/absent/schema-bundle""#,
            r#""/tmp/schema-bundle\u0000hidden""#,
        ),
        UNAVAILABLE_EVIDENCE.replace(
            r#""availability":"unavailable""#,
            r#""availability":"future""#,
        ),
        UNAVAILABLE_EVIDENCE.replace(
            r#""reason":"exportFailed"}"#,
            r#""reason":"exportFailed","schema_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","schema_bundle_dir":"/absent/schema-bundle"}"#,
        ),
    ];
    for value in malformed {
        if serde_json::from_str::<GenerationEvidence>(&value).is_ok() {
            return Err("malformed generation evidence was accepted".into());
        }
    }
    Ok(())
}

#[test]
fn generation_current_payload_preserves_literal_alias_and_optional_name() -> TestResult {
    let payload = serde_json::from_str::<GenerationCurrentPayload>(CURRENT_PAYLOAD)?;
    if serde_json::to_string(&payload)? != CURRENT_PAYLOAD
        || payload.generation().number.get() != 7
        || payload.alias().as_path() != Path::new("/run/codex/gen-11111111-7.sock")
    {
        return Err("current payload differs from its literal generation or alias".into());
    }
    if payload.server_display_name().is_some() {
        return Err("absent display name must remain absent".into());
    }

    let named = serde_json::from_str::<GenerationCurrentPayload>(CURRENT_PAYLOAD_WITH_NAME)?;
    if serde_json::to_string(&named)? != CURRENT_PAYLOAD_WITH_NAME {
        return Err("named current payload differs from its literal contract".into());
    }
    if named.server_display_name().map(|name| name.as_str()) != Some("north\n\"east\"") {
        return Err("valid source name characters must be preserved without normalization".into());
    }
    Ok(())
}

#[test]
fn generation_current_payload_rejects_invalid_generation_alias_pairings_and_names() {
    let mut malformed = vec![
        CURRENT_PAYLOAD.replace(
            "11111111-2222-4333-8444-555555555555",
            "00000000-0000-0000-0000-000000000000",
        ),
        CURRENT_PAYLOAD.replace(r#""number":7"#, r#""number":0"#),
        CURRENT_PAYLOAD.replace(
            "/run/codex/gen-11111111-7.sock",
            "/run/codex/gen-22222222-7.sock",
        ),
        CURRENT_PAYLOAD.replace(
            "/run/codex/gen-11111111-7.sock",
            "/run/codex/gen-11111111-8.sock",
        ),
        CURRENT_PAYLOAD.replace(
            "/run/codex/gen-11111111-7.sock",
            "/run/codex/gen-11111111-07.sock",
        ),
    ];
    for invalid_name in ["", "   ", "x\0y"] {
        let invalid_name_json = serde_json::to_string(invalid_name)
            .unwrap_or_else(|error| panic!("invalid display name fixture serializes: {error}"));
        malformed.push(CURRENT_PAYLOAD.replace(
            r#""server_display_name":null"#,
            &format!(r#""server_display_name":{invalid_name_json}"#),
        ));
    }
    let oversized_name = serde_json::to_string(&"x".repeat(4097))
        .unwrap_or_else(|error| panic!("oversized display name fixture serializes: {error}"));
    malformed.push(CURRENT_PAYLOAD.replace(
        r#""server_display_name":null"#,
        &format!(r#""server_display_name":{oversized_name}"#),
    ));

    for value in malformed {
        assert!(serde_json::from_str::<GenerationCurrentPayload>(&value).is_err());
    }
}

#[test]
fn generation_preparation_tags_keep_candidate_and_current_authority_explicit() -> TestResult {
    let candidate =
        format!(r#"{{"evidenceUse":"candidateAdmission","payload":{CURRENT_PAYLOAD}}}"#);
    let current = format!(r#"{{"evidenceUse":"currentGeneration","payload":{CURRENT_PAYLOAD}}}"#);

    for literal in [&candidate, &current] {
        let preparation = serde_json::from_str::<GenerationPreparation>(literal)?;
        if serde_json::to_string(&preparation)? != *literal {
            return Err("preparation wire differs from its literal evidenceUse contract".into());
        }
    }
    let unknown = current.replace(
        r#""evidenceUse":"currentGeneration""#,
        r#""evidenceUse":"future""#,
    );
    let missing = current.replace(r#""evidenceUse":"currentGeneration","#, "");
    if serde_json::from_str::<GenerationPreparation>(&unknown).is_ok()
        || serde_json::from_str::<GenerationPreparation>(&missing).is_ok()
    {
        return Err("unknown or missing evidenceUse tag was accepted".into());
    }
    Ok(())
}
