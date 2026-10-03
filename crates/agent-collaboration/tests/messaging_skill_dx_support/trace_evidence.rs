use crate::proof_context::{ProofContext, ProofResult, agent_text};
use collaboration_client::protocol::{
    DeliveryOutcome, MachineId, MachineLabel, PushDeliveryState, PushHeaderFacts, PushId,
    PushLineInput, PushMessageSendResult, PushOrigin, RouterLink, SessionRef,
    parse_push_line_header, render_push_line, session_identity,
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub(super) fn trace_value(target: &SessionRef, turns: Vec<Value>) -> Value {
    json!({"target":target,"turns":turns})
}

pub(super) fn write_trace(root: &Path, filename: &str, value: Value) -> ProofResult<()> {
    super::state::write_private_json(&root.join(filename), &value)
}

pub(super) fn require_terminal(turns: &[Value], role: &str) -> ProofResult<()> {
    let terminal = turns.last().filter(|turn| is_terminal(turn));
    match terminal
        .and_then(|turn| turn.get("status"))
        .and_then(Value::as_str)
    {
        Some("completed") => Ok(()),
        Some(status) => Err(format!("{role} ended with terminal status {status}").into()),
        None => Err(format!("{role} has no terminal turn evidence").into()),
    }
}

pub(super) async fn wait_for_terminal_with_text(
    proof: &mut ProofContext,
    target: &SessionRef,
    required_text: Option<&str>,
) -> ProofResult<Vec<Value>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let turns = proof.turns(target).await?;
        if terminal_with_text(&turns, required_text) {
            return Ok(turns);
        }
        if tokio::time::Instant::now() >= deadline {
            proof.record(
                "messagingSessionWaitExpired",
                json!({"target":target,"requiredText":required_text,"turns":turns}),
            )?;
            return Err(format!(
                "Session {} did not reach the required terminal evidence before the deadline",
                String::from(target.session_id.clone())
            )
            .into());
        }
    }
}

fn terminal_with_text(turns: &[Value], required_text: Option<&str>) -> bool {
    turns.last().is_some_and(|turn| {
        is_terminal(turn)
            && (turn.get("status").and_then(Value::as_str) != Some("completed")
                || required_text.is_none_or(|text| agent_text(turn).contains(text)))
    })
}

pub(super) fn successful_cli_receipts(turns: &[Value]) -> Vec<PushMessageSendResult> {
    command_items(turns)
        .filter(|item| item.get("exitCode").and_then(Value::as_i64) == Some(0))
        .filter_map(|item| item.get("aggregatedOutput").and_then(Value::as_str))
        .flat_map(str::lines)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|output| output.get("result").cloned())
        .filter_map(|result| serde_json::from_value(result).ok())
        .collect()
}

pub(super) fn used_messaging_cli(turns: &[Value]) -> bool {
    command_items(turns).any(|item| {
        item.get("exitCode").and_then(Value::as_i64) == Some(0)
            && item
                .get("command")
                .map(all_strings)
                .is_some_and(|command| command.contains(" message send "))
    })
}

pub(super) fn read_skill_and_messaging_reference(turns: &[Value]) -> bool {
    let output = command_items(turns)
        .filter(|item| item.get("exitCode").and_then(Value::as_i64) == Some(0))
        .filter_map(|item| item.get("aggregatedOutput"))
        .map(all_strings)
        .collect::<String>();
    output.contains("name: agent-collaboration")
        && output.contains("session discovery and messaging")
        && output.contains("this skill does not authorize messages or schedule changes")
        && output.contains("complete with the exact target and strongest observed stage")
}

pub(super) fn has_incoming_message_followed_by_agent_text(
    turns: &[Value],
    sender: &SessionRef,
    recipient: &SessionRef,
    expected: &PushMessageSendResult,
    marker: &str,
) -> bool {
    let expected_sender = session_identity(sender, None);
    let sender_session_prefix: String = String::from(sender.session_id.clone())
        .chars()
        .take(8)
        .collect();
    let sender_reference = format!(
        "{}/{}",
        String::from(sender.endpoint.endpoint_id.clone()),
        sender_session_prefix
    );
    if expected.target != *recipient
        || expected.delivery_state != PushDeliveryState::Delivered
        || !matches!(
            &expected.receipt.outcome,
            DeliveryOutcome::PeerMessageWritten
        )
    {
        return false;
    }
    turns.iter().any(|turn| {
        turn.get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().enumerate().any(|(index, item)| {
                    user_text(item).is_some_and(|text| {
                        let Some(header) = parse_push_line_header(text) else {
                            return false;
                        };
                        let Some((_, link_text)) = text.rsplit_once(" · ") else {
                            return false;
                        };
                        let Ok(link) = RouterLink::parse(link_text) else {
                            return false;
                        };
                        let sender_matches =
                            header.title.starts_with(&format!("✉️ {expected_sender}"))
                                || header.title.contains(&sender_reference);
                        header.kind == collaboration_client::protocol::PushKind::DirectMessage
                            && sender_matches
                            && String::from(recipient.endpoint.service_id.clone())
                                == link.machine_id().as_str()
                            && link.push_id() == &expected.push_id
                            && expected.link == link.to_string()
                            && text.contains(marker)
                    }) && items.iter().skip(index + 1).any(|later| {
                        later.get("type").and_then(Value::as_str) == Some("agentMessage")
                    })
                })
            })
    })
}

fn command_items(turns: &[Value]) -> impl Iterator<Item = &Value> {
    turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("commandExecution"))
}

fn is_terminal(turn: &Value) -> bool {
    matches!(
        turn.get("status").and_then(Value::as_str),
        Some("completed" | "failed" | "interrupted")
    )
}

fn user_text(item: &Value) -> Option<&str> {
    if item.get("type").and_then(Value::as_str) != Some("userMessage") {
        return None;
    }
    item.get("content")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|content| content.get("text").and_then(Value::as_str))
}

fn all_strings(value: &Value) -> String {
    match value {
        Value::String(text) => format!(" {text} "),
        Value::Array(values) => values.iter().map(all_strings).collect(),
        Value::Object(values) => values.values().map(all_strings).collect(),
        _ => String::new(),
    }
    .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session(session_id: &str) -> SessionRef {
        serde_json::from_value(json!({
            "endpoint": {
                "serviceId": "00000000-0000-4000-8000-000000000001",
                "endpointId": "codex-local"
            },
            "sessionId": session_id
        }))
        .expect("valid fixture session")
    }

    #[test]
    fn prior_completion_does_not_complete_a_current_active_turn() {
        let turns = vec![
            json!({"status":"completed","items":[]}),
            json!({"status":"inProgress","items":[{"type":"agentMessage","text":"READY"}]}),
        ];
        assert!(!terminal_with_text(&turns, Some("READY")));
        assert!(require_terminal(&turns, "current").is_err());
    }

    #[test]
    fn failed_current_turn_returns_for_failure_reporting_without_a_success_marker() {
        let turns = vec![
            json!({"status":"completed","items":[{"type":"agentMessage","text":"READY"}]}),
            json!({"status":"failed","items":[]}),
        ];
        assert!(terminal_with_text(&turns, Some("READY")));
        assert!(require_terminal(&turns, "current").is_err());
    }

    #[test]
    fn grades_cli_receipt_and_actual_incoming_message_from_history() {
        let sender = session("00000000-0000-4000-8000-000000000002");
        let recipient = session("00000000-0000-4000-8000-000000000003");
        let push_id =
            PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned()).expect("push id");
        let link = RouterLink::new(
            MachineId::from(recipient.endpoint.service_id.clone()),
            push_id.clone(),
        );
        let receipt = json!({
            "kind":"result",
            "result":{
                "pushId":push_id,
                "link":link.to_string(),
                "target":recipient,
                "targetIdentity":"🤖 codex-local/recipient",
                "deliveryState":"delivered",
                "receipt":{"outcome":{"kind":"peerMessageWritten"},"reachability":"claudeCodePeer","client":{"kind":"claudeCodePeer"}}
            }
        });
        let incoming = render_push_line(&PushLineInput {
            link,
            machine_label: MachineLabel::try_from("fixture-host".to_owned())
                .expect("machine label"),
            origin: PushOrigin::Session(sender.clone()),
            header_facts: PushHeaderFacts::DirectMessage {
                sender_display_name: None,
            },
            body: Some("FIXTURE_MARKER".to_owned()),
        })
        .expect("render prepared push line");
        let turns = vec![json!({
            "status":"completed",
            "items":[
                {"type":"commandExecution","command":"agent-collaboration message send --json","exitCode":0,"aggregatedOutput":format!("{receipt}\n")},
                {"type":"userMessage","content":[{"type":"inputText","text":incoming}]},
                {"type":"agentMessage","text":"handled"}
            ]
        })];

        assert!(used_messaging_cli(&turns));
        let send_result = successful_cli_receipts(&turns)
            .into_iter()
            .next()
            .expect("successful prepared-send result");
        assert!(has_incoming_message_followed_by_agent_text(
            &turns,
            &sender,
            &recipient,
            &send_result,
            "FIXTURE_MARKER"
        ));
        require_terminal(&turns, "fixture").expect("completed terminal evidence");
    }
}
