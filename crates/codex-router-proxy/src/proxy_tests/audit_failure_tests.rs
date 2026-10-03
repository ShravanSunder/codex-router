use super::*;

#[test]
fn audit_append_failure_reports_through_audit_failure_reporter_without_secret_leak() {
    let temp_dir = ProxyTestTempDir::new("audit_failure_reporter");
    let blocked_parent = temp_dir.path().join("audit-parent-is-file");
    match fs::write(&blocked_parent, "not-a-directory") {
        Ok(()) => {}
        Err(error) => panic!("blocked parent fixture should write: {error}"),
    }
    let sink = AuditFileSink::new(blocked_parent.join("events.jsonl"));
    let reporter = RecordingAuditFailureReporter::default();
    let event = AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("request-audit-failure"),
        route_kind: AuditRouteKind::Responses,
        transport_kind: TransportKind::Http,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Allowed,
        decision_reason: "allowed",
        response_commit_state: ResponseCommitState::Committed,
        account_hash: Some("acct_hash_without_secret".to_owned()),
        error_class: None,
    });

    append_audit_event_with_reporter(&sink, &event, &reporter);
    let diagnostics = reporter.diagnostics.borrow();

    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("audit append failed"));
    assert!(!diagnostics[0].contains(&blocked_parent.display().to_string()));
    assert!(!diagnostics[0].contains("access-token-canary"));
    assert!(!diagnostics[0].contains("refresh-token-canary"));
    assert!(!diagnostics[0].contains("local-token-canary"));
}
