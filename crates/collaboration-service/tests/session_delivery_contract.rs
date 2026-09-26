use collaboration_service::{
    DeliveryPrecondition, RouteClaim, RouteUnavailableReason, RunSettlement, RunSubmission,
    RunSummarySource,
};
use serde_json::{json, to_value};

#[test]
fn route_claims_and_preconditions_keep_tagged_meaning() -> Result<(), Box<dyn std::error::Error>> {
    let unavailable = RouteClaim::Unavailable {
        reason: RouteUnavailableReason {
            reason: "provider process exited".into(),
            fix: "restart the debug Host".into(),
        },
        retryable: true,
    };
    let encoded = to_value(&unavailable)?;
    if encoded
        != json!({
            "kind":"unavailable","reason":{"reason":"provider process exited","fix":"restart the debug Host"},
            "retryable":true
        })
    {
        return Err("unavailable claim lost its reason or fix".into());
    }
    let decoded: RouteClaim = serde_json::from_value(encoded)?;
    if !matches!(
        decoded,
        RouteClaim::Unavailable {
            retryable: true,
            ..
        }
    ) {
        return Err("unavailable claim changed variant".into());
    }
    let generation = json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1});
    let decoded: DeliveryPrecondition = serde_json::from_value(json!({
        "kind":"endpointGeneration","expected":generation
    }))?;
    if !matches!(decoded, DeliveryPrecondition::EndpointGeneration { .. }) {
        return Err("strict generation guard changed variant".into());
    }
    if to_value(DeliveryPrecondition::Unpinned)? != json!({"kind":"unpinned"}) {
        return Err("unpinned guard changed shape".into());
    }
    Ok(())
}

#[test]
fn settled_run_records_summary_source_only_when_available() -> Result<(), Box<dyn std::error::Error>>
{
    let native_turn = serde_json::from_value(json!({
        "target": {
            "endpoint": {
                "serviceId": "00000000-0000-4000-8000-000000000001",
                "endpointId": "codex-local"
            },
            "sessionId": "thread-one"
        },
        "turnId": "turn-one"
    }))?;
    for source in [
        None,
        Some(RunSummarySource::NativeTurn { turn: native_turn }),
    ] {
        let settlement = RunSettlement::Completed {
            summary_source: source,
        };
        let encoded = to_value(&settlement)?;
        let decoded: RunSettlement = serde_json::from_value(encoded.clone())?;
        if to_value(decoded)? != encoded {
            return Err("settled summary source changed during round trip".into());
        }
    }
    Ok(())
}

#[test]
fn scheduled_submission_and_settlement_keep_distinct_recovery_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    for submission in [
        RunSubmission::NotStartedBusy,
        RunSubmission::Rejected(collaboration_protocol::DeliveryRejection {
            reason: collaboration_protocol::DeliveryRejectionReason::Busy,
            next_action: collaboration_protocol::DeliveryNextAction::RetryLater,
            client_code: None,
            detail: Some("worker still active".into()),
        }),
        RunSubmission::Unknown,
    ] {
        let encoded = to_value(&submission)?;
        let decoded: RunSubmission = serde_json::from_value(encoded.clone())?;
        if to_value(decoded)? != encoded {
            return Err("scheduled submission changed during round trip".into());
        }
    }
    for settlement in [
        RunSettlement::Pending,
        RunSettlement::Failed {
            reason: "native turn failed".into(),
        },
        RunSettlement::Interrupted,
        RunSettlement::WrittenWithoutCompletion,
    ] {
        let encoded = to_value(&settlement)?;
        let decoded: RunSettlement = serde_json::from_value(encoded.clone())?;
        if to_value(decoded)? != encoded {
            return Err("run settlement changed during round trip".into());
        }
    }
    Ok(())
}
