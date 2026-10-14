use super::*;

fn large_picker_request(count: usize) -> SessionsPickerRequest {
    let mut request = picker_request();
    request.root = SessionsRoot::Any;
    request.source = SessionsSource::All;
    request.records = (0..count)
        .map(|index| {
            let mut record = picker_record(
                &format!("performance-{index}"),
                &format!("Account session {index}"),
                "/repo/project-a",
                "codex-router",
                "cli",
            );
            record.conversation.snippets = (0..10)
                .map(|index| format!("Snippet {index}: {}", "bounded message ".repeat(10)))
                .collect();
            record
        })
        .collect();
    request
}

#[test]
fn large_inventory_visible_budget_preserves_literal_row_contract() {
    let mut model = SessionsPickerModel::new(large_picker_request(1000), 120);
    assert_eq!(
        session_visible_row_budget(&model, 30),
        7,
        "top window includes outlined New row and more-below marker"
    );
    model.handle_key(SessionsPickerKey::MoveLast);
    assert_eq!(
        session_visible_row_budget(&model, 30),
        8,
        "last window uses more-above marker without New row"
    );
}

#[tokio::test]
#[ignore = "explicit real component pipeline performance measurement"]
async fn picker_component_performance_measurements() {
    for (width, height) in [(48usize, 18usize), (76, 30), (120, 36), (160, 40)] {
        for records in [10, 1000] {
            let request = large_picker_request(records);
            for open_details in [false, true] {
                let mut samples = Vec::new();
                for _ in 0..15 {
                    let start = std::time::Instant::now();
                    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
                        receiver.recv().await.map(|event| (event, receiver))
                    });
                    let mut ui =
                        element!(SessionsPickerComponent(request:request.clone(),width,height));
                    let frames =
                        ui.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
                    tokio::pin!(frames);
                    let mut opened = false;
                    let mut observed = false;
                    let journey = async {
                        while let Some(canvas) = frames.next().await {
                            let frame = canvas.to_string();
                            if !opened && open_details {
                                sender
                                    .send(TerminalEvent::Key(KeyEvent::new(
                                        KeyEventKind::Press,
                                        KeyCode::F(4),
                                    )))
                                    .unwrap();
                                opened = true;
                            } else if !observed
                                && (!open_details || frame.contains("Session details"))
                            {
                                sender.send(ctrl_key('d')).unwrap();
                                observed = true;
                            }
                            std::hint::black_box(frame);
                        }
                    };
                    tokio::time::timeout(Duration::from_secs(2), journey)
                        .await
                        .unwrap();
                    assert!(observed, "the measured details frame must actually render");
                    samples.push(start.elapsed().as_micros());
                }
                samples.sort_unstable();
                println!(
                    "picker_pipeline width={width} height={height} records={records} details={open_details} samples=15 p50_us={} p95_us={} max_us={}",
                    samples.get(7).unwrap(),
                    samples.get(14).unwrap(),
                    samples.last().unwrap()
                );
            }
        }
    }
}
