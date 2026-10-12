//! Real controller/renderer/HTTP restoration; fixture metadata is not a deployed native proof.
use super::*;
use crate::presentation::session_picker::{
    InventoryPageProgress, PickerSourceContext, SourceInventoryResult,
};
use crate::sessions::configured_inventory_fault_tests::{ReadPeer, ReplyScenario};
use crate::sessions::configured_inventory_read_tests::SourceApiFixture;

#[derive(Clone, Copy, Debug)]
enum PriorView {
    All,
    SameSingle,
    IndependentSingle,
    IndependentInitialFailure,
}

#[derive(Clone, Copy, Debug)]
enum InputWitnessPhase {
    Queued,
    YieldedToTerminalPump,
}

async fn verify_restored_view(prior: PriorView) {
    let failed = ReadPeer::start(if matches!(prior, PriorView::IndependentInitialFailure) {
        ReplyScenario::WrongPageEndpoint
    } else {
        ReplyScenario::RepeatedPartialThenFault
    })
    .await;
    let independent =
        SourceApiFixture::start("00000000-0000-4000-8000-000000000002", "source-model").await;
    let unfinished = ReadPeer::start_bound(
        ReplyScenario::HoldPage,
        "00000000-0000-4000-8000-000000000003",
    )
    .await;
    let mut request = picker_request();
    request.root = SessionsPickerRoot::Any;
    request.source = SessionsSource::All;
    request.records.clear();
    let mut profiles = Vec::new();
    for (source, name) in [
        (failed.request().source_context, "S"),
        (independent.request("T").source_context, "T"),
        (unfinished.request().source_context, "U"),
    ] {
        let PickerSourceContext::ConfiguredHosted(mut profile) = source else {
            panic!("configured fixture")
        };
        profile.name = name.to_owned().try_into().unwrap();
        profiles.push(profile);
    }
    request.router_registry = RouterRegistryRead::Ready(
        crate::sessions::router_connection_registry::RouterConnectionRegistry { routers: profiles },
    );
    let actual = failed.loader();
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        let actual = actual.clone();
        Box::pin(async move {
            if request.source_context == PickerSourceContext::DefaultHosted {
                SourceInventoryResult::Ready {
                    request,
                    binding: None,
                    progress: InventoryPageProgress::Complete,
                    snapshot: observed_records(Vec::new()),
                }
            } else {
                actual(request).await
            }
        })
    });
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let scenario_started = std::time::Instant::now();
    let input_witness = Arc::new(std::sync::Mutex::new(Vec::new()));
    let send_input = |event: TerminalEvent| {
        sender.send(event.clone()).unwrap();
        input_witness.lock().unwrap().push((
            scenario_started.elapsed(),
            InputWitnessPhase::Queued,
            event,
        ));
    };
    send_input(key(KeyCode::F(2)));
    // Yielded means the terminal pump received it, not that the controller handled it.
    let input = futures_util::stream::unfold(
        (receiver, input_witness.clone()),
        move |(mut receiver, witness)| async move {
            let event = receiver.recv().await?;
            witness.lock().unwrap().push((
                scenario_started.elapsed(),
                InputWitnessPhase::YieldedToTerminalPump,
                event.clone(),
            ));
            Some((event, (receiver, witness)))
        },
    );
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let mut last_frame = String::new();
    let mut stage = 0usize;
    let mut phase_milestones = vec![(Duration::ZERO, stage)];
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! {
            SessionsPickerComponent(
                request,
                record_loader: Some(loader),
                width: 200usize,
                height: 60usize,
                selected_outcome_out: &mut outcome
            )
        };
        let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(frames);
        let mut restored = false;
        while let Some(canvas) = frames.next().await {
            let text = canvas.to_string();
            last_frame = text.clone();
            if stage == 0 && text.contains("Choose machine") {
                let down = match prior {
                    PriorView::All => 1,
                    PriorView::SameSingle => 2,
                    PriorView::IndependentSingle | PriorView::IndependentInitialFailure => 3,
                };
                for _ in 0..down {
                    send_input(key(KeyCode::Down));
                }
                send_input(key(KeyCode::Enter));
                stage = 1;
                phase_milestones.push((scenario_started.elapsed(), stage));
            }
            if stage == 1
                && match prior {
                    PriorView::All => {
                        text.contains("Machine: All machines")
                            && text.contains("T: 1 sessions")
                            && text.contains("fetched; unfinished")
                    }
                    PriorView::SameSingle => {
                        text.contains("Machine: S") && text.contains("fetched; unfinished")
                    }
                    PriorView::IndependentSingle | PriorView::IndependentInitialFailure => {
                        text.contains("Machine: T") && text.contains("Source session")
                    }
                }
            {
                send_input(key(KeyCode::F(2)));
                stage = 2;
                phase_milestones.push((scenario_started.elapsed(), stage));
            }
            if stage == 2 && text.contains("Choose machine") {
                send_input(key(KeyCode::Down));
                send_input(key(KeyCode::Down));
                send_input(key(KeyCode::Enter));
                stage = 3;
                phase_milestones.push((scenario_started.elapsed(), stage));
            }
            if stage == 3
                && matches!(prior, PriorView::IndependentInitialFailure)
                && text.contains("previous view retained")
                && text.contains("Choose machine")
            {
                send_input(key(KeyCode::Esc));
                stage = 4;
                phase_milestones.push((scenario_started.elapsed(), stage));
            }
            if stage == 3 && text.contains("Machine: S") && text.contains("fetched; unfinished") {
                failed.release_all_held_pages();
                stage = 4;
                phase_milestones.push((scenario_started.elapsed(), stage));
            }
            if stage == 4
                && match prior {
                    PriorView::All => {
                        text.contains("Machine: All machines")
                            && text.contains("unavailable: S, U")
                            && text.contains("Source session")
                    }
                    PriorView::SameSingle => {
                        text.contains("Machine: S") && text.contains("S: Unavailable")
                    }
                    PriorView::IndependentSingle | PriorView::IndependentInitialFailure => {
                        text.contains("Machine: T")
                            && text.contains("Source session")
                            && independent.api_call_count("sessions_list") == 2
                    }
                }
            {
                restored = true;
                send_input(control_key('c'));
                stage = 5;
                phase_milestones.push((scenario_started.elapsed(), stage));
            }
        }
        restored
    })
    .await;
    let observer_elapsed = scenario_started.elapsed();
    let source_calls = failed.page_calls();
    let source_inventory_calls = failed.inventory_calls();
    let independent_calls = independent.api_call_count("sessions_list");
    let independent_inventory_calls = independent.api_call_count("endpoints_list");
    let unfinished_calls = unfinished.page_calls();
    let unfinished_inventory_calls = unfinished.inventory_calls();
    failed.finish().await;
    independent.finish().await;
    unfinished.finish().await;
    assert!(
        observed.as_ref().is_ok_and(|restored| *restored),
        "{prior:?} must restore with truthful terminal status and permitted pipeline: \
         result={observed:?}; observer_stage={stage}; elapsed={observer_elapsed:?}; \
         phase_milestones={phase_milestones:?}; input_witness={:?}; \
         S(page/inventory)={source_calls}/{source_inventory_calls}; \
         T(page/inventory)={independent_calls}/{independent_inventory_calls}; \
         U(page/inventory)={unfinished_calls}/{unfinished_inventory_calls}; last_frame={last_frame}",
        input_witness.lock().unwrap(),
    );
    assert!(outcome.is_none());
    assert_eq!(
        source_calls,
        match prior {
            PriorView::All | PriorView::SameSingle => 4,
            PriorView::IndependentSingle => 2,
            PriorView::IndependentInitialFailure => 1,
        },
        "only explicit selections read S; restoration never automatically retries it"
    );
    assert_eq!(
        independent_calls,
        match prior {
            PriorView::All => 1,
            PriorView::SameSingle => 0,
            PriorView::IndependentSingle | PriorView::IndependentInitialFailure => 2,
        }
    );
    assert_eq!(
        unfinished_calls,
        if matches!(prior, PriorView::All) {
            1
        } else {
            0
        },
        "canceled unfinished All work must not restart"
    );
}

#[tokio::test]
async fn review_restored_all_retires_unfinished_sources_without_rereading_failed_source() {
    verify_restored_view(PriorView::All).await;
}

#[tokio::test]
async fn review_restored_same_single_retires_failed_source_without_retry() {
    verify_restored_view(PriorView::SameSingle).await;
}

#[tokio::test]
async fn review_restored_independent_single_reestablishes_its_real_read_pipeline() {
    verify_restored_view(PriorView::IndependentSingle).await;
}

#[tokio::test]
async fn review_initial_failure_reestablishes_independent_prior_pipeline() {
    verify_restored_view(PriorView::IndependentInitialFailure).await;
}
