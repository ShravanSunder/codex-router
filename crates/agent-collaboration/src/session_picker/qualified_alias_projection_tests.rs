//! Registry order and returned full refs are independent presentation oracles, not remote proof.
use super::*;
use crate::presentation::session_picker::test_support::{observed_records, picker_record};
use crate::sessions::router_connection_registry::RouterConnectionRegistry;

#[test]
fn default_rows_are_not_promoted_into_configured_alias_labels() {
    let service = "00000000-0000-4000-8000-000000000001";
    let first = source("First", service);
    let second = source("Alias", service);
    let bound = endpoint(service, "native-one");
    let (_, _, mut default_rows) = snapshot(&first, &bound, true);
    default_rows.records[0].provenance = SessionRowProvenance::DefaultAttributed;
    let default = PickerSourceContext::DefaultHosted;
    let snapshots = vec![
        (default.clone(), None, default_rows),
        snapshot(&first, &bound, true),
        snapshot(&second, &bound, true),
    ];
    let merged = merge_source_snapshots(&[default.clone(), first, second], &snapshots);
    assert_eq!(merged.records.len(), 1);
    assert_eq!(merged.records[0].source_context, Some(default));
    assert_eq!(merged.records[0].machine_label(), "This machine");
}

#[test]
fn matching_endpoints_at_conflicting_epochs_never_produce_an_alias_label() {
    let service = "00000000-0000-4000-8000-000000000001";
    let first = source("First", service);
    let second = source("Alias", service);
    let bound = endpoint(service, "native-one");
    let mut other = snapshot(&second, &bound, true);
    other.1.as_mut().unwrap().service_epoch = "00000000-0000-4000-8000-000000000005"
        .to_owned()
        .try_into()
        .unwrap();
    let labels = qualified_alias_labels(
        &[first.clone(), second],
        &[snapshot(&first, &bound, true), other],
    );
    assert!(
        labels.is_empty(),
        "endpoint equality alone cannot establish an alias group"
    );
}

fn source(name: &str, service_id: &str) -> PickerSourceContext {
    let registry = RouterConnectionRegistry::parse(&serde_json::json!({
        "version":1,"routers":[{"name":name,"connection":{"kind":"remote", "serviceId":service_id,
            "mcpUrl":"https://fixture.invalid/mcp"}}]
    }).to_string()).unwrap();
    PickerSourceContext::ConfiguredHosted(registry.routers[0].clone())
}

fn endpoint(service_id: &str, endpoint_id: &str) -> EndpointRef {
    serde_json::from_value(serde_json::json!({"serviceId":service_id,"endpointId":endpoint_id}))
        .unwrap()
}

fn snapshot(
    source: &PickerSourceContext,
    endpoint: &EndpointRef,
    populated: bool,
) -> (
    PickerSourceContext,
    Option<SourceInventoryBinding>,
    PickerRecordsSnapshot,
) {
    let mut rows = Vec::new();
    if populated {
        let mut row = picker_record(
            "shared-id",
            "Actual source row",
            "/source/project",
            "source-provider",
            "cli",
        );
        row.identity =
            SessionPickerIdentity::HostedCodex(collaboration_client::protocol::SessionRef {
                endpoint: endpoint.clone(),
                session_id: "shared-id".to_owned().try_into().unwrap(),
            });
        row.provenance = SessionRowProvenance::ObservedHosted;
        row.normalized_cwd = None;
        rows.push(row);
    }
    (
        source.clone(),
        Some(SourceInventoryBinding::native_fixture(endpoint.clone())),
        observed_records(rows),
    )
}

#[test]
fn empty_first_alias_names_all_rows_without_changing_the_observed_route_or_single_choice() {
    let service = "00000000-0000-4000-8000-000000000001";
    let first = source("First machine", service);
    let second = source("Second alias", service);
    let endpoint = endpoint(service, "source-native");
    // Completion order differs from registry order; the first bound inventory has no rows.
    let snapshots = vec![
        snapshot(&second, &endpoint, true),
        snapshot(&first, &endpoint, false),
    ];
    let combined = merge_source_snapshots(&[first, second.clone()], &snapshots);
    assert_eq!(combined.records.len(), 1);
    assert_eq!(
        combined.records[0].machine_label(),
        "First machine (also: Second alias)"
    );
    assert_eq!(combined.records[0].source_context, Some(second.clone()));
    assert_eq!(combined.records[0].model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(
        combined.records[0].provenance,
        SessionRowProvenance::ObservedHosted
    );
    let single = merge_source_snapshots(std::slice::from_ref(&second), &snapshots);
    assert_eq!(single.records[0].machine_label(), "Second alias");
    assert_eq!(single.records[0].source_context, Some(second));
}

#[test]
fn aliases_group_only_full_qualified_endpoints_and_duplicate_ids_stay_distinct() {
    let first_service = "00000000-0000-4000-8000-000000000001";
    let other_service = "00000000-0000-4000-8000-000000000002";
    let first = source("First", first_service);
    let alias = source("Alias", first_service);
    let another_endpoint = source("Another endpoint", first_service);
    let other_machine = source("Other machine", other_service);
    let sources = vec![
        first.clone(),
        alias.clone(),
        another_endpoint.clone(),
        other_machine.clone(),
    ];
    let bound = endpoint(first_service, "native-one");
    let snapshots = vec![
        snapshot(&alias, &bound, true),
        snapshot(&first, &bound, true),
        snapshot(
            &another_endpoint,
            &endpoint(first_service, "native-two"),
            true,
        ),
        snapshot(&other_machine, &endpoint(other_service, "native-one"), true),
    ];
    let combined = merge_source_snapshots(&sources, &snapshots);
    assert_eq!(combined.records.len(), 3);
    let labels = combined
        .records
        .iter()
        .map(|row| row.machine_label())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"First (also: Alias)"));
    assert!(labels.contains(&"Another endpoint"));
    assert!(labels.contains(&"Other machine"));
    assert!(
        combined
            .records
            .iter()
            .all(|row| row.session_id == "shared-id")
    );
}

#[test]
fn missing_foreign_and_row_mismatched_bindings_cannot_qualify_even_empty_snapshots() {
    let service = "00000000-0000-4000-8000-000000000001";
    let source = source("Expected machine", service);
    let empty = observed_records(vec![]);
    let endpoint = endpoint(service, "native-one");
    assert!(!source_snapshot_matches(&source, None, &empty));
    assert!(source_snapshot_matches(
        &source,
        Some(&SourceInventoryBinding::native_fixture(endpoint.clone())),
        &empty
    ));
    let foreign = super::tests::endpoint("00000000-0000-4000-8000-000000000002", "native-one");
    assert!(!source_snapshot_matches(
        &source,
        Some(&SourceInventoryBinding::native_fixture(foreign.clone())),
        &empty
    ));
    let (_, _, mismatched) = snapshot(
        &source,
        &super::tests::endpoint(service, "native-two"),
        true,
    );
    assert!(!source_snapshot_matches(
        &source,
        Some(&SourceInventoryBinding::native_fixture(endpoint.clone())),
        &mismatched
    ));
    let (_, _, mut local_path_snapshot) = snapshot(&source, &endpoint, true);
    local_path_snapshot.records[0].normalized_cwd = Some("/invoking/canonical-project".to_owned());
    assert!(!source_snapshot_matches(
        &source,
        Some(&SourceInventoryBinding::native_fixture(endpoint)),
        &local_path_snapshot
    ));
    let groups = qualified_alias_labels(
        std::slice::from_ref(&source),
        &[
            (source.clone(), None, empty.clone()),
            (
                source.clone(),
                Some(SourceInventoryBinding::native_fixture(foreign)),
                empty,
            ),
        ],
    );
    assert!(groups.is_empty());
}
