use collaboration_client::protocol::{
    DeliveryNextAction, DeliveryPeerClaim, DeliveryRejection, DeliveryRejectionReason,
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
    if rejection.reason == DeliveryRejectionReason::LiveElsewhere
        && let Some(claims) = rejection
            .claims
            .as_deref()
            .filter(|claims| !claims.is_empty())
    {
        return render_peer_claim_failure_line(target, claims);
    }
    let detail = rejection
        .detail
        .as_deref()
        .unwrap_or_else(|| rejection_reason_label(rejection.reason));
    let explanation = format!("Delivery to {target} was rejected: {detail}");
    let next_step = rejection_next_step(rejection.reason, rejection.next_action, push_link);
    render_failure_line(&explanation, &next_step)
}

fn render_peer_claim_failure_line(target: &str, claims: &[DeliveryPeerClaim]) -> String {
    let next_step = collaboration_client::protocol::escape_push_line_field(
        "close one of these terminals, or run `/branch` in one of them",
    );
    let footer = format!(" — {next_step}");
    let claim_count = claims.len();
    let terminal_label = if claim_count == 1 {
        "terminal"
    } else {
        "terminals"
    };
    let prefix_without_target = format!("error:  is open in {claim_count} {terminal_label}");
    let max_remainder = format!(" (+{claim_count} more)");
    let target_budget = MAX_FAILURE_LINE_SCALARS
        .saturating_sub(prefix_without_target.chars().count())
        .saturating_sub(max_remainder.chars().count())
        .saturating_sub(footer.chars().count());
    let target = escaped_prefix(target, target_budget);
    let prefix = format!("error: {target} is open in {claim_count} {terminal_label}");
    let rows = claims
        .iter()
        .map(|claim| {
            let display_name = peer_claim_display_name(claim);
            let display_name =
                collaboration_client::protocol::escape_push_line_field(&display_name);
            format!("pid {} {display_name}", claim.pid)
        })
        .collect::<Vec<_>>();

    for visible_count in (0..=rows.len()).rev() {
        let omitted_count = rows.len().saturating_sub(visible_count);
        let mut visible_rows = rows.iter().take(visible_count).cloned().collect::<Vec<_>>();
        if omitted_count > 0 {
            visible_rows.push(format!("+{omitted_count} more"));
        }
        let detail = visible_rows.join(", ");
        let explanation = if detail.is_empty() {
            prefix.clone()
        } else {
            format!("{prefix} ({detail})")
        };
        let line = format!("{explanation}{footer}");
        if line.chars().count() <= MAX_FAILURE_LINE_SCALARS {
            return line;
        }
    }

    format!("{prefix}{footer}")
}

fn peer_claim_display_name(claim: &DeliveryPeerClaim) -> String {
    let name = claim
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            claim.cwd.as_deref().and_then(|cwd| {
                std::path::Path::new(cwd)
                    .file_name()
                    .and_then(|basename| basename.to_str())
            })
        })
        .unwrap_or("unnamed terminal");
    let mut shortened = name.chars().take(24).collect::<String>();
    if name.chars().count() > 24 {
        shortened.pop();
        shortened.push('…');
    }
    shortened
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
    if reason == DeliveryRejectionReason::QueueUnsupported {
        return "resend with --delivery auto".to_owned();
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
        MAX_FAILURE_LINE_SCALARS, render_failure_line, render_held_line, render_rejection_line,
        render_unknown_line,
    };
    use collaboration_client::protocol::{
        DeliveryNextAction, DeliveryPeerClaim, DeliveryRejection, DeliveryRejectionReason,
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

    #[test]
    fn ambiguous_peer_claims_render_in_order_with_cwd_fallback() {
        let rejection = DeliveryRejection {
            reason: DeliveryRejectionReason::LiveElsewhere,
            next_action: DeliveryNextAction::InspectTarget,
            client_code: None,
            detail: Some("this Claude session is claimed by 3 live terminals".to_owned()),
            claims: Some(vec![
                DeliveryPeerClaim {
                    pid: 52304,
                    name: Some("agent-studio-pane-fixes-b4".to_owned()),
                    cwd: Some("/workspace/ignored-for-display".to_owned()),
                },
                DeliveryPeerClaim {
                    pid: 68833,
                    name: None,
                    cwd: Some("/workspace/ipc-remote-zmx".to_owned()),
                },
                DeliveryPeerClaim {
                    pid: 70001,
                    name: None,
                    cwd: None,
                },
            ]),
        };

        let line = render_rejection_line(&rejection, "fixture-session", "router://host/push/id");

        assert_eq!(
            line,
            "error: fixture-session is open in 3 terminals (pid 52304 agent-studio-pane-fixes…, pid 68833 ipc-remote-zmx, pid 70001 unnamed terminal) — close one of these terminals, or run `/branch` in one of them"
        );
        assert!(!line.contains("/workspace/"));
        assert!(line.chars().count() <= MAX_FAILURE_LINE_SCALARS);
    }

    #[test]
    fn five_long_peer_claims_keep_full_structured_data_and_fit_the_line_budget() {
        let claims = (600..605)
            .map(|pid| DeliveryPeerClaim {
                pid,
                name: Some(format!(
                    "long-terminal-name-for-peer-claim-{pid}-with-a-full-workspace-label"
                )),
                cwd: Some(format!("/workspace/terminal-{pid}")),
            })
            .collect::<Vec<_>>();
        let rejection = DeliveryRejection {
            reason: DeliveryRejectionReason::LiveElsewhere,
            next_action: DeliveryNextAction::InspectTarget,
            client_code: None,
            detail: Some("this Claude session is claimed by 5 live terminals".to_owned()),
            claims: Some(claims.clone()),
        };

        let line = render_rejection_line(&rejection, "fixture-session", "router://host/push/id");

        assert!(line.chars().count() <= MAX_FAILURE_LINE_SCALARS);
        assert!(line.contains("error: fixture-session is open in 5 terminals"));
        assert!(line.contains("pid 600"));
        assert!(line.contains("pid 601"));
        assert!(line.contains("pid 602"));
        assert!(!line.contains("pid 603"));
        assert!(!line.contains("pid 604"));
        assert!(line.contains("+2 more)"));
        let visible_pids = line
            .split("pid ")
            .skip(1)
            .map(|claim| {
                claim
                    .split_whitespace()
                    .next()
                    .expect("visible claim has a PID")
                    .parse::<u32>()
                    .expect("visible PID is numeric")
            })
            .collect::<Vec<_>>();
        assert_eq!(visible_pids, vec![600, 601, 602]);
        assert_eq!(line.matches("pid ").count(), 3);
        let remainder = line
            .rsplit_once(", +")
            .expect("omitted peers have a remainder summary")
            .1
            .split_once(" more)")
            .expect("remainder summary closes the peer list")
            .0;
        assert_eq!(remainder, "2");
        assert_eq!(
            remainder
                .parse::<usize>()
                .expect("omitted count is numeric"),
            claims.len() - visible_pids.len()
        );
        assert_eq!(line.matches(" more)").count(), 1);
        assert_eq!(rejection.claims.as_ref(), Some(&claims));
    }
}
