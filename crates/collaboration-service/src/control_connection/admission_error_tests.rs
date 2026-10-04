use super::*;

#[test]
fn saturated_methods_preserve_their_published_error_contracts() {
    let mut admission = ControlAdmission::default();
    admission.admit("init", "control/initialize").unwrap();
    admission.initialized();
    admission.complete("init");
    for number in 0..64 {
        admission
            .admit(&format!("pending-{number}"), "codex/sessionInspect")
            .unwrap();
    }
    let schema = collaboration_protocol::control_schema_document(None).unwrap();
    let methods = schema.get("x-methods").and_then(Value::as_object).unwrap();
    for method in methods.keys() {
        let response = admit_request(
            json!({"jsonrpc":"2.0","id":format!("overloaded-{method}"),"method":method,"params":{"wakeupId":collaboration_protocol::WakeupId::generate()}}),
            &mut admission,
        );
        let Err(response) = response else {
            panic!("overloaded request admitted");
        };
        assert!(
            collaboration_protocol::control_error_is_valid(method, &response),
            "invalid overload response for {method}: {response}"
        );
    }
}

#[test]
fn provider_methods_report_overload_before_target_resolution() {
    let mut admission = ControlAdmission::default();
    admission.admit("init", "control/initialize").unwrap();
    admission.initialized();
    admission.complete("init");
    for number in 0..64 {
        admission
            .admit(&format!("pending-{number}"), "codex/sessionInspect")
            .unwrap();
    }
    let target = json!({
        "endpoint": {
            "serviceId": "00000000-0000-4000-8000-000000000001",
            "endpointId": "cursor-local"
        },
        "sessionId": "fixture-session"
    });
    let operation_id = collaboration_protocol::OperationId::generate();
    for method in [
        "provider/sessionInspect",
        "provider/sessionList",
        "conversation/resume",
        "conversation/close",
        "conversation/settingsSet",
        "conversation/settingsAccept",
    ] {
        for include_target in [false, true] {
            let params = if include_target {
                json!({"target": target, "operationId": operation_id})
            } else {
                json!({})
            };
            let response = match admit_request(
                json!({"jsonrpc":"2.0","id":format!("{method}-{include_target}"),"method":method,"params":params}),
                &mut admission,
            ) {
                Err(response) => response,
                Ok(_) => panic!("saturated admission accepted {method}"),
            };
            let data = &response["error"]["data"];
            assert_eq!(data["kind"], "overloaded", "{method}: {response}");
            assert_eq!(data["stage"], "discovery", "{method}: {response}");
            assert!(
                collaboration_protocol::control_error_is_valid(method, &response),
                "{method}: {response}"
            );
            if method == "provider/sessionList" {
                assert!(data.get("target").is_none());
            } else if include_target {
                assert_eq!(data["target"], target);
            } else {
                assert!(data.get("target").is_none());
            }
        }
    }
}

#[test]
fn saturated_message_admission_stops_before_any_route_dispatch() {
    // Arrange: existing admitted work occupies every pending slot.
    let mut admission = ControlAdmission::default();
    admission.admit("init", "control/initialize").unwrap();
    admission.initialized();
    admission.complete("init");
    for number in 0..64 {
        admission
            .admit(&format!("pending-{number}"), "codex/sessionInspect")
            .unwrap();
    }
    // Act: this request is rejected by the real admission path before native dispatch.
    let result = admit_request(
        json!({"jsonrpc":"2.0","id":"rejected-message","method":"message/send","params":{}}),
        &mut admission,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("saturated admission accepted work"),
    };
    // Assert: clients can distinguish overload from unknown or partial native submission.
    assert_eq!(error["error"]["data"]["kind"], "overloaded");
    assert!(error["error"]["data"].get("client").is_none());
    assert!(collaboration_protocol::control_error_is_valid(
        "message/send",
        &error
    ));
    let initialization = admit_request(
        json!({"jsonrpc":"2.0","id":"rejected-init","method":"control/initialize","params":{}}),
        &mut admission,
    );
    let initialization = match initialization {
        Err(error) => error,
        Ok(_) => panic!("saturated initialization accepted"),
    };
    assert!(collaboration_protocol::control_error_is_valid(
        "control/initialize",
        &initialization
    ));
}
