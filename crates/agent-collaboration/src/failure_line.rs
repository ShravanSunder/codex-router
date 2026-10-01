use collaboration_client::protocol::{
    DeliveryNextAction, DeliveryRejection, DeliveryRejectionReason,
};

const MAX_FAILURE_LINE_SCALARS: usize = 240;

pub(crate) fn render_failure_line(explanation: &str, next_step: &str) -> String {
    let next_step = collaboration_client::protocol::escape_push_line_field(next_step);
    let fixed_line = format!("error:  — {next_step}");
    let available_explanation = MAX_FAILURE_LINE_SCALARS.saturating_sub(fixed_line.chars().count());
    let explanation = escaped_prefix(explanation, available_explanation);
    format!("error: {explanation} — {next_step}")
}

pub(crate) fn render_held_line(push_link: &str, target: &str) -> String {
    let push_link = collaboration_client::protocol::escape_push_line_field(push_link);
    let fixed_line = format!("held: {push_link} — delivered when  is next running");
    let available_target = MAX_FAILURE_LINE_SCALARS.saturating_sub(fixed_line.chars().count());
    let target = escaped_prefix(target, available_target);
    format!("held: {push_link} — delivered when {target} is next running")
}

pub(crate) fn render_rejection_line(
    rejection: &DeliveryRejection,
    target: &str,
    push_link: &str,
) -> String {
    let detail = rejection
        .detail
        .as_deref()
        .unwrap_or_else(|| rejection_reason_label(rejection.reason));
    let explanation = format!("Delivery to {target} was rejected: {detail}");
    let next_step = rejection_next_step(rejection.reason, rejection.next_action, push_link);
    render_failure_line(&explanation, &next_step)
}

pub(crate) fn render_unknown_line(target: &str, push_link: &str) -> String {
    render_failure_line(
        &format!("Delivery outcome for {target} is unknown"),
        &format!("run agent-collaboration show {push_link} before retrying"),
    )
}

fn escaped_prefix(value: &str, maximum_scalars: usize) -> String {
    use std::fmt::Write as _;

    let mut output = String::new();
    let mut input = value.chars().peekable();
    while let Some(character) = input.next() {
        let escaped_character = match character {
            '\\' => "\\\\".to_owned(),
            '"' => "\\\"".to_owned(),
            '\r' | '\n' | '\u{2028}' | '\u{2029}' => "⏎".to_owned(),
            '·' => "\\u{b7}".to_owned(),
            value if value.is_control() => {
                let mut escaped = String::new();
                let _ = write!(escaped, "\\u{{{:x}}}", u32::from(value));
                escaped
            }
            value => value.to_string(),
        };
        let needs_ellipsis = input.peek().is_some();
        let required_scalars = escaped_character.chars().count() + usize::from(needs_ellipsis);
        if output.chars().count() + required_scalars > maximum_scalars {
            if maximum_scalars > 0 {
                output.push('…');
            }
            return output;
        }
        output.push_str(&escaped_character);
    }
    output
}

fn rejection_reason_label(reason: DeliveryRejectionReason) -> &'static str {
    match reason {
        DeliveryRejectionReason::ChildThread => "target is a child thread",
        DeliveryRejectionReason::Busy => "target is busy",
        DeliveryRejectionReason::HeldByAnotherClient => "another client holds the target",
        DeliveryRejectionReason::SettingsUnresolved => "provider settings need resolution",
        DeliveryRejectionReason::NotResumable => "target cannot be resumed",
        DeliveryRejectionReason::PermissionDenied => "target denied the request",
        DeliveryRejectionReason::UnsupportedCapability => {
            "the provider does not support this request"
        }
        DeliveryRejectionReason::Unknown => "delivery was rejected for an unknown reason",
        DeliveryRejectionReason::EndpointUnavailable => "the target endpoint is unavailable",
        DeliveryRejectionReason::NoRoute => "no delivery route is available for the target",
        DeliveryRejectionReason::LiveElsewhere => "the target is live in another terminal",
        DeliveryRejectionReason::SteerUnsupported => "the provider does not support steering",
        DeliveryRejectionReason::QueueUnsupported => "the provider does not support queueing",
        DeliveryRejectionReason::StaleGeneration => "the target changed generation",
        DeliveryRejectionReason::ProviderSessionNotFound => "the provider session was not found",
        DeliveryRejectionReason::ProviderRejected => "the provider rejected the request",
    }
}

fn rejection_next_step(
    reason: DeliveryRejectionReason,
    next_action: DeliveryNextAction,
    push_link: &str,
) -> String {
    if reason == DeliveryRejectionReason::LiveElsewhere {
        return "close one of these terminals, or run `/branch` in one of them".to_owned();
    }
    if reason == DeliveryRejectionReason::ProviderSessionNotFound {
        return "create a new conversation".to_owned();
    }
    match next_action {
        DeliveryNextAction::InspectTarget => {
            format!("run agent-collaboration show {push_link}")
        }
        DeliveryNextAction::UseDeliverySteer => "retry with --delivery steer".to_owned(),
        DeliveryNextAction::MessageFromHoldingCodexClient => {
            "message from the Codex client that holds this thread".to_owned()
        }
        DeliveryNextAction::RequestApproval => {
            "request approval from the holding client".to_owned()
        }
        DeliveryNextAction::CorrectRequest => {
            "correct the message or target and try again".to_owned()
        }
        DeliveryNextAction::RetryLater => "retry later".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_FAILURE_LINE_SCALARS, render_failure_line, render_held_line, render_unknown_line,
    };

    #[test]
    fn failure_line_includes_the_explanation_and_next_step() {
        assert_eq!(
            render_failure_line("target is ambiguous", "close one terminal"),
            "error: target is ambiguous — close one terminal"
        );
    }

    #[test]
    fn held_line_includes_the_link_and_delivery_condition() {
        assert_eq!(
            render_held_line("router://host/push/abc", "target-session"),
            "held: router://host/push/abc — delivered when target-session is next running"
        );
    }

    #[test]
    fn failure_line_escapes_controls_and_stays_within_the_line_budget() {
        let escaped = render_failure_line("failure\ntext", "retry later");
        assert!(escaped.contains("failure⏎text"));
        assert!(!escaped.contains('\n'));

        let line = render_failure_line(&"x".repeat(300), "retry later");

        assert!(line.starts_with("error: "));
        assert!(line.chars().count() <= MAX_FAILURE_LINE_SCALARS);
    }

    #[test]
    fn unknown_line_points_to_show_before_retrying() {
        assert_eq!(
            render_unknown_line("Claude target", "router://host/push/abc"),
            "error: Delivery outcome for Claude target is unknown — run agent-collaboration show router://host/push/abc before retrying"
        );
    }
}
