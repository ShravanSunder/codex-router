use collaboration_protocol::{
    DeliveryClientReceipt, DeliveryEvidence, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt,
    DeliveryRejection, DeliveryRejectionReason, DeliveryRouteEvidence, NativeSendReceipt,
    OperationId, SessionReachability,
};
use serde_json::{Value, json};

fn native_receipt() -> Result<NativeSendReceipt, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"thread-one"},
        "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1},
        "inputKind":"humanUser",
        "representation":"humanUserText",
        "clientUserMessageId":"caller-correlation",
        "resumeEffect":"notRequested",
        "acceptance":{"kind":"queueAccepted","submissionId":"queue-one"}
    }))?)
}

#[test]
fn receipt_round_trips_selected_and_unselected_outcomes() -> Result<(), Box<dyn std::error::Error>>
{
    let native = native_receipt()?;
    let cases = [
        DeliveryReceipt {
            outcome: DeliveryOutcome::Rejected(DeliveryRejection {
                reason: DeliveryRejectionReason::NoRoute,
                next_action: DeliveryNextAction::CorrectRequest,
                client_code: None,
                detail: Some("no route".into()),
            }),
            reachability: None,
            client: None,
        },
        DeliveryReceipt {
            outcome: DeliveryOutcome::NotSubmitted {
                retryable: true,
                reason: "starting".into(),
            },
            reachability: None,
            client: None,
        },
        DeliveryReceipt {
            outcome: DeliveryOutcome::NotSubmitted {
                retryable: false,
                reason: "staleGeneration".into(),
            },
            reachability: Some(SessionReachability::CodexAppServer),
            client: None,
        },
        DeliveryReceipt {
            outcome: DeliveryOutcome::Unknown,
            reachability: Some(SessionReachability::ProviderAcp),
            client: None,
        },
        DeliveryReceipt {
            outcome: DeliveryOutcome::Queued,
            reachability: Some(SessionReachability::CodexAppServer),
            client: Some(DeliveryClientReceipt::CodexAppServer(native)),
        },
        DeliveryReceipt {
            outcome: DeliveryOutcome::Started,
            reachability: Some(SessionReachability::ProviderAcp),
            client: Some(DeliveryClientReceipt::ProviderAcp {
                operation_id: OperationId::generate(),
            }),
        },
        DeliveryReceipt {
            outcome: DeliveryOutcome::PeerMessageWritten,
            reachability: Some(SessionReachability::ClaudeCodePeer),
            client: Some(DeliveryClientReceipt::ClaudeCodePeer),
        },
    ];
    for receipt in cases {
        let encoded = serde_json::to_value(&receipt)?;
        let decoded: DeliveryReceipt = serde_json::from_value(encoded.clone())?;
        if serde_json::to_value(decoded)? != encoded {
            return Err("delivery receipt lost its client or reachability".into());
        }
    }
    Ok(())
}

#[test]
fn inspection_evidence_round_trips_each_selected_route() -> Result<(), Box<dyn std::error::Error>> {
    let native = json!({
        "kind":"codexAppServer",
        "target":null,"generation":null,"clientUserMessageId":null,
        "nativeTurnId":null,"nativeSubmissionId":null,
        "allocation":"notRequested","resume":"notRequested",
        "submission":"dispatching","cessation":"notApplicable"
    });
    let provider = json!({
        "kind":"providerAcp","bindingId":"binding-one",
        "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1},
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"thread-one"},
        "operationId":OperationId::generate(),"submission":"dispatching"
    });
    let peer = json!({"kind":"claudeCodePeer","sessionId":"peer-one","write":"written"});
    for effect in [native, provider, peer] {
        let decoded: DeliveryRouteEvidence = serde_json::from_value(effect.clone())?;
        if serde_json::to_value(decoded)? != effect {
            return Err("route evidence lost its tagged variant".into());
        }
        let inspection = json!({
            "kind":"dispatching",
            "attemptId":agent_automation::AttemptId::generate(),
            "effects":effect
        });
        let decoded: DeliveryEvidence = serde_json::from_value(inspection.clone())?;
        if serde_json::to_value(decoded)? != inspection {
            return Err("inspection lost selected route evidence".into());
        }
    }
    let pending = json!({"kind":"dispatching","attemptId":agent_automation::AttemptId::generate(),"effects":Value::Null});
    let decoded: DeliveryEvidence = serde_json::from_value(pending.clone())?;
    if serde_json::to_value(decoded)? != pending {
        return Err("unselected attempt acquired route evidence".into());
    }
    Ok(())
}
