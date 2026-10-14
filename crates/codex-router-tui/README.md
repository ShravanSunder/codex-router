# Shared iocraft dialogs

`DialogHost` keeps the background mounted and provides its descendants an input
scope. `DialogFrame` composes an opaque positioned `View`, fixed title/actions,
and the library's `ScrollView`. `DialogAction` uses the library's `Button`.
Product state, filters, loading and commit/cancel effects stay with the caller.

```rust
use codex_router_tui::{DialogFrame, DialogHost, DialogMargins, DialogPresentation};
use iocraft::prelude::*;

fn framed_example() -> impl Into<AnyElement<'static>> {
    element! {
        DialogHost(width: 120u16, height: 36u16,
            background: vec![element!(Text(content: "Session list")).into_any()],
            dialog: Some(element! {
                DialogFrame(width: 120u16, height: 36u16,
                    title: "Session filters".to_owned(),
                    presentation: DialogPresentation::Framed {
                        maximum_width: 72, maximum_height: 14,
                    },
                    margins: DialogMargins { top: 1, right: 2, bottom: 1, left: 2 },
                    padding: 1u16, border_style: BorderStyle::Round,
                    border_color: Color::Cyan,
                    actions: vec![element!(Text(content: "Apply  Cancel")).into_any()]) {
                    Text(content: "Draft: All providers")
                }
            }.into_any()))
    }
}
```

`FullScreen` fills the host minus explicit margins. `Framed` uses configured
dimensions capped to the host minus margins. Vertical margins are rows;
horizontal margins are columns. The caller chooses presentation from its
existing responsive rules. The body scrolls while title/actions remain fixed;
enable keyboard scrolling for details, and leave selection keys with the owner
for a filter chooser. Escape invokes `on_close`; application-wide quit remains
an application decision.

Painting an overlay does **not** consume iocraft's broadcast events. Background
widgets must register through `UseDialogInteraction::use_dialog_events` or
`use_dialog_local_events`. An outer application dispatcher must handle its
active modal state before browse keys. The input scope deliberately does not
claim to intercept arbitrary uncooperative third-party callbacks.

Use `hooks.use_state` for local widget state and component-bound `use_future`
for asynchronous UI work on the caller's existing Tokio runtime. The dialog
creates no runtime, tasks, database connection or model request. Blocking I/O
does not belong in render/event callbacks.

Performance measurements are explicit ignored tests, separate from correctness
gates. The initial unoptimized local renderer measured small bodies around
1–1.6 ms p95 but a deliberately unbounded 1,000-line body around 66–68 ms p95;
that stress limitation is preserved, not labeled fast. The application session
history boundary caps recent snippets to ten, each truncated to 180 characters.
The subsequent actual component-pipeline experiment used 15 samples per case,
four viewport sizes and 10 or 1,000 typed records. Its largest observed p95 was
approximately 102 ms for a session open-and-exit journey with details, and 38 ms
for quota. These include component startup, event-driven rendering and exit in
an unoptimized local test build; they are not per-frame or native/network
latencies, nor a baseline comparison. Final-byte storyboards and independent
review remain required before delivery readiness. This generic `ScrollView` is
not a virtualized list.
