//! Opaque page tokens are bound to their original request and consumed before transport.
use super::*;
use crate::presentation::session_picker::{InventoryPageProgress, SourceInventoryResult};

async fn first_page(
    peer: &ReadPeer,
) -> (
    SourceInventoryRequest,
    crate::presentation::session_picker::SourceContinuation,
) {
    let mut request = peer.request();
    let SourceInventoryResult::Qualified { continuation, .. } =
        peer.loader()(request.clone()).await
    else {
        panic!("metadata qualification")
    };
    request.continuation = Some(continuation);
    let SourceInventoryResult::Ready {
        progress: InventoryPageProgress::MoreAvailable { continuation },
        snapshot,
        ..
    } = peer.loader()(request.clone()).await
    else {
        panic!("nonterminal first page")
    };
    assert_eq!(snapshot.records.len(), 100);
    (request, continuation)
}

#[tokio::test]
async fn consumed_page_token_cannot_replay_a_cursor_on_the_wire() {
    let peer = ReadPeer::start(ReplyScenario::RowBudget(101)).await;
    let (mut request, continuation) = first_page(&peer).await;
    request.continuation = Some(continuation);
    let result = peer.loader()(request.clone()).await;
    assert!(matches!(
        result,
        SourceInventoryResult::Ready {
            progress: InventoryPageProgress::Complete,
            ..
        }
    ));
    assert_eq!(peer.page_calls(), 2);
    let replay = peer.loader()(request).await;
    assert!(matches!(
        replay,
        SourceInventoryResult::Rejected {
            reason: SourceInventoryRejection::InvalidInventory,
            ..
        }
    ));
    assert_eq!(
        peer.page_calls(),
        2,
        "consumption must precede any repeated HTTP request"
    );
    peer.finish().await;
}

#[tokio::test]
async fn page_token_rejects_changed_generation_profile_or_filter_without_another_read() {
    #[derive(Debug)]
    enum ChangedRequestPart {
        Generation,
        Profile,
        Filter,
    }
    for changed_part in [
        ChangedRequestPart::Generation,
        ChangedRequestPart::Profile,
        ChangedRequestPart::Filter,
    ] {
        let peer = ReadPeer::start(ReplyScenario::RowBudget(101)).await;
        let (mut request, continuation) = first_page(&peer).await;
        request.continuation = Some(continuation);
        match changed_part {
            ChangedRequestPart::Generation => request.request_generation += 1,
            ChangedRequestPart::Profile => {
                let PickerSourceContext::ConfiguredHosted(profile) = &mut request.source_context
                else {
                    panic!("configured profile")
                };
                profile.name = "Changed alias".to_owned().try_into().unwrap();
            }
            ChangedRequestPart::Filter => request.query.include_empty_sessions = true,
        }
        let result = peer.loader()(request).await;
        assert!(matches!(
            result,
            SourceInventoryResult::Rejected {
                reason: SourceInventoryRejection::StaleSnapshot,
                ..
            }
        ));
        assert_eq!(
            peer.page_calls(),
            1,
            "changed {changed_part:?} cannot reuse an old source cursor"
        );
        peer.finish().await;
    }
}
