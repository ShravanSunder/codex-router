//! Alias qualification and catalog scheduling through the actual dispatcher/HTTP worker.
use super::*;
use crate::sessions::configured_inventory_fault_tests::{ReadPeer, ReplyScenario};
use crate::sessions::configured_inventory_read_tests::SourceApiFixture;

async fn final_update(
    selected: Vec<PickerSourceContext>,
    loader: SessionsPickerRecordLoader,
) -> SourceReloadUpdate {
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query(""),
        sources: vec![],
    });
    let (published_sender, mut published) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_, update| {
            published_sender.send(update).unwrap();
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query(""),
        sources: selected,
    });
    let update = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let update = published.recv().await.unwrap();
            if update.source_progress.iter().all(|progress| {
                matches!(
                    progress.read_state,
                    SourceReadState::Complete { .. } | SourceReadState::Rejected { .. }
                )
            }) {
                break update;
            }
        }
    })
    .await
    .unwrap();
    drop(sender);
    worker.await.unwrap();
    update
}

#[tokio::test]
async fn equal_observed_aliases_share_one_genuine_catalog_and_registry_order_label() {
    for names in [
        ["First machine", "Second alias"],
        ["Second alias", "First machine"],
    ] {
        let fixture =
            SourceApiFixture::start("00000000-0000-4000-8000-000000000001", "source-model").await;
        let selected = names
            .iter()
            .map(|name| fixture.request(name).source_context)
            .collect();
        let update = final_update(selected, fixture.loader()).await;
        let SourceRecordsUpdate::Ready(snapshot) = update.records_update else {
            panic!("qualified catalog")
        };
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(
            snapshot.records[0].machine_label(),
            format!("{} (also: {})", names[0], names[1])
        );
        assert_eq!(
            fixture.api_call_count("sessions_list"),
            1,
            "aliases must be qualified separately but read only one catalog"
        );
        assert_eq!(
            fixture.api_call_count("endpoints_list"),
            3,
            "two qualifications and one page validation"
        );
        fixture.finish().await;
    }
}

#[tokio::test]
async fn conflicting_observed_epochs_or_membership_reject_both_aliases_before_catalog_reads() {
    for (epoch, scenario) in [
        (
            "00000000-0000-4000-8000-000000000004",
            ReplyScenario::RowBudget(1),
        ),
        (
            "00000000-0000-4000-8000-000000000002",
            ReplyScenario::ProviderRows,
        ),
    ] {
        let first = ReadPeer::start(ReplyScenario::RowBudget(1)).await;
        let second =
            ReadPeer::start_bound_at(scenario, "00000000-0000-4000-8000-000000000001", epoch).await;
        for reverse in [false, true] {
            let mut selected = vec![
                first.request().source_context,
                second.request().source_context,
            ];
            if reverse {
                selected.reverse();
            }
            let update = final_update(selected, first.loader()).await;
            assert!(
                update.source_progress.iter().all(|progress| matches!(
                    progress.read_state,
                    SourceReadState::Rejected {
                        reason: SourceInventoryRejection::StaleSnapshot
                    }
                )),
                "same service observed at conflicting epochs/membership cannot pick a first-success winner"
            );
            assert!(matches!(
                update.records_update,
                SourceRecordsUpdate::Rejected(SourceInventoryRejection::StaleSnapshot)
            ));
        }
        assert_eq!(
            first.page_calls() + second.page_calls(),
            0,
            "conflicting bindings must never start a catalog"
        );
        first.finish().await;
        second.finish().await;
    }
}

#[tokio::test]
async fn later_epoch_change_withdraws_every_alias_and_retains_unrelated_source_rows() {
    let changed = ReadPeer::start(ReplyScenario::HoldSecondChangedEpoch).await;
    let healthy = ReadPeer::start_bound(
        ReplyScenario::RowBudget(1),
        "00000000-0000-4000-8000-000000000003",
    )
    .await;
    let mut first = changed.request().source_context;
    let mut second = first.clone();
    if let PickerSourceContext::ConfiguredHosted(profile) = &mut first {
        profile.name = "First machine".to_owned().try_into().unwrap();
    }
    if let PickerSourceContext::ConfiguredHosted(profile) = &mut second {
        profile.name = "Second alias".to_owned().try_into().unwrap();
    }
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query(""),
        sources: vec![],
    });
    let (published_sender, mut published) = tokio::sync::mpsc::unbounded_channel();
    let loader = changed.loader();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_, update| {
            published_sender.send(update).unwrap();
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query(""),
        sources: vec![first, second, healthy.request().source_context],
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let SourceRecordsUpdate::Ready(snapshot) =
                published.recv().await.unwrap().records_update
                && snapshot.records.len() == 2
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    changed.wait_for_held_page().await;
    changed.release_held_page().await;
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            let update=published.recv().await.unwrap();
            if update.source_progress.iter().filter(|progress|matches!(progress.read_state,SourceReadState::Rejected {reason:SourceInventoryRejection::StaleSnapshot})).count()==2 {
                let SourceRecordsUpdate::Ready(snapshot)=update.records_update else {panic!("healthy source remains available")};
                assert_eq!(snapshot.records.len(),1);
                assert!(matches!(&snapshot.records[0].identity,SessionPickerIdentity::HostedCodex(target) if String::from(target.endpoint.service_id.clone())=="00000000-0000-4000-8000-000000000003"));
                break;
            }
        }
    }).await.unwrap();
    assert_eq!(
        changed.page_calls(),
        2,
        "one source cursor chain for the alias group; no replay"
    );
    drop(sender);
    worker.await.unwrap();
    changed.finish().await;
    healthy.finish().await;
}
