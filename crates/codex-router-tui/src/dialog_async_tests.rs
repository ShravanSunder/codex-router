use crate::*;
use futures_util::StreamExt;
use iocraft::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Props)]
struct AsyncDialogProps {
    #[iocraft(required)]
    receiver: tokio::sync::watch::Receiver<bool>,
    #[iocraft(required)]
    starts: Arc<AtomicUsize>,
}

#[component]
fn AsyncDialog(props: &AsyncDialogProps, mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut loaded = hooks.use_state(|| false);
    let mut clicks = hooks.use_state(|| 0_u32);
    let mut viewport = hooks.use_state(|| (80_u16, 24_u16));
    let mut exit = hooks.use_state(|| false);
    let mut system = hooks.use_context_mut::<SystemContext>();
    hooks.use_future({
        let mut receiver = props.receiver.clone();
        let starts = props.starts.clone();
        async move {
            starts.fetch_add(1, Ordering::SeqCst);
            while !*receiver.borrow_and_update() {
                if receiver.changed().await.is_err() {
                    return;
                }
            }
            loaded.set(true);
        }
    });
    hooks.use_terminal_events(move |event| match event {
        TerminalEvent::Resize(width, height) => viewport.set((width, height)),
        TerminalEvent::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) => exit.set(true),
        _ => {}
    });
    if exit.get() {
        system.exit();
    }
    let (width, height) = viewport.get();
    let status = if loaded.get() { "Loaded" } else { "Loading" };
    element! {
        DialogHost(width,height, dialog: Some(element! {
            DialogFrame(width,height, title: format!("{width}x{height} {status}"),
                presentation: if width < 60 { DialogPresentation::FullScreen } else { DialogPresentation::Framed { maximum_width: 50, maximum_height: 14 } },
                margins: DialogMargins { top: 1, right: 2, bottom: 1, left: 2 },
                actions: vec![element! {
                    DialogAction(label: format!("Apply {}", clicks.get()), has_focus: true, handler: move |_| clicks.set(clicks.get()+1))
                }.into_any()]) {
                Text(content: "Loading runs independently of dialog input")
            }
        }.into_any()))
    }
}

#[tokio::test]
async fn resize_and_apply_remain_responsive_while_tokio_loader_is_pending() {
    let (load_sender, load_receiver) = tokio::sync::watch::channel(false);
    let starts = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut ui = element!(AsyncDialog(receiver: load_receiver, starts: starts.clone()));
    let canvases = ui.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
    tokio::pin!(canvases);
    let mut phase = 0;
    let journey = async {
        while let Some(canvas) = canvases.next().await {
            let frame = canvas.to_string();
            match phase {
                0 if frame.contains("80x24 Loading") => {
                    sender.send(TerminalEvent::Resize(48, 18)).unwrap();
                    phase = 1;
                }
                1 if frame.contains("48x18 Loading") => {
                    assert!(frame.lines().count() <= 18, "{frame}");
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::Enter,
                        )))
                        .unwrap();
                    phase = 2;
                }
                2 if frame.contains("Apply 1") => {
                    assert!(
                        frame.contains("Loading"),
                        "loader must still be held: {frame}"
                    );
                    assert_eq!(starts.load(Ordering::SeqCst), 1);
                    load_sender.send(true).unwrap();
                    phase = 3;
                }
                3 if frame.contains("48x18 Loaded") => {
                    assert!(frame.contains("Apply 1"));
                    sender.send(TerminalEvent::Resize(160, 40)).unwrap();
                    phase = 4;
                }
                4 if frame.contains("160x40 Loaded") => {
                    assert_eq!(starts.load(Ordering::SeqCst), 1, "resize recreated loader");
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::Esc,
                        )))
                        .unwrap();
                    phase = 5;
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), journey)
        .await
        .unwrap();
    assert_eq!(phase, 5);
}

#[test]
#[ignore = "explicit performance measurement, not a timing-based correctness gate"]
fn dialog_render_performance_measurements() {
    for (width, height) in [(48_u16, 18_u16), (76, 30), (120, 36), (160, 40)] {
        for rows in [10, 1000] {
            let content = (0..rows)
                .map(|row| format!("ACCOUNT {row}: available quota\n"))
                .collect::<String>();
            for dialog_open in [false, true] {
                let mut samples = Vec::new();
                for _ in 0..50 {
                    let start = std::time::Instant::now();
                    let dialog=dialog_open.then(||element! {
                        DialogFrame(width,height,title:"Filters".to_owned(),presentation:DialogPresentation::Framed{maximum_width:72,maximum_height:20},
                            actions:vec![element!(Text(content:"Apply Cancel")).into_any()]) {
                            Text(content:content.clone())
                        }
                    }.into_any());
                    let output=element! {
                        DialogHost(width,height,background:vec![element! {
                            View(width:u32::from(width),height:u32::from(height),overflow:Overflow::Hidden) {
                                Text(content:"Background account list")
                            }
                        }.into_any()],dialog)
                    }.to_string();
                    std::hint::black_box(output);
                    samples.push(start.elapsed().as_micros());
                }
                samples.sort_unstable();
                println!(
                    "dialog_render width={width} height={height} rows={rows} open={dialog_open} samples=50 p50_us={} p95_us={} max_us={}",
                    samples.get(24).unwrap(),
                    samples.get(47).unwrap(),
                    samples.last().unwrap()
                );
            }
        }
    }
}
