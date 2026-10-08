//! Published rejections and refused deliveries keep their typed evidence through the client.
use collaboration_client::protocol::{
    DeliveryNextAction, DeliveryOutcome, DeliveryRejectionReason, OperationEffect,
    SessionMessageSendParams,
};
use collaboration_client::{ClientError, CollaborationClient, operation_failure_from_client_error};
use serde_json::{Value, json};

#[path = "support/scripted_api.rs"]
mod scripted_api;
use scripted_api::{SCRIPTED_SERVICE_ID, ScriptedApi};

#[tokio::test]
async fn client_preserves_valid_cursor_errors() {
    // Arrange: valid errors have deliberately different data shapes.
    let cases = [
        ("addresses_list", json!({"kind":"snapshotExpired"})),
        (
            "journal_read",
            json!({"kind":"historyExpired","current":{"journalId":"00000000-0000-4000-8000-000000000001","earliestSequence":4,"lastSequence":9}}),
        ),
    ];
    for (tool, data) in cases {
        // Act: the Router publishes the rejection as an operation failure tool error.
        let result = receive_failure(tool, published_rejection(-32050, data.clone()))
            .await
            .unwrap();
        // Assert: validity does not erase cursor bounds, correlation or partial effects.
        assert!(
            matches!(result,ClientError::Rejected {code:-32050,data:Some(actual)} if actual==data),
            "{tool}"
        );
    }
}

#[tokio::test]
async fn message_result_preserves_native_rejection_reason_action_and_code() {
    let (api, mut calls) = ScriptedApi::start().await.unwrap();
    let fixture = tokio::spawn(async move {
        let request = calls.next().await.unwrap();
        assert_eq!(request.tool, "message_send");
        let push_id = "01900000-0000-7000-8000-000000000003";
        let target = request.arguments["target"].clone();
        request.fail(json!({
            "mcpResult":"error",
            "kind":"rejected",
            "message":"Delivery was rejected",
            "effect":"none",
            "pushId":push_id,
            "link":format!("router://{SCRIPTED_SERVICE_ID}/push/{push_id}"),
            "target":target,
            "targetIdentity":"fixture",
            "deliveryState":"rejected",
            "receipt":{
                "outcome":{"kind":"rejected","reason":"childThread","nextAction":"inspectTarget","clientCode":-32000,"detail":null},
                "reachability":"codexAppServer","client":null
            }
        }));
    });
    let client = CollaborationClient::connect(api.directory(), "error-contract", "1")
        .await
        .unwrap();
    let request: SessionMessageSendParams = serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":SCRIPTED_SERVICE_ID,"endpointId":"codex-local"},"sessionId":"owned"},
        "message":{"kind":"humanUser","text":"fixture"},"mode":"auto",
        "generationGuard":null
    }))
    .unwrap();
    let receipt = client.send_human_input(request).await.unwrap();
    assert!(
        matches!(receipt.outcome, DeliveryOutcome::Rejected(rejection)
        if rejection.reason == DeliveryRejectionReason::ChildThread
            && rejection.next_action == DeliveryNextAction::InspectTarget
            && rejection.client_code == Some(-32000))
    );
    fixture.await.unwrap();
}

/// The tool error the Router publishes for a rejection, as its presentation builds it.
fn published_rejection(code: i64, data: Value) -> Value {
    let failure = operation_failure_from_client_error(
        ClientError::Rejected {
            code,
            data: Some(data),
        },
        OperationEffect::None,
    );
    let mut published = serde_json::to_value(failure).unwrap_or(Value::Null);
    if let Some(fields) = published.as_object_mut() {
        fields.insert("mcpResult".to_owned(), json!("error"));
    }
    published
}

async fn receive_failure(
    tool: &'static str,
    failure: Value,
) -> Result<ClientError, Box<dyn std::error::Error + Send + Sync>> {
    let (api, mut calls) = ScriptedApi::start().await?;
    let fixture = tokio::spawn(async move {
        let request = calls.next().await?;
        if request.tool != tool {
            return Err(std::io::Error::other("unexpected tool"));
        }
        request.fail(failure);
        Ok(())
    });
    let client = CollaborationClient::connect(api.directory(), "error-contract", "1").await?;
    let endpoint = serde_json::from_value(
        json!({"serviceId":SCRIPTED_SERVICE_ID,"endpointId":"codex-local"}),
    )?;
    let result = match tool {
        "addresses_list" => client
            .list_addresses(&endpoint, 100, None)
            .await
            .map(|_| ()),
        "journal_read" => client
            .read_journal(
                &endpoint,
                serde_json::from_value(
                    json!({"journalId":"00000000-0000-4000-8000-000000000001","sequence":0}),
                )?,
                100,
                0,
            )
            .await
            .map(|_| ()),
        _ => return Err("unexpected fixture tool".into()),
    };
    fixture.await??;
    match result {
        Ok(()) => Err("the published failure was accepted".into()),
        Err(error) => Ok(error),
    }
}
