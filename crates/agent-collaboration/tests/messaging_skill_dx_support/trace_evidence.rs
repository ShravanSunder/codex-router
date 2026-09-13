use crate::proof_context::{ProofContext, ProofResult, agent_text};
use collaboration_client::protocol::{NativeSendReceipt, SessionRef};
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

pub(super) fn successful_cli_receipts(turns: &[Value]) -> Vec<NativeSendReceipt> {
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
    marker: &str,
) -> bool {
    let prefix = format!(
        "Agent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\n",
        serde_json::to_string(sender).unwrap_or_default(),
        serde_json::to_string(recipient).unwrap_or_default()
    );
    turns.iter().any(|turn| {
        turn.get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().enumerate().any(|(index, item)| {
                    user_text(item)
                        .is_some_and(|text| text.starts_with(&prefix) && text.contains(marker))
                        && items.iter().skip(index + 1).any(|later| {
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
        let receipt = json!({
            "kind":"result",
            "result":{
                "target":recipient,
                "generation":{
                    "serviceEpoch":"00000000-0000-4000-8000-000000000004",
                    "generation":1
                },
                "inputKind":"agent",
                "representation":"declaredAgentText",
                "clientUserMessageId":"fixture-message",
                "resumeEffect":"accepted",
                "acceptance":{
                    "kind":"nativeInputAccepted",
                    "operation":"turnStart",
                    "disposition":"startedOrSteered",
                    "turnId":"fixture-turn"
                }
            }
        });
        let incoming = format!(
            "Agent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\nFIXTURE_MARKER",
            serde_json::to_string(&sender).expect("sender JSON"),
            serde_json::to_string(&recipient).expect("recipient JSON")
        );
        let turns = vec![json!({
            "status":"completed",
            "items":[
                {"type":"commandExecution","command":"agent-collaboration message send --json","exitCode":0,"aggregatedOutput":format!("{receipt}\n")},
                {"type":"userMessage","content":[{"type":"inputText","text":incoming}]},
                {"type":"agentMessage","text":"handled"}
            ]
        })];

        assert!(used_messaging_cli(&turns));
        assert_eq!(successful_cli_receipts(&turns).len(), 1);
        assert!(has_incoming_message_followed_by_agent_text(
            &turns,
            &sender,
            &recipient,
            "FIXTURE_MARKER"
        ));
        require_terminal(&turns, "fixture").expect("completed terminal evidence");
    }
}
