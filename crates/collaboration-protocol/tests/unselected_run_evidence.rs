use collaboration_protocol::{RunExecutionEvidence, RunSnapshot};
use serde_json::json;

#[test]
fn waiting_run_exposes_absent_client_evidence_without_inventing_native_effects() {
    let waiting = json!({
        "runId": agent_automation::RunId::generate(),
        "scheduleId": agent_automation::ScheduleId::generate(),
        "dueAt": "2026-09-24T00:00:00Z",
        "state": {"kind":"waiting"},
        "executionEvidence": {"route":null,"timing":null,"acceptance":null},
        "summary":null
    });
    let decoded: RunSnapshot =
        serde_json::from_value(waiting.clone()).expect("waiting run has no selected client");
    assert_eq!(serde_json::to_value(decoded).unwrap(), waiting);
    let evidence: RunExecutionEvidence =
        serde_json::from_value(waiting["executionEvidence"].clone())
            .expect("absent native evidence decodes");
    assert!(evidence.route.is_none());
}

#[test]
fn absent_client_evidence_cannot_carry_an_execution_budget() {
    let invalid = json!({
        "runId": agent_automation::RunId::generate(),
        "scheduleId": agent_automation::ScheduleId::generate(),
        "dueAt": "2026-09-24T00:00:00Z",
        "state": {"kind":"waiting"},
        "executionEvidence": {"route":null,"timing":{
            "dispatchStartedAt":"2026-09-24T00:00:00Z",
            "effectiveTimeoutSeconds":60,
            "deadlineAt":"2026-09-24T00:01:00Z"
        },"acceptance":null},
        "summary":null
    });
    assert!(serde_json::from_value::<RunSnapshot>(invalid).is_err());
}
