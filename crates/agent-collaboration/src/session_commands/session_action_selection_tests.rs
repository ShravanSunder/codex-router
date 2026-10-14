//! Construction executes in a child test process so stderr observation cannot affect siblings.
use super::*;

#[test]
fn factory_probe_preserves_safe_stored_value_and_session() {
    let summary: collaboration_client::protocol::NativeSessionSummary =
        serde_json::from_value(serde_json::json!({
            "target": {
                "endpoint": {
                    "serviceId": "00000000-0000-4000-8000-000000000001",
                    "endpointId": "codex-local"
                },
                "sessionId": "safe-session"
            },
            "name": null,
            "title": "Literal source",
            "source": "interactive",
            "workingDirectory": "/source",
            "observation": {"kind": "stored", "updatedAt": "2026-10-12T00:00:00Z"},
            "model": null,
            "gitBranch": null,
            "idleSeconds": 0
        }))
        .unwrap();
    let mut record = SessionPickerRecord::from_native_summary(&summary);
    record.model = Some("rejected\u{1}value".to_owned());
    record.reasoning_effort = Some("high".to_owned());
    let selection = SessionActionSelection::from_picker_record(&record);
    assert_eq!(selection.session_id(), "safe-session");
    assert_eq!(
        selection.model_diagnostic,
        Some(StoredModelDiagnostic::UnquotableStoredValueDiscarded)
    );
    assert_eq!(
        selection.model_choice,
        ResumeModelChoice::from_stored_values(None, Some("high"))
    );
}

#[test]
fn review_capture_factory_has_no_fullscreen_stderr_side_effect() {
    let probe = "sessions::session_action_selection::tests::factory_probe_preserves_safe_stored_value_and_session";
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", probe, "--nocapture"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "factory probe must execute successfully"
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("1 passed"),
        "the exact factory probe must run, not filter away"
    );
    assert!(
        result.stderr.is_empty(),
        "capturing a fullscreen selection cannot write terminal stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
