//! Real dispatcher/API/storage/renderer integration compiled through a private lib-test seam.
//! No public production API is exported solely to construct test fixtures.
use super::*;
use crate::presentation::session_picker::PickerSourceContext;
use crate::sessions::router_connection_registry::RouterConnectionRegistry;
#[path = "configured_inventory_read/source_api_fixture.rs"]
mod source_api_fixture;
pub(crate) use source_api_fixture::SourceApiFixture;

#[tokio::test]
async fn wire_observer_preserves_genuine_api_header_acceptance_and_rejection() {
    let fixture =
        SourceApiFixture::start("00000000-0000-4000-8000-000000000001", "source-model").await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let payload=serde_json::json!({"jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"endpoints_list","arguments":{}}}).to_string();
    for (accept, content_type, protocol) in [
        (
            Some("application/json, text/event-stream"),
            Some("application/json"),
            Some("2025-11-25"),
        ),
        (None, Some("application/json"), Some("2025-11-25")),
        (
            Some("text/plain"),
            Some("application/json"),
            Some("2025-11-25"),
        ),
        (
            Some("application/json, text/event-stream"),
            None,
            Some("2025-11-25"),
        ),
        (
            Some("application/json, text/event-stream"),
            Some("text/plain"),
            Some("2025-11-25"),
        ),
        (
            Some("application/json, text/event-stream"),
            Some("application/json"),
            Some("not-a-protocol"),
        ),
    ] {
        let mut replies = Vec::new();
        for url in [&fixture.direct_api_url, &fixture.url] {
            let mut request = client.post(url).body(payload.clone());
            for (header, value) in [
                ("accept", accept),
                ("content-type", content_type),
                ("mcp-protocol-version", protocol),
            ] {
                if let Some(value) = value {
                    request = request.header(header, value);
                }
            }
            let response = request.send().await.unwrap();
            let status = response.status();
            let media = response.headers().get("content-type").cloned();
            let body = response.bytes().await.unwrap();
            replies.push((status, media, body));
        }
        assert_eq!(
            replies[0], replies[1],
            "observer must preserve the actual API response for {accept:?}/{content_type:?}/{protocol:?}"
        );
    }
    fixture.finish().await;
}

#[tokio::test]
async fn configured_dispatch_reads_two_real_mcp_sources_without_collapsing_equal_ids() {
    // Arrange: actual APIs own separate SQLite catalogs; neither native backend is running.
    let (first, second) = tokio::join!(
        SourceApiFixture::start("00000000-0000-4000-8000-000000000001", "first-source-model"),
        SourceApiFixture::start(
            "00000000-0000-4000-8000-000000000002",
            "second-source-model"
        ),
    );
    let first_request = first.request("First machine");
    let second_request = second.request("Second machine");
    let loader = session_picker_record_loader(
        CliContext::new(Vec::new()),
        discover_repository_identity(std::path::Path::new("/tmp")),
        None,
    );
    // Act: call the production dispatcher callback, not a substitute source producer.
    let (first_result, second_result) = tokio::join!(
        read_complete_source(loader.clone(), first_request.clone()),
        read_complete_source(loader.clone(), second_request.clone())
    );
    tokio::join!(first.finish(), second.finish());
    // Assert: independently fixed source/model/path facts survive the actual read path.
    assert_eq!(first_result.request(), &first_request);
    assert_eq!(second_result.request(), &second_request);
    let first_snapshot = first_result
        .into_snapshot()
        .expect("first configured read must reach its API");
    let second_snapshot = second_result
        .into_snapshot()
        .expect("second configured read must reach its API");
    assert_eq!(first_snapshot.records.len(), 1);
    assert_eq!(second_snapshot.records.len(), 1);
    let first_row = &first_snapshot.records[0];
    let second_row = &second_snapshot.records[0];
    assert_eq!(first_row.session_id, "shared-source-id");
    assert_eq!(first_row.session_id, second_row.session_id);
    assert_ne!(first_row.identity, second_row.identity);
    assert_eq!(first_row.model.as_deref(), Some("first-source-model"));
    assert_eq!(second_row.model.as_deref(), Some("second-source-model"));
    assert_eq!(first_row.cwd.as_deref(), Some("/source/project"));
    assert!(first_row.normalized_cwd.is_none() && second_row.normalized_cwd.is_none());
    assert!(first_row.conversation_source.is_none() && second_row.conversation_source.is_none());
}

#[tokio::test]
async fn configured_dispatch_rejects_wrong_service_and_unqualified_credential_before_source_rows() {
    let fixture =
        SourceApiFixture::start("00000000-0000-4000-8000-000000000003", "source-model").await;
    let mut wrong = fixture.request("Wrong identity");
    let PickerSourceContext::ConfiguredHosted(profile) = &mut wrong.source_context else {
        panic!("configured fixture");
    };
    profile.service_id = "00000000-0000-4000-8000-000000000004"
        .to_owned()
        .try_into()
        .unwrap();
    let registry = RouterConnectionRegistry::parse(&serde_json::json!({"version":1,"routers":[{
        "name":"Unqualified credential","connection":{"kind":"remote","serviceId":fixture.endpoint.service_id,
        "mcpUrl":fixture.url,"credential":{"kind":"environment","variable":"SOURCE_CREDENTIAL_MUST_NOT_BE_READ"}}
    }]}).to_string()).unwrap();
    let mut credential = fixture.request("Credential source");
    credential.source_context = PickerSourceContext::ConfiguredHosted(registry.routers[0].clone());
    let loader = session_picker_record_loader(
        CliContext::new(Vec::new()),
        discover_repository_identity(std::path::Path::new("/tmp")),
        None,
    );
    let wrong_result = loader(wrong.clone()).await;
    let credential_result = loader(credential.clone()).await;
    fixture.finish().await;
    assert_eq!(wrong_result.request(), &wrong);
    assert_eq!(credential_result.request(), &credential);
    assert_eq!(
        wrong_result.into_snapshot().unwrap_err(),
        crate::presentation::session_picker::SourceInventoryRejection::InvalidInventory
    );
    assert_eq!(
        credential_result.into_snapshot().unwrap_err(),
        crate::presentation::session_picker::SourceInventoryRejection::EndpointUnqualified
    );
}

#[test]
fn source_provider_projection_keeps_source_path_and_unknown_model_without_caller_canonicalization()
{
    use collaboration_client::protocol::{
        ProviderIdentity, ProviderSessionState, ProviderSessionSummary, SessionRef,
    };
    let caller = tempfile::tempdir().unwrap();
    let caller_root = std::fs::canonicalize(caller.path()).unwrap();
    let actual = caller_root.join("caller-target");
    std::fs::create_dir(&actual).unwrap();
    let source_path = caller_root.join("source-shaped-link");
    std::os::unix::fs::symlink(&actual, &source_path).unwrap();
    let target: SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
        "sessionId":"source-provider-id"
    }))
    .unwrap();
    let summary = ProviderSessionSummary::HostedProvider {
        origin: None,
        target: target.clone(),
        working_directory: source_path.display().to_string().try_into().unwrap(),
        updated_at: 2000,
        state: ProviderSessionState::Idle,
        approver: ProviderIdentity::Session(target.clone()),
        created_by: ProviderIdentity::Session(target.clone()),
    };
    let row = SessionPickerRecord::from_source_provider_summary(&summary, "Source provider");
    assert_eq!(row.identity, SessionPickerIdentity::HostedProvider(target));
    assert_eq!(row.cwd.as_deref(), source_path.to_str());
    assert!(row.normalized_cwd.is_none());
    assert!(
        row.model.is_none() && row.reasoning_effort.is_none() && row.conversation_source.is_none()
    );
    let default = SessionPickerRecord::from_provider_summary(&summary, "Default provider");
    assert_eq!(
        default.normalized_cwd.as_deref(),
        actual.to_str(),
        "existing default path behavior remains intact"
    );
}

#[tokio::test]
async fn configured_any_reads_real_native_and_provider_api_storage_without_equal_id_collision() {
    // Arrange durable source data only: no provider conversation or native backend is started.
    let fixture = SourceApiFixture::start_with_provider(
        "00000000-0000-4000-8000-000000000005",
        "native-source-model",
        true,
    )
    .await;
    let loader = session_picker_record_loader(
        CliContext::new(Vec::new()),
        discover_repository_identity(std::path::Path::new("/tmp")),
        None,
    );
    let snapshot = read_complete_source(loader, fixture.request("Mixed source"))
        .await
        .into_snapshot()
        .unwrap();
    assert_eq!(snapshot.records.len(), 2);
    let native = snapshot
        .records
        .iter()
        .find(|row| matches!(row.identity, SessionPickerIdentity::HostedCodex(_)))
        .unwrap();
    let provider = snapshot
        .records
        .iter()
        .find(|row| matches!(row.identity, SessionPickerIdentity::HostedProvider(_)))
        .unwrap();
    assert_eq!(native.session_id, "shared-source-id");
    assert_eq!(provider.session_id, "shared-source-id");
    assert_ne!(native.identity, provider.identity);
    assert_eq!(native.model.as_deref(), Some("native-source-model"));
    assert_eq!(provider.cwd.as_deref(), Some("/source/provider"));
    assert!(
        provider.model.is_none()
            && provider.reasoning_effort.is_none()
            && provider.normalized_cwd.is_none()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn provider_sources_reject_unsupported_source_filters_before_any_provider_request() {
    use crate::presentation::session_picker::SourceInventoryRejection;
    let fixture = SourceApiFixture::start_with_provider(
        "00000000-0000-4000-8000-000000000007",
        "source-model",
        true,
    )
    .await;
    let loader = fixture.loader();
    let mut observations = Vec::new();
    for filter in [SessionsSource::Interactive, SessionsSource::Subagents] {
        let mut request = fixture.request("Mixed source");
        request.query.source = filter;
        let before = fixture.api_call_count("provider_sessions_list");
        let result = loader(request.clone()).await;
        assert_eq!(result.request(), &request);
        observations.push((
            filter,
            result.into_snapshot().unwrap_err(),
            fixture.api_call_count("provider_sessions_list") - before,
        ));
    }
    fixture.finish().await;
    assert_eq!(
        observations,
        vec![
            (
                SessionsSource::Interactive,
                SourceInventoryRejection::UnsupportedViewOrScope,
                0
            ),
            (
                SessionsSource::Subagents,
                SourceInventoryRejection::UnsupportedViewOrScope,
                0
            ),
        ]
    );
    assert!(
        SourceInventoryRejection::UnsupportedViewOrScope
            .to_string()
            .contains("All sources")
    );
}

#[tokio::test]
async fn native_only_source_keeps_supported_interactive_filter_and_no_provider_call() {
    let fixture =
        SourceApiFixture::start("00000000-0000-4000-8000-000000000008", "interactive-model").await;
    let mut request = fixture.request("Native source");
    request.query.source = SessionsSource::Interactive;
    let snapshot = read_complete_source(fixture.loader(), request)
        .await
        .into_snapshot()
        .unwrap();
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.records[0].session_id, "shared-source-id");
    assert_eq!(
        snapshot.records[0].model.as_deref(),
        Some("interactive-model")
    );
    assert_eq!(fixture.api_call_count("provider_sessions_list"), 0);
    fixture.finish().await;
}

/// A test consumer exhausts real page continuations to preserve terminal scan oracles.
/// Production progressive publication is separately exercised through the actual worker/UI.
pub(crate) async fn read_complete_source(
    loader: SessionsPickerRecordLoader,
    initial: crate::presentation::session_picker::SourceInventoryRequest,
) -> crate::presentation::session_picker::SourceInventoryResult {
    use crate::presentation::session_picker::{InventoryPageProgress, SourceInventoryResult};
    let mut request = initial.clone();
    let mut rows = Vec::new();
    loop {
        match loader(request.clone()).await {
            SourceInventoryResult::Qualified { continuation, .. } => {
                request.continuation = Some(continuation);
            }
            SourceInventoryResult::Rejected { reason, .. } => {
                return SourceInventoryResult::Rejected {
                    request: initial,
                    reason,
                };
            }
            SourceInventoryResult::Ready {
                binding,
                snapshot,
                progress,
                ..
            } => {
                rows.extend(snapshot.records);
                match progress {
                    InventoryPageProgress::Complete => {
                        return SourceInventoryResult::Ready {
                            request: initial,
                            binding,
                            snapshot: crate::picker_runtime_status::PickerRecordsSnapshot {
                                records: rows,
                                runtime_coverage: snapshot.runtime_coverage,
                            },
                            progress: InventoryPageProgress::Complete,
                        };
                    }
                    InventoryPageProgress::MoreAvailable { continuation } => {
                        request.continuation = Some(continuation)
                    }
                }
            }
        }
    }
}
