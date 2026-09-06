//! Method-specific Control errors must obey the same published contract as success results.
#[cfg(test)]
mod tests {
    use communication_client::{ClientError, ControlClient};
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn control_client_rejects_errors_from_another_method_and_malformed_envelopes() {
        // Arrange: each response violates a different explicit Control 1.0 obligation.
        let cases = [
            json!({"code":-32050,"message":"fixture","data":{"kind":"snapshotExpired"}}),
            json!({"code":-32050,"message":"fixture","data":{"kind":"overloaded","stage":"discovery","message":"fixture","extra":true}}),
            json!({"code":-32602}),
            json!({"code":-32602,"message":"🙂".repeat(1024)}),
        ];
        for error in cases {
            // Act: endpoint/list has no snapshot-expiry variant, and errors are closed/bounded.
            let result = receive_error("endpoint/list", error).await;
            // Assert: an invalid error is never presented as a known protocol rejection.
            assert!(matches!(result, ClientError::Protocol(_)), "{result:?}");
        }
    }

    #[tokio::test]
    async fn control_client_preserves_valid_cursor_and_message_effect_errors() {
        // Arrange: valid errors have deliberately different data shapes.
        let cases = [
            ("addressBook/list", json!({"kind":"snapshotExpired"})),
            (
                "lifecycleJournal/read",
                json!({"kind":"historyExpired","current":{"journalId":"00000000-0000-4000-8000-000000000001","earliestSequence":4,"lastSequence":9}}),
            ),
            (
                "codex/messageSend",
                json!({"kind":"nativeRejected","stage":"start","message":"fixture rejection","effects":{"resume":"accepted","submission":"rejected"},"clientUserMessageId":"correlation"}),
            ),
        ];
        for (method, data) in cases {
            // Act.
            let result = receive_error(
                method,
                json!({"code":-32050,"message":"fixture","data":data}),
            )
            .await;
            // Assert: validity does not erase cursor bounds, correlation or partial effects.
            assert!(
                matches!(result,ClientError::Rejected {code:-32050,data:Some(actual)} if actual==data)
            );
        }
    }

    async fn receive_error(method: &'static str, error: Value) -> ClientError {
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let fixture = tokio::spawn(async move {
            let mut stream = BufReader::new(server);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let init: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(init["method"], "control/initialize");
            let response = json!({"jsonrpc":"2.0","id":init["id"],"result":{"version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}","a".repeat(64))}});
            stream
                .get_mut()
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            stream.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], method);
            let response = json!({"jsonrpc":"2.0","id":request["id"],"error":error});
            stream
                .get_mut()
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        });
        let mut client = ControlClient::initialize(client, "error-contract", "1")
            .await
            .unwrap();
        let endpoint = serde_json::from_value(
            json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"}),
        )
        .unwrap();
        let result=match method {
            "endpoint/list"=>client.list_endpoints().await.map(|_|()),
            "addressBook/list"=>client.list_addresses(&endpoint,100,None).await.map(|_|()),
            "lifecycleJournal/read"=>client.read_journal(&endpoint,serde_json::from_value(json!({"journalId":"00000000-0000-4000-8000-000000000001","sequence":0})).unwrap(),100,0).await.map(|_|()),
            "codex/messageSend"=>client.send_human_input(serde_json::from_value(json!({"target":{"endpoint":endpoint,"sessionId":"owned"},"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},"message":{"kind":"humanUser","text":"fixture"}})).unwrap()).await.map(|_|()),
            _=>panic!("unexpected fixture method"),
        };
        drop(client);
        fixture.await.unwrap();
        result.unwrap_err()
    }
}
