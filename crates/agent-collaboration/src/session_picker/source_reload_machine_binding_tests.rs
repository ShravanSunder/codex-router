//! Machine membership is service-scoped; the read producer is outside this unit proof.
use super::*;
use collaboration_client::protocol::{EndpointRef, SessionRef};

#[tokio::test]
async fn configured_machine_keeps_native_and_provider_rows_on_their_own_endpoints() {
    // Arrange: one machine returns two independently routed kinds of sessions.
    let source = sources(1).unwrap().remove(0);
    let PickerSourceContext::ConfiguredHosted(profile) = &source else {
        panic!("configured fixture");
    };
    let native_endpoint: EndpointRef = serde_json::from_value(serde_json::json!({
        "serviceId":String::from(profile.service_id.clone()),"endpointId":"codex-local"
    }))
    .unwrap();
    let provider_endpoint: EndpointRef = serde_json::from_value(serde_json::json!({
        "serviceId":String::from(profile.service_id.clone()),"endpointId":"claude-local"
    }))
    .unwrap();
    let mut native_row = picker_record("shared-id", "Native source", "/source/native", "", "cli")
        .with_hosted_codex(&native_endpoint);
    native_row.provenance = SessionRowProvenance::ObservedHosted;
    native_row.normalized_cwd = None;
    let mut provider_row = picker_record(
        "shared-id",
        "Provider source",
        "/source/provider",
        "",
        "cli",
    );
    provider_row.identity = SessionPickerIdentity::HostedProvider(SessionRef {
        endpoint: provider_endpoint.clone(),
        session_id: "shared-id".to_owned().try_into().unwrap(),
    });
    provider_row.provenance = SessionRowProvenance::ObservedProvider;
    provider_row.normalized_cwd = None;
    let inventory = serde_json::from_value(serde_json::json!({
        "serviceEpoch":"00000000-0000-4000-8000-000000000002","sequence":1,
        "endpoints":[
            {"endpoint":native_endpoint,"label":"Native","availability":{"state":"unprobed"},
             "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]},
            {"endpoint":provider_endpoint,"label":"Provider","availability":{"state":"unprobed"},
             "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"00000000-0000-4000-8000-000000000003","bindingGeneration":1,
                          "runtime":{"provider":"claudeCode","runtimeName":"claude"},"capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]}
        ]
    })).unwrap();
    let binding = SourceInventoryBinding::from_inventory(&profile.service_id, &inventory).unwrap();
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query(""),
        sources: vec![],
    });
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        let binding = binding.clone();
        let rows = vec![native_row.clone(), provider_row.clone()];
        Box::pin(async move {
            SourceInventoryResult::Ready {
                request,
                binding: Some(binding),
                snapshot: observed_records(rows),
            }
        })
    });
    let (published_sender, mut published) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_request, update| {
            if let Some(result) = update.into_snapshot() {
                let _ = published_sender.send(result);
            }
        })
        .await;
    });
    // Act: switch to the machine without selecting a native launch destination.
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query(""),
        sources: vec![source],
    });
    let result = tokio::time::timeout(Duration::from_secs(2), published.recv())
        .await
        .unwrap()
        .unwrap();
    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
    // Assert: equal IDs remain separate native/provider routes within the same machine.
    let snapshot = result.expect("a machine view cannot omit its provider endpoint");
    assert_eq!(snapshot.records.len(), 2);
    assert!(snapshot.records.iter().any(|row| {
        matches!(&row.identity, SessionPickerIdentity::HostedProvider(target)
            if target.endpoint == provider_endpoint && String::from(target.session_id.clone()) == "shared-id")
    }));
    assert_ne!(snapshot.records[0].identity, snapshot.records[1].identity);
}
