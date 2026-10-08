use super::*;
use crate::session_identity;

#[derive(Clone, Copy)]
struct DisplayLimits {
    machine_label: usize,
    name: Option<usize>,
    preview: Option<usize>,
}

impl DisplayLimits {
    fn for_input(input: &PushLineInput) -> Self {
        let body_scalars = input.body.as_deref().map_or(0, |body| body.chars().count());
        let preview = input
            .body
            .as_ref()
            .map(|_| body_scalars.min(PREVIEW_MAX_SCALARS));
        let name = match (&input.header_facts, &input.origin) {
            (
                PushHeaderFacts::DirectMessage {
                    sender_display_name,
                },
                PushOrigin::Session(sender),
            ) => Some(
                session_identity(sender, sender_display_name.as_ref())
                    .chars()
                    .count(),
            ),
            (
                PushHeaderFacts::Approval {
                    requester,
                    requester_display_name,
                },
                _,
            )
            | (
                PushHeaderFacts::Question {
                    requester,
                    requester_display_name,
                },
                _,
            ) => Some(
                session_identity(requester, requester_display_name.as_ref())
                    .chars()
                    .count(),
            ),
            _ => None,
        };
        Self {
            machine_label: machine_label(input).chars().count(),
            name,
            preview,
        }
    }

    fn render(&self, input: &PushLineInput) -> String {
        let raw_machine_label = machine_label(input);
        let machine_label = escaped_prefix(
            &raw_machine_label,
            self.machine_label,
            self.machine_label < raw_machine_label.chars().count(),
        );
        let mut header = match (&input.header_facts, &input.origin) {
            (
                PushHeaderFacts::DirectMessage {
                    sender_display_name,
                },
                PushOrigin::Session(sender),
            ) => {
                let name = session_identity(sender, sender_display_name.as_ref());
                let visible_name = self.name.unwrap_or_default();
                let name = escaped_prefix(&name, visible_name, visible_name < name.chars().count());
                let sender_reference = if sender_display_name.is_some() {
                    format!(
                        " ({}/{})",
                        String::from(sender.endpoint.endpoint_id.clone()),
                        short_identity(&sender.session_id)
                    )
                } else {
                    String::new()
                };
                format!("✉️ {name}{sender_reference} @{machine_label} → you")
            }
            (PushHeaderFacts::DirectMessage { .. }, PushOrigin::OwnerUnverified) => {
                format!("🧑 Owner (unverified) @{machine_label} → you")
            }
            (PushHeaderFacts::Wake, PushOrigin::Router(PushKind::Wake)) => {
                format!("⏰ Router wake @{machine_label}")
            }
            (
                PushHeaderFacts::ScheduleRun {
                    schedule_id,
                    run_id,
                },
                PushOrigin::Router(PushKind::ScheduleRun),
            ) => {
                let schedule_prefix: String = schedule_id.as_str().chars().take(8).collect();
                let run_prefix: String = run_id.as_str().chars().take(8).collect();
                format!("🗓 Router schedule {schedule_prefix} @{machine_label} · run {run_prefix}")
            }
            (
                PushHeaderFacts::Approval {
                    requester,
                    requester_display_name,
                },
                PushOrigin::Router(PushKind::Approval),
            ) => {
                let requester_name = session_identity(requester, requester_display_name.as_ref());
                let visible_name = self.name.unwrap_or_default();
                let requester_name = escaped_prefix(
                    &requester_name,
                    visible_name,
                    visible_name < requester_name.chars().count(),
                );
                format!("❓ Router approval @{machine_label} · {requester_name} asks")
            }
            (
                PushHeaderFacts::Question {
                    requester,
                    requester_display_name,
                },
                PushOrigin::Router(PushKind::Question),
            ) => {
                let requester_name = session_identity(requester, requester_display_name.as_ref());
                let visible_name = self.name.unwrap_or_default();
                let requester_name = escaped_prefix(
                    &requester_name,
                    visible_name,
                    visible_name < requester_name.chars().count(),
                );
                format!("❓ Router question @{machine_label} · {requester_name} asks")
            }
            (
                PushHeaderFacts::SubscriptionActivity {
                    root_count,
                    message_count,
                    held_since,
                    thread_resolved,
                },
                PushOrigin::Router(PushKind::SubscriptionActivity),
            ) => {
                let root_label = pluralized_count(u64::from(*root_count), "thread", "threads");
                let message_label = pluralized_count(*message_count, "message", "messages");
                let mut line = format!(
                    "🧵 Router: new thread activity @{machine_label} · {root_label} · {message_label}"
                );
                if let Some(held_since) = held_since {
                    line.push_str(" · held since ");
                    line.push_str(&escape_field(held_since));
                }
                if *thread_resolved {
                    line.push_str(" · thread resolved");
                }
                line
            }
            (
                PushHeaderFacts::SubscriptionExpiry { scope },
                PushOrigin::Router(PushKind::SubscriptionExpiry),
            ) => format!(
                "🧵 Router: subscription expired @{machine_label} · {}",
                escape_field(scope)
            ),
            _ => return String::new(),
        };

        if let Some(body) = &input.body {
            let total_scalars = body.chars().count();
            let shown_scalars = self.preview.unwrap_or_default().min(total_scalars);
            let preview = escaped_prefix(body, shown_scalars, shown_scalars < total_scalars);
            header.push_str(" · \"");
            header.push_str(&preview);
            header.push('\"');
            let omitted_scalars = total_scalars.saturating_sub(shown_scalars);
            if omitted_scalars > 0 {
                header.push_str(&format!(" (+{omitted_scalars} more chars)"));
            }
        }
        header.push_str(" · ");
        header.push_str(&input.link.to_string());
        header
    }

    fn field_limit_mut(&mut self, field: DisplayField) -> Option<&mut usize> {
        match field {
            DisplayField::MachineLabel => Some(&mut self.machine_label),
            DisplayField::Name => self.name.as_mut(),
            DisplayField::Preview => self.preview.as_mut(),
        }
    }
}

#[derive(Clone, Copy)]
enum DisplayField {
    MachineLabel,
    Name,
    Preview,
}

fn machine_label(input: &PushLineInput) -> String {
    input.machine_label.as_str().to_owned()
}

fn short_identity(identity: &crate::SessionId) -> String {
    String::from(identity.clone()).chars().take(8).collect()
}

fn pluralized_count(count: u64, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

fn escaped_prefix(value: &str, visible_scalars: usize, append_ellipsis: bool) -> String {
    let original_scalars = value.chars().count();
    let mut visible: String = value.chars().take(visible_scalars).collect();
    if append_ellipsis && visible_scalars < original_scalars {
        visible.push('…');
    }
    escape_field(&visible)
}

fn escape_field(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\r' | '\n' | '\u{2028}' | '\u{2029}' => escaped.push('⏎'),
            '·' => escaped.push_str("\\u{b7}"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{{{:x}}}", u32::from(character));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[must_use]
pub fn escape_push_line_field(value: &str) -> String {
    escape_field(value)
}

fn validate_input(input: &PushLineInput) -> Result<(), PushLineError> {
    let kind = input.header_facts.kind();
    match (&input.origin, kind) {
        (PushOrigin::Session(_), PushKind::DirectMessage)
        | (PushOrigin::OwnerUnverified, PushKind::DirectMessage) => {}
        (PushOrigin::Router(origin_kind), candidate_kind)
            if *origin_kind == candidate_kind
                && kind == candidate_kind
                && kind != PushKind::DirectMessage => {}
        _ => return Err(PushLineError::OriginKindMismatch),
    }

    let subscription_kind = matches!(
        kind,
        PushKind::SubscriptionActivity | PushKind::SubscriptionExpiry
    );
    match (subscription_kind, input.body.is_some()) {
        (false, false) => return Err(PushLineError::MissingBody),
        (true, true) => return Err(PushLineError::UnexpectedBody),
        _ => {}
    }

    match &input.header_facts {
        PushHeaderFacts::SubscriptionActivity {
            root_count,
            held_since,
            ..
        } if *root_count == 0
            || *root_count > MAX_SUBSCRIPTION_ROOTS
            || held_since
                .as_ref()
                .is_some_and(|value| value.chars().count() > MAX_HELD_SINCE_SCALARS) =>
        {
            return Err(PushLineError::InvalidHeaderFacts);
        }
        PushHeaderFacts::SubscriptionExpiry { scope }
            if scope.trim().is_empty()
                || scope.chars().count() > MAX_SUBSCRIPTION_SCOPE_SCALARS =>
        {
            return Err(PushLineError::InvalidHeaderFacts);
        }
        _ => {}
    }
    Ok(())
}

fn assemble_line(input: &PushLineInput, limits: &DisplayLimits) -> String {
    limits.render(input)
}

fn largest_fitting_prefix(
    input: &PushLineInput,
    limits: &mut DisplayLimits,
    field: DisplayField,
) -> usize {
    let Some(current_limit) = limits.field_limit_mut(field).copied() else {
        return 0;
    };
    let mut lower = 0;
    let mut upper = current_limit;
    let mut best = None;
    while lower <= upper {
        let candidate = lower + (upper - lower) / 2;
        if let Some(field_limit) = limits.field_limit_mut(field) {
            *field_limit = candidate;
        }
        if assemble_line(input, limits).len() <= MAX_PUSH_LINE_BYTES {
            best = Some(candidate);
            lower = candidate.saturating_add(1);
        } else if candidate == 0 {
            break;
        } else {
            upper = candidate - 1;
        }
    }
    let selected = best.unwrap_or(0);
    if let Some(field_limit) = limits.field_limit_mut(field) {
        *field_limit = selected;
    }
    selected
}

pub fn render_push_line(input: &PushLineInput) -> Result<String, PushLineError> {
    validate_input(input)?;
    let mut limits = DisplayLimits::for_input(input);
    let mut line = assemble_line(input, &limits);
    for field in [
        DisplayField::MachineLabel,
        DisplayField::Name,
        DisplayField::Preview,
    ] {
        if line.len() <= MAX_PUSH_LINE_BYTES {
            return Ok(line);
        }
        if limits.field_limit_mut(field).is_none() {
            continue;
        }
        largest_fitting_prefix(input, &mut limits, field);
        line = assemble_line(input, &limits);
    }
    if line.len() <= MAX_PUSH_LINE_BYTES {
        Ok(line)
    } else {
        Err(PushLineError::FixedLineExceedsBudget)
    }
}

pub fn parse_push_line_header(text: &str) -> Option<ParsedPushLineHeader> {
    let header = text
        .split_once(" · \"")
        .map(|(header, _)| header)
        .or_else(|| {
            text.rsplit_once(" · ")
                .filter(|(_, suffix)| suffix.starts_with("router://"))
                .map(|(header, _)| header)
        })
        .unwrap_or(text);
    let kind = if header.starts_with("✉️ ") || header.starts_with("🧑 Owner (unverified)") {
        PushKind::DirectMessage
    } else if header.starts_with("⏰ Router wake") {
        PushKind::Wake
    } else if header.starts_with("🗓 Router schedule") {
        PushKind::ScheduleRun
    } else if header.starts_with("❓ Router approval") {
        PushKind::Approval
    } else if header.starts_with("❓ Router question") {
        PushKind::Question
    } else if header.starts_with("🧵 Router: new thread activity") {
        PushKind::SubscriptionActivity
    } else if header.starts_with("🧵 Router: subscription expired") {
        PushKind::SubscriptionExpiry
    } else {
        return None;
    };
    Some(ParsedPushLineHeader {
        kind,
        title: header.to_owned(),
    })
}
