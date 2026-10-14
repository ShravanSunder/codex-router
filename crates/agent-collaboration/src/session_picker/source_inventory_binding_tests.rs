use super::*;
use serde_json::{Value, json};

const SERVICE: &str = "00000000-0000-4000-8000-000000000001";

fn native(service: &str) -> Value {
    json!({"endpoint":{"serviceId":service,"endpointId":"codex-local"},"label":"Native","availability":{"state":"unprobed"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]})
}

fn inventory(endpoints: Vec<Value>) -> EndpointInventory {
    serde_json::from_value(json!({"serviceEpoch":"00000000-0000-4000-8000-000000000002","sequence":1,"endpoints":endpoints})).unwrap()
}

#[test]
fn invalid_machine_membership_never_becomes_an_empty_qualified_catalog() {
    let expected = SERVICE.to_owned().try_into().unwrap();
    let mut duplicate_channel = native(SERVICE);
    let repeated_channel = duplicate_channel["channels"][0].clone();
    duplicate_channel["channels"]
        .as_array_mut()
        .unwrap()
        .push(repeated_channel);
    let mut unsupported = native(SERVICE);
    unsupported["channels"] = json!([{"kind":"acp","transport":"unixJsonLines","path":"source-acp.sock","schemaDigest":format!("sha256:{}","b".repeat(64))}]);
    for (observed, reason) in [
        (
            inventory(vec![]),
            SourceInventoryRejection::EndpointUnqualified,
        ),
        (
            inventory(vec![native("00000000-0000-4000-8000-000000000003")]),
            SourceInventoryRejection::InvalidInventory,
        ),
        (
            inventory(vec![
                native(SERVICE),
                native("00000000-0000-4000-8000-000000000003"),
            ]),
            SourceInventoryRejection::InvalidInventory,
        ),
        (
            inventory(vec![native(SERVICE), native(SERVICE)]),
            SourceInventoryRejection::InvalidInventory,
        ),
        (
            inventory(vec![duplicate_channel]),
            SourceInventoryRejection::InvalidInventory,
        ),
        (
            inventory(vec![unsupported]),
            SourceInventoryRejection::EndpointUnqualified,
        ),
    ] {
        assert_eq!(
            SourceInventoryBinding::from_inventory(&expected, &observed).unwrap_err(),
            reason
        );
    }
    let mut excessive = inventory(vec![native(SERVICE)]);
    excessive.sequence = 9_007_199_254_740_992;
    assert_eq!(
        SourceInventoryBinding::from_inventory(&expected, &excessive).unwrap_err(),
        SourceInventoryRejection::InvalidInventory
    );
}

#[test]
fn stored_binding_uses_observed_service_and_epoch_without_requiring_live_native_generation() {
    let expected = SERVICE.to_owned().try_into().unwrap();
    let observed = inventory(vec![native(SERVICE)]);
    let binding = SourceInventoryBinding::from_inventory(&expected, &observed).unwrap();
    assert_eq!(String::from(binding.service_id.clone()), SERVICE);
    assert_eq!(
        String::from(binding.service_epoch.clone()),
        "00000000-0000-4000-8000-000000000002"
    );
    let endpoint = observed.endpoints[0].endpoint.clone();
    assert!(binding.contains(&endpoint, InventoryEndpointKind::NativeCodex));
    assert!(!binding.contains(&endpoint, InventoryEndpointKind::ExternalProvider));
    let mut foreign = endpoint;
    foreign.service_id = "00000000-0000-4000-8000-000000000003"
        .to_owned()
        .try_into()
        .unwrap();
    assert!(!binding.contains(&foreign, InventoryEndpointKind::NativeCodex));
}
