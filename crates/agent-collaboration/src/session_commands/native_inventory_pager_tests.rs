//! Source/view/cursor behavior through the real public client and an external protocol peer.
use super::*;

#[path = "native_inventory_service_tests.rs"]
mod native_inventory_service_tests;

fn paging_request(view: NativeSessionView) -> NativeSessionListParams {
    NativeSessionListParams {
        endpoint: serde_json::from_value(endpoint()).unwrap(),
        view,
        scope: NativeSessionScope::Cwd {
            path: "/repo".into(),
        },
        source: NativeSessionSource::Interactive,
        include_empty_sessions: false,
        query: Some("native query".to_owned()),
        page_size: 100,
        cursor: None,
    }
}

fn stored_page(id: &str, cursor: Value) -> Value {
    let mut stored = page(id, json!({"type":"idle"}), cursor);
    stored["generation"] = Value::Null;
    stored["sessions"][0]["observation"] =
        json!({"kind":"stored","updatedAt":"2026-10-05T00:00:00Z"});
    stored
}

#[tokio::test]
async fn sparse_stored_pages_preserve_the_entire_request_and_have_no_runtime_generation() {
    let mut sparse = stored_page("skipped", json!("opaque/first?unchanged"));
    sparse["sessions"] = json!([]);
    let mut exhausted = stored_page("none", Value::Null);
    exhausted["sessions"] = json!([]);
    let (mut client, peer) = connect_fixture(vec![
        ("codex/sessionList", sparse),
        (
            "codex/sessionList",
            stored_page("source-only", json!("opaque second")),
        ),
        ("codex/sessionList", exhausted),
    ])
    .await;
    let mut pager =
        NativeInventoryPager::new(paging_request(NativeSessionView::Stored), None).unwrap();
    let mut rows = Vec::new();
    while let Some(page) = pager.next_page(&mut client).await.unwrap() {
        assert!(page.generation.is_none());
        rows.extend(page.sessions);
    }
    assert!(
        pager.next_page(&mut client).await.unwrap().is_none(),
        "exhaustion cannot trigger a new read"
    );
    client.close().await.unwrap();
    let requests = peer.await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        String::from(rows[0].target.session_id.clone()),
        "source-only"
    );
    for request in &requests {
        assert_eq!(request["params"]["view"], "stored");
        assert_eq!(request["params"]["endpoint"], endpoint());
        assert_eq!(
            request["params"]["scope"],
            json!({"kind":"cwd","path":"/repo"})
        );
        assert_eq!(request["params"]["source"], "interactive");
        assert_eq!(request["params"]["query"], "native query");
        assert_eq!(request["params"]["includeEmptySessions"], false);
    }
    assert_eq!(requests[1]["params"]["cursor"], "opaque/first?unchanged");
    assert_eq!(requests[2]["params"]["cursor"], "opaque second");
}

#[tokio::test]
async fn stored_and_runtime_observations_cannot_be_mixed_or_retried() {
    let runtime = page("one", json!({"type":"idle"}), Value::Null);
    let stored = stored_page("one", Value::Null);
    let mut changed_generation = runtime.clone();
    changed_generation["generation"]["generation"] = json!(2);
    let mut missing_generation = runtime.clone();
    missing_generation["generation"] = Value::Null;
    let mut stored_with_runtime_observation = runtime.clone();
    stored_with_runtime_observation["generation"] = Value::Null;
    let mut runtime_with_stored_observation = stored.clone();
    runtime_with_stored_observation["generation"] = generation();
    for (view, response) in [
        (NativeSessionView::Stored, runtime),
        (NativeSessionView::Loaded, stored.clone()),
        (NativeSessionView::Active, changed_generation),
        (NativeSessionView::Loaded, missing_generation),
        (NativeSessionView::Stored, stored_with_runtime_observation),
        (NativeSessionView::Loaded, runtime_with_stored_observation),
    ] {
        let expected = (!matches!(view, NativeSessionView::Stored))
            .then(|| serde_json::from_value(generation()).unwrap());
        let (mut client, peer) = connect_fixture(vec![("codex/sessionList", response)]).await;
        let mut pager = NativeInventoryPager::new(paging_request(view), expected).unwrap();
        assert!(pager.next_page(&mut client).await.is_err());
        assert!(
            pager.next_page(&mut client).await.is_err(),
            "a rejected pager cannot issue a retry or report exhaustion"
        );
        client.close().await.unwrap();
        assert_eq!(peer.await.unwrap().len(), 1);
    }
    for view in [
        NativeSessionView::Stored,
        NativeSessionView::Loaded,
        NativeSessionView::Active,
    ] {
        let wrong_context = matches!(view, NativeSessionView::Stored)
            .then(|| serde_json::from_value(generation()).unwrap());
        assert!(NativeInventoryPager::new(paging_request(view), wrong_context).is_err());
    }
}

#[tokio::test]
async fn foreign_endpoint_and_service_pages_are_rejected_by_the_public_client() {
    for foreign in [
        json!({"serviceId":"00000000-0000-4000-8000-000000000003","endpointId":"codex-local"}),
        json!({"serviceId":SERVICE,"endpointId":"codex-other"}),
    ] {
        let mut response = stored_page("equal-id", Value::Null);
        response["sessions"][0]["target"]["endpoint"] = foreign;
        let (mut client, peer) = connect_fixture(vec![("codex/sessionList", response)]).await;
        let mut pager =
            NativeInventoryPager::new(paging_request(NativeSessionView::Stored), None).unwrap();
        assert!(matches!(
            pager.next_page(&mut client).await,
            Err(ClientError::Protocol("inconsistent session inventory"))
        ));
        client.close().await.unwrap();
        assert_eq!(peer.await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn repeated_oversized_and_duplicate_continuations_stop_without_another_request() {
    for steps in [
        vec![(
            "codex/sessionList",
            stored_page("one", json!("x".repeat(1025))),
        )],
        vec![
            ("codex/sessionList", stored_page("one", json!("repeat"))),
            ("codex/sessionList", stored_page("two", json!("repeat"))),
        ],
        vec![
            ("codex/sessionList", stored_page("same-id", json!("next"))),
            ("codex/sessionList", stored_page("same-id", Value::Null)),
        ],
    ] {
        let expected_reads = steps.len();
        let (mut client, peer) = connect_fixture(steps).await;
        let mut pager =
            NativeInventoryPager::new(paging_request(NativeSessionView::Stored), None).unwrap();
        for index in 0..expected_reads {
            let result = pager.next_page(&mut client).await;
            if index + 1 == expected_reads {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().is_some());
            }
        }
        assert!(pager.next_page(&mut client).await.is_err());
        client.close().await.unwrap();
        assert_eq!(peer.await.unwrap().len(), expected_reads);
    }
}
