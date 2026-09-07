use codex_router_host::BackendPublication;
use communication_protocol::{
    ChannelDescription, EndpointAvailability, EndpointRef, ObservationTimestamp,
};
use communication_service::EndpointDirectory;

#[tokio::test]
async fn replacement_clears_advertised_generation_before_new_readiness() {
    let endpoint: EndpointRef = serde_json::from_str(
        r#"{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"}"#,
    )
    .unwrap_or_else(|e| panic!("endpoint: {e}"));
    let directory = EndpointDirectory::new(endpoint.service_id.clone());
    let mut events = directory
        .subscribe()
        .unwrap_or_else(|e| panic!("subscribe: {e}"));
    let epoch = "00000000-0000-4000-8000-000000000002"
        .to_owned()
        .try_into()
        .unwrap_or_else(|e| panic!("epoch: {e}"));
    let mut publication = BackendPublication::new(
        directory,
        endpoint,
        epoch,
        "/tmp/debug-native-owner/backend.sock".into(),
    )
    .unwrap_or_else(|e| panic!("publication: {e}"));
    let gate = publication.admission_gate();
    assert!(gate.acquire().is_err());
    let time = || {
        ObservationTimestamp::try_from("2026-09-05T12:00:00Z".to_owned())
            .unwrap_or_else(|e| panic!("time: {e}"))
    };
    let first = publication
        .ready(time(), None, None)
        .unwrap_or_else(|e| panic!("ready: {e}"));
    let admitted = gate.acquire().unwrap_or_else(|e| panic!("admission: {e}"));
    assert_eq!(admitted.generation(), &first);
    assert!(matches!(
        events
            .next()
            .await
            .unwrap_or_else(|e| panic!("event: {e}"))
            .endpoint
            .availability,
        EndpointAvailability::Available { .. }
    ));
    publication
        .unavailable(
            time(),
            "backend replaced"
                .to_owned()
                .try_into()
                .unwrap_or_else(|e| panic!("reason: {e}")),
        )
        .unwrap_or_else(|e| panic!("loss: {e}"));
    assert!(admitted.retirement().is_cancelled());
    assert!(gate.acquire().is_err());
    let unavailable = events.next().await.unwrap_or_else(|e| panic!("event: {e}"));
    assert!(matches!(
        unavailable.endpoint.channels.first(),
        Some(ChannelDescription::NativeCodex {
            generation: None,
            schema_digest: None,
            ..
        })
    ));
    let second = publication
        .ready(time(), None, None)
        .unwrap_or_else(|e| panic!("successor: {e}"));
    assert_eq!(first.service_epoch, second.service_epoch);
    assert_eq!(
        u64::from(second.generation),
        u64::from(first.generation) + 1
    );
}
