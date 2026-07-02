use iocraft::prelude::*;

const SIDECAR_BREAKPOINT: u16 = 96;
const FULL_DETAIL_BREAKPOINT: u16 = 128;
const QUOTA_SIDECAR_BREAKPOINT: u16 = 140;

fn accent_color() -> Color {
    Color::Rgb {
        r: 126,
        g: 231,
        b: 242,
    }
}

fn panel_border_color() -> Color {
    Color::Rgb {
        r: 168,
        g: 181,
        b: 218,
    }
}

fn selected_background_color() -> Color {
    Color::Rgb {
        r: 58,
        g: 70,
        b: 122,
    }
}

fn selected_text_color() -> Color {
    Color::Rgb {
        r: 255,
        g: 224,
        b: 102,
    }
}

fn success_color() -> Color {
    Color::Rgb {
        r: 107,
        g: 226,
        b: 141,
    }
}

fn warning_color() -> Color {
    Color::Rgb {
        r: 255,
        g: 193,
        b: 111,
    }
}

fn main() {
    let surface = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sessions".to_owned());

    match surface.as_str() {
        "quota" => {
            let mut element = element!(QuotaSurface);
            element.print();
        }
        _ => {
            let mut element = element!(SessionsSurface);
            element.print();
        }
    }
}

#[component]
fn SessionsSurface(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let (terminal_width, terminal_height) = hooks.use_terminal_size();
    let show_detail_sidecar = terminal_width >= SIDECAR_BREAKPOINT;
    let compact_detail = terminal_width < FULL_DETAIL_BREAKPOINT;
    let terminal_height = u32::from(terminal_height);
    let minimum_height = if show_detail_sidecar { 30 } else { 43 };
    let root_height = terminal_height.saturating_sub(2).max(minimum_height);
    let show_header_filters = terminal_width >= 120;
    let list_height = if show_detail_sidecar {
        root_height.saturating_sub(9).max(19)
    } else {
        22
    };
    let detail_height = if show_detail_sidecar {
        list_height
    } else {
        root_height.saturating_sub(list_height + 9).max(14)
    };

    element! {
        View(
            width: u32::from(terminal_width),
            height: root_height,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: accent_color(),
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            padding_bottom: 1,
        ) {
            View(width: 100pct) {
                Text(content: "Resume a previous session", color: accent_color(), weight: Weight::Bold)
            }
            View(width: 100pct, justify_content: JustifyContent::SpaceBetween, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                Text(content: "Type to search", color: Color::DarkGrey)
                #(if show_header_filters {
                    element! {
                        Text(content: "Scope: [📂 cwd]  ⎇ worktree  repo  all     Threads: [interactive]     Sort: [updated]", color: Color::Grey)
                    }.into_any()
                } else {
                    element! {
                        Text(content: "")
                    }.into_any()
                })
            }
            #(if show_detail_sidecar {
                element! {
                    View(width: 100pct) {
                        #(session_list_panel(Size::Percent(66.0), list_height))
                        View(width: 3) { Text(content: "") }
                        #(session_detail_panel(Size::Percent(29.0), detail_height, compact_detail))
                    }
                }.into_any()
            } else {
                element! {
                    View(width: 100pct, flex_direction: FlexDirection::Column) {
                        #(session_list_panel(Size::Percent(100.0), list_height))
                        #(session_detail_panel(Size::Percent(100.0), detail_height, true))
                    }
                }.into_any()
            })
            #(sessions_footer(terminal_width))
        }
    }
}

fn session_rows() -> Vec<AnyElement<'static>> {
    vec![
        session_row(SessionRowProps {
            title: "pull main origin please",
            updated: "1d ago",
            created: "1d ago",
            branch: "master",
            cwd: "~/dev/open-source/ai-dev/codex-router",
            selected: true,
        }),
        list_gap(),
        session_row(SessionRowProps {
            title: "yo how are we doing in ai skills and orchestrate look",
            updated: "10d ago",
            created: "10d ago",
            branch: "master",
            cwd: "~/dev/ai-tools",
            selected: false,
        }),
        list_gap(),
        session_row(SessionRowProps {
            title: "resume",
            updated: "12d ago",
            created: "12d ago",
            branch: "master",
            cwd: "~/dev/open-source/ai-dev/codex-router",
            selected: false,
        }),
        list_gap(),
        session_row(SessionRowProps {
            title: "When you upgrade from pnpm 10 to pnpm 11",
            updated: "15d ago",
            created: "15d ago",
            branch: "master",
            cwd: "~/dev/devfiles",
            selected: false,
        }),
        list_gap(),
        session_row(SessionRowProps {
            title: "ok doign a review of the dev workflow skillos",
            updated: "15d ago",
            created: "15d ago",
            branch: "master",
            cwd: "~/dev/ai-tools",
            selected: false,
        }),
    ]
}

fn list_gap() -> AnyElement<'static> {
    element! {
        View(width: 100pct, height: 1) {
            Text(content: "")
        }
    }
    .into_any()
}

fn session_list_panel(width: Size, height: u32) -> AnyElement<'static> {
    element! {
        View(
            width,
            min_width: 0,
            height,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: panel_border_color(),
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            padding_bottom: 1,
        ) {
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                #(session_header())
            }
            #(session_rows())
        }
    }
    .into_any()
}

fn session_header() -> AnyElement<'static> {
    element! {
        View(width: 100pct) {
            #(cell("", 3, accent_color(), Weight::Bold))
            #(cell("Session", 44, accent_color(), Weight::Bold))
            #(cell("Updated", 11, accent_color(), Weight::Bold))
            #(cell("Created", 11, accent_color(), Weight::Bold))
        }
    }
    .into_any()
}

fn sessions_footer(terminal_width: u16) -> AnyElement<'static> {
    if terminal_width < SIDECAR_BREAKPOINT {
        element! {
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Top, border_color: Color::DarkGrey) {
                Text(content: "type search    enter resume    ", color: Color::Grey)
                Text(content: "⌘N new", color: selected_text_color(), weight: Weight::Bold)
                Text(content: "    esc exit", color: Color::Grey)
            }
        }
        .into_any()
    } else if terminal_width < 120 {
        element! {
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Top, border_color: Color::DarkGrey) {
                Text(content: "type search    enter resume    ", color: Color::Grey)
                Text(content: "⌘N new thread", color: selected_text_color(), weight: Weight::Bold)
                Text(content: "    esc exit    tab focus", color: Color::Grey)
            }
        }
        .into_any()
    } else {
        element! {
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Top, border_color: Color::DarkGrey) {
                Text(content: "type search    enter resume    ", color: Color::Grey)
                Text(content: "⌘N new thread", color: selected_text_color(), weight: Weight::Bold)
                Text(content: "    esc exit    tab focus    ←/→ option    ↑/↓ browse", color: Color::Grey)
            }
        }
        .into_any()
    }
}

struct SessionRowProps<'a> {
    title: &'a str,
    updated: &'a str,
    created: &'a str,
    branch: &'a str,
    cwd: &'a str,
    selected: bool,
}

fn session_row(props: SessionRowProps<'static>) -> AnyElement<'static> {
    let background_color = props.selected.then(selected_background_color);
    let marker = if props.selected { "❯" } else { " " };
    let title_color = if props.selected {
        selected_text_color()
    } else {
        Color::White
    };
    element! {
        View(
            width: 100pct,
            flex_direction: FlexDirection::Column,
            background_color,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(width: 100pct) {
                #(cell(marker, 3, title_color, Weight::Bold))
                #(cell(props.title, 44, title_color, Weight::Bold))
                #(cell(props.updated, 11, Color::Grey, Weight::Normal))
                #(cell(props.created, 11, Color::Grey, Weight::Normal))
            }
            View(width: 100pct) {
                #(cell("", 3, Color::Grey, Weight::Normal))
                #(cell("⎇", 3, Color::Grey, Weight::Normal))
                #(cell(props.branch, 12, Color::Grey, Weight::Normal))
                #(cell("📂", 3, Color::Grey, Weight::Normal))
                Text(content: props.cwd, color: Color::Grey)
            }
        }
    }
    .into_any()
}

fn session_detail_panel(width: Size, height: u32, compact: bool) -> AnyElement<'static> {
    element! {
        View(
            width,
            min_width: 28,
            height,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: panel_border_color(),
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            padding_bottom: 1,
        ) {
            Text(content: "Preview", color: accent_color(), weight: Weight::Bold)
            Text(content: "pull main origin please", color: selected_text_color(), weight: Weight::Bold)
            Text(content: "checking branch and upstream state", color: Color::Grey)
            Text(content: "keep the working tree clean", color: Color::Grey)
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                Text(content: "")
            }
            Text(content: "Conversation", color: accent_color(), weight: Weight::Bold)
            #(if compact {
                element! {
                    View(width: 100pct, flex_direction: FlexDirection::Column) {
                        Text(content: "• pull main origin please", color: Color::Grey)
                        Text(content: "• checking branch and upstream state", color: Color::Grey)
                        Text(content: "• keep the working tree clean", color: Color::Grey)
                        View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                            Text(content: "")
                        }
                        Text(content: "Metadata", color: accent_color(), weight: Weight::Bold)
                        #(detail_line("id", "019f15c4-b8b4", Color::Grey))
                    }
                }.into_any()
            } else {
                element! {
                    View(width: 100pct, flex_direction: FlexDirection::Column) {
                        Text(content: "• pull main origin please", color: Color::Grey)
                        Text(content: "• checking branch and upstream state", color: Color::Grey)
                        Text(content: "• keep the working tree clean", color: Color::Grey)
                        View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                            Text(content: "")
                        }
                        Text(content: "Metadata", color: accent_color(), weight: Weight::Bold)
                        #(detail_line("provider", "codex-router", Color::Grey))
                        #(detail_line("model", "gpt-5-codex", Color::Grey))
                        #(detail_line("thread", "cli interactive", Color::Grey))
                        #(detail_line("id", "019f15c4-b8b4", Color::Grey))
                    }
                }.into_any()
            })
        }
    }
    .into_any()
}

fn detail_line(label: &'static str, value: &'static str, color: Color) -> AnyElement<'static> {
    element! {
        View(width: 100pct) {
            #(cell(label, 9, Color::Grey, Weight::Normal))
            Text(content: value, color)
        }
    }
    .into_any()
}

#[component]
fn QuotaSurface(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let (terminal_width, terminal_height) = hooks.use_terminal_size();
    let show_detail_sidecar = terminal_width >= QUOTA_SIDECAR_BREAKPOINT;
    let terminal_height = u32::from(terminal_height);
    let minimum_height = if show_detail_sidecar { 38 } else { 50 };
    let root_height = terminal_height.saturating_sub(2).max(minimum_height);
    let list_height = if show_detail_sidecar {
        root_height.saturating_sub(9).max(27)
    } else {
        18
    };
    let detail_height = if show_detail_sidecar {
        list_height
    } else {
        root_height.saturating_sub(list_height + 9).max(22)
    };
    let compact = terminal_width < FULL_DETAIL_BREAKPOINT;

    element! {
        View(
            width: u32::from(terminal_width),
            height: root_height,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: accent_color(),
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            padding_bottom: 1,
        ) {
            Text(content: "Quota status", color: accent_color(), weight: Weight::Bold)
            View(
                width: 100pct,
                border_style: BorderStyle::Single,
                border_color: panel_border_color(),
                padding_left: 1,
                padding_right: 1,
            ) {
                View(width: 100pct) {
                    #(cell("", 3, Color::Grey, Weight::Normal))
                    Text(content: "responses → ssdev", color: selected_text_color(), weight: Weight::Bold)
                    Text(content: "   [preferred]", color: selected_text_color(), weight: Weight::Bold)
                    Text(content: "   fresh 14s ago", color: success_color())
                    Text(content: "   why: safest quota", color: Color::Grey)
                }
            }
            #(if show_detail_sidecar {
                element! {
                    View(width: 100pct) {
                        #(quota_list_panel(Size::Percent(66.0), list_height, compact))
                        View(width: 3) { Text(content: "") }
                        #(quota_detail_panel(Size::Percent(30.0), detail_height, compact))
                    }
                }.into_any()
            } else {
                element! {
                    View(width: 100pct, flex_direction: FlexDirection::Column) {
                        #(quota_list_panel(Size::Percent(100.0), list_height, compact))
                        #(quota_detail_panel(Size::Percent(100.0), detail_height, true))
                    }
                }.into_any()
            })
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Top, border_color: Color::DarkGrey) {
                Text(content: "r refresh    enter route    tab details    q quit", color: Color::Grey)
            }
        }
    }
}

fn quota_accounts(compact: bool) -> Vec<AnyElement<'static>> {
    vec![
        account_status_row(
            AccountStatusProps {
                account: "askluna",
                status: "available",
                reason: "same pool",
                weekly_percent: 76,
                weekly_reset: "4d 15h",
                pace_state: "ahead 2d 17h",
                safe_pace_percent: 63,
                rate: "rate 0.43%/h (low)",
                activity: "1 client",
                guard_short: "wk guard 0%",
                _reset: "1 available",
                _note: "available by quota: same pool",
                selected: false,
            },
            compact,
        ),
        list_gap(),
        account_status_row(
            AccountStatusProps {
                account: "matches",
                status: "available",
                reason: "same pool",
                weekly_percent: 70,
                weekly_reset: "4d 15h",
                pace_state: "ahead 20h 42m",
                safe_pace_percent: 85,
                rate: "rate 0.53%/h (low)",
                activity: "0 clients",
                guard_short: "wk guard 3%",
                _reset: "1 available",
                _note: "available by quota: same pool",
                selected: false,
            },
            compact,
        ),
        list_gap(),
        account_status_row(
            AccountStatusProps {
                account: "ssdev",
                status: "preferred",
                reason: "safest quota",
                weekly_percent: 45,
                weekly_reset: "4d 15h",
                pace_state: "ahead 1d 14h",
                safe_pace_percent: 75,
                rate: "rate 0.30%/h (normal)",
                activity: "0 clients",
                guard_short: "wk guard 22%",
                _reset: "2 available",
                _note: "preferred by quota: safest quota",
                selected: true,
            },
            compact,
        ),
    ]
}

fn quota_list_panel(width: Size, height: u32, compact: bool) -> AnyElement<'static> {
    let min_width = if compact {
        Size::Length(0)
    } else {
        Size::Length(76)
    };

    element! {
        View(
            width,
            min_width,
            height,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: panel_border_color(),
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            padding_bottom: 1,
        ) {
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                #(quota_header(compact))
            }
            #(quota_accounts(compact))
        }
    }
    .into_any()
}

fn quota_header(compact: bool) -> AnyElement<'static> {
    if compact {
        element! {
            View(width: 100pct) {
                #(cell("", 3, accent_color(), Weight::Bold))
                #(cell("Account", 14, accent_color(), Weight::Bold))
                #(cell("Status", 18, accent_color(), Weight::Bold))
                #(cell("Weekly pace", 26, accent_color(), Weight::Bold))
            }
        }
        .into_any()
    } else {
        element! {
            View(width: 100pct) {
                #(cell("", 3, accent_color(), Weight::Bold))
                #(cell("Account", 14, accent_color(), Weight::Bold))
                #(cell("Status", 17, accent_color(), Weight::Bold))
                #(cell("Weekly pace", 42, accent_color(), Weight::Bold))
                #(cell("Activity", 14, accent_color(), Weight::Bold))
            }
        }
        .into_any()
    }
}

struct AccountStatusProps<'a> {
    account: &'a str,
    status: &'a str,
    reason: &'a str,
    weekly_percent: u8,
    weekly_reset: &'a str,
    pace_state: &'a str,
    safe_pace_percent: u8,
    rate: &'a str,
    activity: &'a str,
    guard_short: &'a str,
    _reset: &'a str,
    _note: &'a str,
    selected: bool,
}

fn account_status_row(props: AccountStatusProps<'static>, compact: bool) -> AnyElement<'static> {
    let background_color = props.selected.then(selected_background_color);
    let marker = if props.selected { "❯" } else { " " };
    let status_color = if props.status == "preferred" {
        success_color()
    } else if props.status.starts_with("held") {
        warning_color()
    } else {
        Color::Grey
    };
    let account_color = if props.selected {
        selected_text_color()
    } else {
        Color::White
    };
    let weekly_summary = format!(
        "{:>2}% left, reset {}",
        props.weekly_percent, props.weekly_reset
    );
    let safe_pace_summary = format!("{}% safe pace", props.safe_pace_percent);

    if compact {
        element! {
            View(
                width: 100pct,
                flex_direction: FlexDirection::Column,
                background_color,
                padding_left: 1,
                padding_right: 1,
            ) {
                View(width: 100pct) {
                    #(cell(marker, 3, selected_text_color(), Weight::Bold))
                    #(cell(props.account, 14, account_color, Weight::Bold))
                    #(cell(props.status, 18, status_color, Weight::Bold))
                    Text(content: props.reason, color: Color::Grey)
                }
                View(width: 100pct) {
                    #(cell("", 3, Color::Grey, Weight::Normal))
                    #(cell("weekly", 14, Color::Grey, Weight::Normal))
                    #(inline_quota_bar(props.weekly_percent, 12))
                    Text(content: weekly_summary, color: Color::Grey)
                }
                View(width: 100pct) {
                    #(cell("", 3, Color::Grey, Weight::Normal))
                    #(cell("pace", 14, Color::Grey, Weight::Normal))
                    Text(content: props.pace_state, color: Color::Grey)
                    Text(content: " · burn ", color: Color::Grey)
                    Text(content: safe_pace_summary, color: Color::Grey)
                }
            }
        }
        .into_any()
    } else {
        element! {
            View(
                width: 100pct,
                flex_direction: FlexDirection::Column,
                background_color,
                padding_left: 1,
                padding_right: 1,
            ) {
                View(width: 100pct) {
                    #(cell(marker, 3, selected_text_color(), Weight::Bold))
                    #(cell(props.account, 14, account_color, Weight::Bold))
                    #(cell(props.status, 17, status_color, Weight::Bold))
                    #(cell(props.reason, 42, Color::Grey, Weight::Normal))
                    Text(content: props.activity, color: Color::Grey)
                }
                View(width: 100pct) {
                    #(cell("", 3, Color::Grey, Weight::Normal))
                    #(cell("", 14, Color::Grey, Weight::Normal))
                    #(cell("", 17, Color::Grey, Weight::Normal))
                    #(inline_quota_bar(props.weekly_percent, 12))
                    #(cell(weekly_summary, 30, Color::Grey, Weight::Normal))
                    Text(content: props.guard_short, color: Color::Grey)
                }
                View(width: 100pct) {
                    #(cell("", 3, Color::Grey, Weight::Normal))
                    #(cell("", 14, Color::Grey, Weight::Normal))
                    #(cell("", 17, Color::Grey, Weight::Normal))
                    #(cell(props.pace_state, 16, Color::Grey, Weight::Normal))
                    #(cell(safe_pace_summary, 16, Color::Grey, Weight::Normal))
                    Text(content: props.rate, color: Color::Grey)
                }
            }
        }
        .into_any()
    }
}

fn quota_detail_panel(width: Size, height: u32, compact: bool) -> AnyElement<'static> {
    if compact {
        element! {
            View(
                width,
                min_width: 30,
                height,
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Round,
                border_color: panel_border_color(),
                padding_left: 1,
                padding_right: 1,
                padding_top: 1,
                padding_bottom: 1,
            ) {
                Text(content: "Selected account", color: accent_color(), weight: Weight::Bold)
                Text(content: "ssdev", color: selected_text_color(), weight: Weight::Bold)
                Text(content: "preferred · safest quota", color: Color::Grey)
                View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                    Text(content: "")
                }
                Text(content: "Quota windows", color: accent_color(), weight: Weight::Bold)
                View(width: 100pct) {
                    #(cell("5h", 5, Color::Grey, Weight::Normal))
                    #(inline_quota_bar(92, 13))
                    Text(content: "92% left, reset 4h 12m", color: Color::Grey)
                }
                View(width: 100pct) {
                    #(cell("wk", 5, Color::Grey, Weight::Normal))
                    #(inline_quota_bar(45, 13))
                    Text(content: "45% left, reset 4d 15h", color: Color::Grey)
                }
                View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                    Text(content: "")
                }
                Text(content: "Burn pace", color: accent_color(), weight: Weight::Bold)
                View(width: 100pct) {
                    #(safe_pace_meter(75, 13))
                    Text(content: "75% safe pace", color: Color::Grey)
                }
                Text(content: "ahead 1d 14h", color: Color::Grey)
                Text(content: "rate 0.30%/h total", color: Color::Grey)
                Text(content: "per-conn 0.30%/h (normal)", color: Color::Grey)
                View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                    Text(content: "")
                }
                Text(content: "activity 0 clients", color: Color::Grey)
                Text(content: "guards 5h 0% / weekly 22%", color: Color::Grey)
                Text(content: "reset  2 available", color: Color::Grey)
                Text(content: "note   preferred by quota: safest quota", color: Color::Grey)
            }
        }
        .into_any()
    } else {
        element! {
            View(
                width,
                min_width: 30,
                height,
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Round,
                border_color: panel_border_color(),
                padding_left: 1,
                padding_right: 1,
                padding_top: 1,
                padding_bottom: 1,
            ) {
                Text(content: "Selected account", color: accent_color(), weight: Weight::Bold)
                Text(content: "ssdev", color: selected_text_color(), weight: Weight::Bold)
                Text(content: "preferred · safest quota", color: Color::Grey)
                View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                    Text(content: "")
                }
                Text(content: "Quota windows", color: accent_color(), weight: Weight::Bold)
                View(width: 100pct) {
                    #(cell("5h", 8, Color::Grey, Weight::Normal))
                    #(inline_quota_bar(92, 16))
                    Text(content: "92% left, reset 4h 12m", color: Color::Grey)
                }
                View(width: 100pct) {
                    #(cell("weekly", 8, Color::Grey, Weight::Normal))
                    #(inline_quota_bar(45, 16))
                    Text(content: "45% left, reset 4d 15h", color: Color::Grey)
                }
                View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                    Text(content: "")
                }
                Text(content: "Burn pace", color: accent_color(), weight: Weight::Bold)
                View(width: 100pct) {
                    #(cell("current", 8, Color::Grey, Weight::Normal))
                    #(safe_pace_meter(75, 13))
                    Text(content: "75% safe pace", color: Color::Grey)
                }
                Text(content: "ahead   1d 14h before projected runout", color: Color::Grey)
                Text(content: "rate    0.30%/h total", color: Color::Grey)
                Text(content: "conn    0.30%/h per connection (normal)", color: Color::Grey)
                View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                    Text(content: "")
                }
                Text(content: "activity 0 clients", color: Color::Grey)
                Text(content: "guards  5h 0% / weekly 22%", color: Color::Grey)
                Text(content: "reset   2 available", color: Color::Grey)
                Text(content: "note    preferred by quota: safest quota", color: Color::Grey)
            }
        }
        .into_any()
    }
}

fn inline_quota_bar(percent: u8, width: u32) -> AnyElement<'static> {
    let filled_count = usize::from((percent + 9) / 10).min(10);
    let empty_count = 10usize.saturating_sub(filled_count);
    let bar = format!("{}{}", "█".repeat(filled_count), "░".repeat(empty_count));

    element! {
        View(width, padding_right: 1) {
            Text(content: bar, color: Color::Grey, weight: Weight::Normal)
        }
    }
    .into_any()
}

fn safe_pace_meter(percent: u8, width: u32) -> AnyElement<'static> {
    let filled_count = usize::from((percent + 9) / 10).min(10);
    let empty_count = 10usize.saturating_sub(filled_count);
    let bar = format!("[{}{}]", "■".repeat(filled_count), "□".repeat(empty_count));

    element! {
        View(width, padding_right: 1) {
            Text(content: bar, color: Color::Grey, weight: Weight::Normal)
        }
    }
    .into_any()
}

fn cell(
    content: impl Into<String>,
    width: u32,
    color: Color,
    weight: Weight,
) -> AnyElement<'static> {
    element! {
        View(width, padding_right: 1) {
            Text(content: content.into(), color, weight)
        }
    }
    .into_any()
}
