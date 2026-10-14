//! Inventory eligibility tables; native attachment is deliberately outside this read boundary.
use super::*;
use serde_json::{Value, json};

const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const FOREIGN: &str = "00000000-0000-4000-8000-000000000003";

fn manifest_identity() -> collaboration_client::RouterIdentity {
    collaboration_client::RouterIdentity {
        service_id: SERVICE.to_owned().try_into().unwrap(),
        service_epoch: EPOCH.to_owned().try_into().unwrap(),
        service_version: "1".to_owned(),
        machine_label: "Binding fixture".to_owned().try_into().unwrap(),
        native_schema_digest: None,
    }
}

fn native(endpoint_id: &str) -> Value {
    json!({
        "endpoint":{"serviceId":SERVICE,"endpointId":endpoint_id},"label":"Native fixture",
        "availability":{"state":"available","observedAt":"2026-10-06T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock",
            "schemaDigest":null,"generation":{"serviceEpoch":EPOCH,"generation":1}}]
    })
}

fn bind(
    endpoints: Vec<Value>,
    view: NativeSessionView,
) -> Result<NativeInventoryBinding, NativeBindingRejection> {
    let inventory = serde_json::from_value(json!({
        "serviceEpoch":EPOCH,"sequence":0,"endpoints":endpoints
    }))
    .unwrap();
    let router_identity = manifest_identity();
    bind_native_inventory(
        &inventory,
        &router_identity,
        &router_identity.service_id,
        NativeEndpointSelector::UniqueNative,
        view,
    )
}

#[test]
fn unique_native_binding_rejects_missing_ambiguous_duplicate_and_foreign_endpoints() {
    let mut foreign = native("codex-other");
    foreign["endpoint"]["serviceId"] = json!(FOREIGN);
    let mut duplicated_channel = native("codex-local");
    let channel = duplicated_channel["channels"][0].clone();
    duplicated_channel["channels"]
        .as_array_mut()
        .unwrap()
        .push(channel);
    for (entries, expected) in [
        (vec![], NativeBindingRejection::EndpointUnavailable),
        (
            vec![native("codex-local"), native("codex-other")],
            NativeBindingRejection::AmbiguousEndpoint,
        ),
        (
            vec![native("codex-local"), native("codex-local")],
            NativeBindingRejection::InvalidInventory,
        ),
        (vec![foreign], NativeBindingRejection::WrongService),
        (
            vec![duplicated_channel],
            NativeBindingRejection::InvalidInventory,
        ),
    ] {
        assert_eq!(bind(entries, NativeSessionView::Stored), Err(expected));
    }
}

#[test]
fn stored_binding_does_not_claim_runtime_eligibility() {
    for availability in [
        json!({"state":"unprobed"}),
        json!({"state":"unavailable","observedAt":"2026-10-06T00:00:00Z","reason":"native offline"}),
        json!({"state":"available","observedAt":"2026-10-06T00:00:00Z"}),
    ] {
        let mut entry = native("source-native");
        entry["availability"] = availability;
        entry["channels"][0]["generation"] = Value::Null;
        let stored = bind(vec![entry.clone()], NativeSessionView::Stored).unwrap();
        assert_eq!(String::from(stored.endpoint.endpoint_id), "source-native");
        assert_eq!(stored.context, NativeInventoryContext::Stored);
        for view in [NativeSessionView::Loaded, NativeSessionView::Active] {
            assert_eq!(
                bind(vec![entry.clone()], view),
                Err(NativeBindingRejection::RuntimeUnavailable)
            );
        }
    }
}

#[test]
fn runtime_binding_pins_generation_and_rejects_wrong_epoch() {
    for view in [NativeSessionView::Loaded, NativeSessionView::Active] {
        let binding = bind(vec![native("source-native")], view).unwrap();
        assert_eq!(
            binding.context,
            NativeInventoryContext::Runtime(
                serde_json::from_value(json!({"serviceEpoch":EPOCH,"generation":1})).unwrap()
            )
        );
        let mut changed = native("source-native");
        changed["channels"][0]["generation"]["serviceEpoch"] = json!(FOREIGN);
        assert_eq!(
            bind(vec![changed], view),
            Err(NativeBindingRejection::StaleSnapshot)
        );
    }
}

#[test]
fn expected_service_epoch_and_exact_selector_cannot_be_substituted() {
    let mut inventory: EndpointInventory = serde_json::from_value(json!({
        "serviceEpoch":EPOCH,"sequence":0,"endpoints":[native("codex-other"),native("codex-local")]
    }))
    .unwrap();
    let router_identity = manifest_identity();
    let endpoint: EndpointRef =
        serde_json::from_value(json!({"serviceId":SERVICE,"endpointId":"codex-local"})).unwrap();
    let binding = bind_native_inventory(
        &inventory,
        &router_identity,
        &router_identity.service_id,
        NativeEndpointSelector::Exact(endpoint.clone()),
        NativeSessionView::Loaded,
    )
    .unwrap();
    assert_eq!(binding.endpoint, endpoint);
    inventory.endpoints.reverse();
    assert_eq!(
        bind_native_inventory(
            &inventory,
            &router_identity,
            &router_identity.service_id,
            NativeEndpointSelector::Exact(endpoint.clone()),
            NativeSessionView::Loaded
        )
        .unwrap(),
        binding
    );
    let foreign = FOREIGN.to_owned().try_into().unwrap();
    assert_eq!(
        bind_native_inventory(
            &inventory,
            &router_identity,
            &foreign,
            NativeEndpointSelector::UniqueNative,
            NativeSessionView::Stored
        ),
        Err(NativeBindingRejection::WrongService)
    );
    inventory.service_epoch = foreign;
    assert_eq!(
        bind_native_inventory(
            &inventory,
            &router_identity,
            &router_identity.service_id,
            NativeEndpointSelector::Exact(endpoint),
            NativeSessionView::Stored
        ),
        Err(NativeBindingRejection::StaleSnapshot)
    );
}
