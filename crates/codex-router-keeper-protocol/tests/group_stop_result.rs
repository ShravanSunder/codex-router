use codex_router_keeper_protocol::GroupStopResult;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn shared_stop_results_preserve_exact_closed_camel_case_wire() -> TestResult {
    for (result, wire) in [
        (GroupStopResult::Graceful, r#""graceful""#),
        (GroupStopResult::Killed, r#""killed""#),
        (
            GroupStopResult::TimedOutStillRunning,
            r#""timedOutStillRunning""#,
        ),
        (GroupStopResult::DrainOverrun, r#""drainOverrun""#),
    ] {
        if serde_json::to_string(&result)? != wire
            || serde_json::from_str::<GroupStopResult>(wire)? != result
        {
            return Err("shared stop result wire changed".into());
        }
    }
    Ok(())
}
#[test]
fn unknown_or_malformed_stop_result_claims_are_rejected() {
    for wire in [
        r#""groupEmpty""#,
        r#""Killed""#,
        "null",
        "1",
        r#"{"result":"killed"}"#,
    ] {
        assert!(serde_json::from_str::<GroupStopResult>(wire).is_err());
    }
}
