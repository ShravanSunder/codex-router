use super::quota_browse_presentation_test::quota_two_account_view_model;
use super::quota_status_component::QuotaStatusComponent;
use codex_router_core::ids::AccountId;
use futures_util::StreamExt;
use iocraft::prelude::*;
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "explicit actual quota component pipeline measurement"]
async fn quota_component_performance_measurements() {
    for (width, height) in [(48usize, 24usize), (76, 30), (120, 36), (160, 40)] {
        for accounts in [10, 1000] {
            let mut model = quota_two_account_view_model();
            let template = model.rows[0].clone();
            model.rows = (0..accounts)
                .map(|index| {
                    let mut row = template.clone();
                    row.account_id = AccountId::new(format!("benchmark-{index}")).unwrap();
                    row.account = format!("Account {index}");
                    row.selected = index == 0;
                    row
                })
                .collect();
            for (screen, opening_key, marker) in [
                ("browse", None, "Quota status"),
                ("filters", Some(KeyCode::F(3)), "Account provider"),
                ("details", Some(KeyCode::F(4)), "Account details"),
            ] {
                let mut samples = Vec::new();
                for _ in 0..15 {
                    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
                        receiver.recv().await.map(|event| (event, receiver))
                    });
                    let mut component =
                        element!(QuotaStatusComponent(view_model:model.clone(),width,height));
                    let start = Instant::now();
                    let frames = component
                        .mock_terminal_render_loop(MockTerminalConfig::with_events(events));
                    tokio::pin!(frames);
                    let mut opened = false;
                    let mut observed = false;
                    let journey = async {
                        while let Some(canvas) = frames.next().await {
                            let frame = canvas.to_string();
                            if let Some(opening_key) = opening_key.filter(|_| !opened) {
                                sender
                                    .send(TerminalEvent::Key(KeyEvent::new(
                                        KeyEventKind::Press,
                                        opening_key,
                                    )))
                                    .unwrap();
                                opened = true;
                            } else if !observed && frame.contains(marker) {
                                let mut quit =
                                    KeyEvent::new(KeyEventKind::Press, KeyCode::Char('d'));
                                quit.modifiers = KeyModifiers::CONTROL;
                                sender.send(TerminalEvent::Key(quit)).unwrap();
                                observed = true;
                            }
                            std::hint::black_box(frame);
                        }
                    };
                    tokio::time::timeout(Duration::from_secs(2), journey)
                        .await
                        .unwrap();
                    assert!(observed, "the measured {screen} must actually render");
                    samples.push(start.elapsed().as_micros());
                }
                samples.sort_unstable();
                println!(
                    "quota_pipeline width={width} height={height} accounts={accounts} screen={screen} samples=15 p50_us={} p95_us={} max_us={}",
                    samples[7], samples[14], samples[14]
                );
            }
        }
    }
}
