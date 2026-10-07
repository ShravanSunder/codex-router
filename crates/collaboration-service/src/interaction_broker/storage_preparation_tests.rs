use super::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, path::Path};

const REPLACEMENT_TIMESTAMP: &str = "2026-10-07T12:13:14.987654321Z";
const TEST_SERVICE_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";

macro_rules! ensure_test {
    ($condition:expr) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
    ($condition:expr, $message:expr) => {
        if !$condition {
            return Err($message.to_string().into());
        }
    };
}

macro_rules! ensure_test_eq {
    ($left:expr, $right:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "assertion failed: {} == {}: left={left:?}, right={right:?}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    }};
}

fn protocol_session_ref(
    session_id: &str,
) -> Result<collaboration_protocol::SessionRef, serde_json::Error> {
    serde_json::from_value(json!({
        "endpoint": {
            "serviceId": TEST_SERVICE_ID,
            "endpointId": "codex-local"
        },
        "sessionId": session_id
    }))
}

fn valid_approval_routes_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let route = codex_acp_adapter::ApprovalRoute {
        thread_id: "thread-root".to_owned(),
        created_by: protocol_session_ref("route-creator")?,
        approver: protocol_session_ref("route-approver")?,
        access: collaboration_protocol::RouterAccess::WorkspaceWrite,
        scratch_path: "/tmp/fixture-scratch".to_owned(),
        root_message_id: Some("thread-root".to_owned()),
    };
    serde_json::to_vec_pretty(&[route]).map_err(Into::into)
}

fn valid_approval_history_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let record = collaboration_protocol::ApprovalRequestRecord {
        request_id: "legacy-approval".to_owned(),
        requester: protocol_session_ref("legacy-requester")?,
        approver: protocol_session_ref("legacy-approver")?,
        generation: serde_json::from_value(json!({
            "serviceEpoch": TEST_SERVICE_ID,
            "generation": 1
        }))?,
        state: collaboration_protocol::ApprovalState::PendingClientDecision,
        reason: None,
        offered_options: Vec::new(),
        presentation: None,
        decision: None,
        operation: json!({"params":{"options":[]}}),
        expires_at: "2099-01-01T00:00:00Z".to_owned(),
    };
    serde_json::to_vec_pretty(&[record]).map_err(Into::into)
}

fn valid_interaction_history_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let entries = json!({
        "approval-pending": {
            "kind": "approval",
            "requester": {
                "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "codex-local"},
                "sessionId": "requester"
            },
            "approver": {
                "kind": "session",
                "session": {
                    "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "codex-local"},
                    "sessionId": "approver"
                }
            },
            "request": {
                "requestId": "approval-pending",
                "title": "Run command",
                "optionsOrigin": "agentOffered",
                "options": [{
                    "optionId": "allow-once",
                    "label": "Allow once",
                    "choice": {"effect": "allow", "scope": "once"}
                }]
            },
            "state": {"kind": "pending"},
            "createdAt": "2026-10-06T09:08:07.123456789Z"
        },
        "question-pending": {
            "kind": "question",
            "requester": {
                "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "codex-local"},
                "sessionId": "requester"
            },
            "approver": {
                "kind": "session",
                "session": {
                    "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "codex-local"},
                    "sessionId": "approver"
                }
            },
            "request": {
                "requestId": "question-pending",
                "prompt": "Proceed?",
                "fields": [{
                    "kind": "boolean",
                    "fieldId": "confirm",
                    "label": "Confirm",
                    "description": null,
                    "required": true
                }]
            },
            "state": {"kind": "pending"}
        },
        "refused-with-second-precision": {
            "kind": "refusedApproval",
            "requester": {
                "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "codex-local"},
                "sessionId": "requester"
            },
            "approver": {
                "kind": "session",
                "session": {
                    "endpoint": {"serviceId": TEST_SERVICE_ID, "endpointId": "codex-local"},
                    "sessionId": "approver"
                }
            },
            "refusal": {
                "requestId": "refused-with-second-precision",
                "title": "Rejected approval",
                "description": null,
                "subject": null,
                "options": [],
                "reason": "fixture refusal"
            },
            "createdAt": "2026-10-06T09:08:07Z"
        }
    });
    serde_json::to_vec_pretty(&entries).map_err(Into::into)
}

fn invalid_interaction_history_bytes(invalid_case: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut values: Value = serde_json::from_slice(&valid_interaction_history_bytes()?)?;
    let entries = values
        .as_object_mut()
        .ok_or("interaction history must serialize as an object")?;
    match invalid_case {
        "null-timestamp" => {
            entries
                .get_mut("approval-pending")
                .and_then(Value::as_object_mut)
                .ok_or("approval row must be an object")?
                .insert("createdAt".to_owned(), Value::Null);
        }
        "offset-timestamp" => {
            entries
                .get_mut("approval-pending")
                .and_then(Value::as_object_mut)
                .ok_or("approval row must be an object")?
                .insert(
                    "createdAt".to_owned(),
                    Value::String("2026-10-06T09:08:07.123456789+00:00".to_owned()),
                );
        }
        "invalid-timestamp" => {
            entries
                .get_mut("approval-pending")
                .and_then(Value::as_object_mut)
                .ok_or("approval row must be an object")?
                .insert(
                    "createdAt".to_owned(),
                    Value::String("not-a-timestampZ".to_owned()),
                );
        }
        "mismatched-map-key" => {
            let record = entries
                .remove("approval-pending")
                .ok_or("approval row must exist")?;
            entries.insert("different-map-key".to_owned(), record);
        }
        "invalid-stored-value" => {
            entries
                .get_mut("refused-with-second-precision")
                .and_then(Value::as_object_mut)
                .and_then(|record| record.get_mut("refusal"))
                .and_then(Value::as_object_mut)
                .ok_or("refusal record must be an object")?
                .insert("reason".to_owned(), Value::String(String::new()));
        }
        "unknown-field" => {
            entries
                .get_mut("refused-with-second-precision")
                .and_then(Value::as_object_mut)
                .ok_or("refusal row must be an object")?
                .insert("unexpectedField".to_owned(), Value::Bool(true));
        }
        _ => return Err(format!("unknown invalid-case fixture: {invalid_case}").into()),
    }
    serde_json::to_vec_pretty(&values).map_err(Into::into)
}

fn replacement_interaction_history_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let mut values: Value = serde_json::from_slice(&valid_interaction_history_bytes()?)?;
    let entries = values
        .as_object_mut()
        .ok_or("interaction history must serialize as an object")?;
    let mut latest_question = entries
        .remove("question-pending")
        .ok_or("pending question must exist")?;
    let question_object = latest_question
        .as_object_mut()
        .ok_or("question record must be an object")?;
    question_object
        .get_mut("request")
        .and_then(Value::as_object_mut)
        .ok_or("question request must be an object")?
        .insert(
            "requestId".to_owned(),
            Value::String("latest-after-prepare".to_owned()),
        );
    question_object.insert(
        "createdAt".to_owned(),
        Value::String(REPLACEMENT_TIMESTAMP.to_owned()),
    );
    entries.clear();
    entries.insert("latest-after-prepare".to_owned(), latest_question);
    serde_json::to_vec_pretty(&values).map_err(Into::into)
}

fn write_store_files(directory: &Path, interaction_history: &[u8]) -> Result<(), Box<dyn Error>> {
    std::fs::write(
        directory.join("approval-routes.json"),
        valid_approval_routes_bytes()?,
    )?;
    std::fs::write(
        directory.join("approval-history.json"),
        valid_approval_history_bytes()?,
    )?;
    std::fs::write(
        directory.join("interaction-history.json"),
        interaction_history,
    )?;
    Ok(())
}

fn snapshot_directory(directory: &Path) -> std::io::Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let file_name = entry
            .file_name()
            .into_string()
            .map_err(|_| std::io::Error::other("fixture filename is not UTF-8"))?;
        files.insert(file_name, std::fs::read(entry.path())?);
    }
    Ok(files)
}

#[tokio::test]
async fn preparation_validation_preserves_real_files() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    write_store_files(directory.path(), &valid_interaction_history_bytes()?)?;
    let before = snapshot_directory(directory.path())?;

    ServiceInteractionBroker::validate_storage_for_preparation(
        &directory.path().join("approval-routes.json"),
    )
    .await?;

    let after = snapshot_directory(directory.path())?;
    ensure_test!(
        before == after,
        "the replacement-preparation read must preserve the existing files"
    );
    Ok(())
}

#[tokio::test]
async fn preparation_validation_leaves_missing_files_missing() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let before = snapshot_directory(directory.path())?;

    ServiceInteractionBroker::validate_storage_for_preparation(
        &directory.path().join("approval-routes.json"),
    )
    .await?;

    let after = snapshot_directory(directory.path())?;
    ensure_test!(before == after && after.is_empty());
    Ok(())
}

#[tokio::test]
async fn preparation_validation_rejects_corrupt_source_files_without_repair()
-> Result<(), Box<dyn Error>> {
    for corrupt_file in [
        "approval-routes.json",
        "approval-history.json",
        "interaction-history.json",
    ] {
        let directory = tempfile::tempdir()?;
        write_store_files(directory.path(), &valid_interaction_history_bytes()?)?;
        std::fs::write(directory.path().join(corrupt_file), b"{")?;
        let before = snapshot_directory(directory.path())?;

        let result = ServiceInteractionBroker::validate_storage_for_preparation(
            &directory.path().join("approval-routes.json"),
        )
        .await;

        let after = snapshot_directory(directory.path())?;
        ensure_test!(result.is_err(), format!("{corrupt_file} must fail closed"));
        ensure_test!(
            before == after,
            format!("{corrupt_file} must not be repaired")
        );
    }
    Ok(())
}

#[tokio::test]
async fn preparation_validation_rejects_invalid_timestamps_and_records_without_repair()
-> Result<(), Box<dyn Error>> {
    for invalid_case in [
        "null-timestamp",
        "offset-timestamp",
        "invalid-timestamp",
        "mismatched-map-key",
        "invalid-stored-value",
        "unknown-field",
    ] {
        let directory = tempfile::tempdir()?;
        write_store_files(
            directory.path(),
            &invalid_interaction_history_bytes(invalid_case)?,
        )?;
        let before = snapshot_directory(directory.path())?;

        let result = ServiceInteractionBroker::validate_storage_for_preparation(
            &directory.path().join("approval-routes.json"),
        )
        .await;

        let after = snapshot_directory(directory.path())?;
        ensure_test!(result.is_err(), format!("{invalid_case} must fail closed"));
        ensure_test!(
            before == after,
            format!("{invalid_case} must not be repaired")
        );
    }
    Ok(())
}

#[tokio::test]
async fn ordinary_load_rereads_replaced_history_and_keeps_valid_timestamp()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let routes_path = directory.path().join("approval-routes.json");
    write_store_files(directory.path(), &valid_interaction_history_bytes()?)?;
    ServiceInteractionBroker::validate_storage_for_preparation(&routes_path).await?;
    std::fs::write(
        directory.path().join("interaction-history.json"),
        replacement_interaction_history_bytes()?,
    )?;

    let service_id: collaboration_protocol::UuidIdentity =
        serde_json::from_value(json!(TEST_SERVICE_ID))?;
    let backend = NativeControlBackend {
        endpoint: collaboration_protocol::EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: collaboration_protocol::EndpointId::try_from("codex-local".to_owned())?,
        },
        gate: crate::NativeGenerationGate::default(),
        codex_home: directory.path().to_path_buf(),
    };
    let broker = ServiceInteractionBroker::load(service_id, backend, routes_path).await?;

    let records = broker.list_interactions().await;
    ensure_test_eq!(records.len(), 1);
    ensure_test_eq!(records[0].request_id(), "latest-after-prepare");
    ensure_test!(matches!(
        &records[0],
        InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Cancelled { reason },
            ..
        } if reason.as_str() == "hostRestarted"
    ));
    let persisted: Value = serde_json::from_slice(&std::fs::read(
        directory.path().join("interaction-history.json"),
    )?)?;
    let persisted_timestamp = chrono::DateTime::parse_from_rfc3339(
        persisted["latest-after-prepare"]["createdAt"]
            .as_str()
            .ok_or("ordinary load must retain the timestamp")?,
    )?;
    let expected_timestamp = chrono::DateTime::parse_from_rfc3339(REPLACEMENT_TIMESTAMP)?;
    ensure_test_eq!(persisted_timestamp, expected_timestamp);
    Ok(())
}
