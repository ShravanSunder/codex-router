//! Select the completed native assistant item that represents one turn's final reply.
use collaboration_protocol::{MAX_CONTROL_FRAME_BYTES, MessageText};
use serde_json::{Value, json};

enum ReplyCandidate {
    Available(String),
    Unavailable(&'static str),
}

#[derive(Default)]
pub(crate) struct FinalReplySelection {
    plan: Option<ReplyCandidate>,
    final_answer: Option<ReplyCandidate>,
    unphased: Option<ReplyCandidate>,
}

impl FinalReplySelection {
    pub(crate) const fn new() -> Self {
        Self {
            plan: None,
            final_answer: None,
            unphased: None,
        }
    }

    pub(crate) fn observe_completed_item(
        &mut self,
        session_id: &str,
        turn_id: &str,
        notification: &Value,
    ) {
        if notification.get("method").and_then(Value::as_str) != Some("item/completed") {
            return;
        }
        let Some(params) = notification.get("params") else {
            return;
        };
        if params.get("threadId").and_then(Value::as_str) != Some(session_id)
            || params.get("turnId").and_then(Value::as_str) != Some(turn_id)
        {
            return;
        }
        let Some(item) = params.get("item") else {
            return;
        };
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            return;
        };

        match item.get("type").and_then(Value::as_str) {
            Some("plan") => self.plan = Some(ReplyCandidate::from_text(text)),
            Some("agentMessage") => match item.get("phase") {
                Some(phase) if phase.as_str() == Some("final_answer") => {
                    self.final_answer = Some(ReplyCandidate::from_text(text));
                }
                None | Some(Value::Null) => {
                    self.unphased = Some(ReplyCandidate::from_text(text));
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub(crate) fn metadata(&self) -> Value {
        match self
            .plan
            .as_ref()
            .or(self.final_answer.as_ref())
            .or(self.unphased.as_ref())
        {
            None => json!({"kind":"available","text":null}),
            Some(ReplyCandidate::Available(text)) => {
                json!({"kind":"available","text":text})
            }
            Some(ReplyCandidate::Unavailable(reason)) => {
                json!({"kind":"unavailable","reason":reason})
            }
        }
    }
}

impl ReplyCandidate {
    fn from_text(text: &str) -> Self {
        if text.len() > MAX_CONTROL_FRAME_BYTES {
            return Self::Unavailable("outputLimitExceeded");
        }
        if MessageText::try_from(text.to_owned()).is_err() {
            return Self::Unavailable("outputInvalid");
        }
        Self::Available(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::FinalReplySelection;
    use serde_json::{Value, json};

    fn completed_message(phase: Option<&str>, text: &str) -> Value {
        let mut item = json!({"type":"agentMessage","id":"message","text":text});
        if let Some(phase) = phase {
            item["phase"] = json!(phase);
        }
        json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":item}})
    }

    fn completed_plan(text: &str) -> Value {
        json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":{"type":"plan","id":"plan","text":text}}})
    }

    fn observe(selection: &mut FinalReplySelection, notification: &Value) {
        selection.observe_completed_item("thread-a", "turn-a", notification);
    }

    #[test]
    fn final_answer_wins_over_commentary_tool_activity_and_earlier_final_answers() {
        let mut selection = FinalReplySelection::new();
        observe(
            &mut selection,
            &completed_message(Some("commentary"), "thinking aloud"),
        );
        observe(
            &mut selection,
            &json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":{"type":"commandExecution","id":"tool-a","command":"cargo test","status":"completed"}}}),
        );
        observe(
            &mut selection,
            &completed_message(Some("final_answer"), "first answer"),
        );
        // No delta accompanies this completed item; its full text must be selected.
        observe(
            &mut selection,
            &completed_message(Some("final_answer"), "last answer"),
        );
        selection.observe_completed_item(
            "thread-a",
            "turn-a",
            &json!({"method":"item/completed","params":{"threadId":"another-thread","turnId":"turn-a","item":{"type":"agentMessage","phase":"final_answer","text":"unrelated"}}}),
        );

        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":"last answer"})
        );
    }

    #[test]
    fn absent_phase_falls_back_to_last_unphased_message_and_never_commentary() {
        let mut selection = FinalReplySelection::new();
        observe(&mut selection, &completed_message(None, "first unphased"));
        let mut null_phase = completed_message(None, "null phase");
        null_phase["params"]["item"]["phase"] = Value::Null;
        observe(&mut selection, &null_phase);
        observe(
            &mut selection,
            &completed_message(Some("commentary"), "commentary"),
        );
        observe(&mut selection, &completed_message(None, "last unphased"));

        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":"last unphased"})
        );
    }

    #[test]
    fn commentary_without_a_reply_produces_available_null() {
        let mut selection = FinalReplySelection::new();
        observe(
            &mut selection,
            &completed_message(Some("commentary"), "not the reply"),
        );

        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":null})
        );
    }

    #[test]
    fn size_limit_applies_to_selected_reply_in_utf8_bytes() {
        let mut selection = FinalReplySelection::new();
        observe(
            &mut selection,
            &completed_message(Some("commentary"), &"c".repeat(1_048_577)),
        );
        observe(
            &mut selection,
            &completed_message(Some("final_answer"), "small answer"),
        );
        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":"small answer"})
        );

        let mut exact_limit = FinalReplySelection::new();
        observe(
            &mut exact_limit,
            &completed_message(Some("final_answer"), &"🪿".repeat(262_144)),
        );
        assert_eq!(exact_limit.metadata()["kind"], "available");
        assert_eq!(
            exact_limit.metadata()["text"].as_str().unwrap().len(),
            1_048_576
        );

        let mut over_limit = FinalReplySelection::new();
        observe(
            &mut over_limit,
            &completed_message(
                Some("final_answer"),
                &format!("{}🪿", "o".repeat(1_048_573)),
            ),
        );
        assert_eq!(
            over_limit.metadata(),
            json!({"kind":"unavailable","reason":"outputLimitExceeded"})
        );
    }

    #[test]
    fn invalid_control_content_is_unavailable_without_rewriting_text() {
        let mut selection = FinalReplySelection::new();
        observe(
            &mut selection,
            &completed_message(Some("final_answer"), "before\u{0001}after"),
        );

        assert_eq!(
            selection.metadata(),
            json!({"kind":"unavailable","reason":"outputInvalid"})
        );
    }

    #[test]
    fn selected_empty_text_is_invalid_and_distinct_from_no_selected_item() {
        let mut selection = FinalReplySelection::new();
        observe(&mut selection, &completed_message(Some("final_answer"), ""));

        assert_eq!(
            selection.metadata(),
            json!({"kind":"unavailable","reason":"outputInvalid"})
        );
        assert_eq!(
            FinalReplySelection::new().metadata(),
            json!({"kind":"available","text":null})
        );
    }

    #[test]
    fn plan_only_turn_returns_its_last_completed_plan() {
        let mut selection = FinalReplySelection::new();
        observe(&mut selection, &completed_plan("first plan"));
        observe(&mut selection, &completed_plan("final plan"));

        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":"final plan"})
        );
    }

    #[test]
    fn last_completed_plan_wins_over_surrounding_agent_prose() {
        let mut selection = FinalReplySelection::new();
        observe(
            &mut selection,
            &completed_message(Some("final_answer"), "prose before plan"),
        );
        observe(&mut selection, &completed_plan("first plan"));
        observe(
            &mut selection,
            &completed_message(Some("final_answer"), "prose after plan"),
        );
        observe(&mut selection, &completed_plan("last plan"));

        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":"last plan"})
        );
    }

    #[test]
    fn plan_updated_snapshots_are_never_reply_candidates() {
        let mut selection = FinalReplySelection::new();
        observe(
            &mut selection,
            &json!({"method":"turn/plan/updated","params":{"threadId":"thread-a","turnId":"turn-a","plan":[{"step":"snapshot plan"}]}}),
        );

        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":null})
        );

        observe(&mut selection, &completed_plan("completed plan"));
        observe(
            &mut selection,
            &json!({"method":"turn/plan/updated","params":{"threadId":"thread-a","turnId":"turn-a","plan":[{"step":"later progress snapshot"}]}}),
        );

        assert_eq!(
            selection.metadata(),
            json!({"kind":"available","text":"completed plan"})
        );
    }

    #[test]
    fn selected_plan_uses_the_same_size_and_content_validation_as_messages() {
        let mut empty_plan = FinalReplySelection::new();
        observe(&mut empty_plan, &completed_plan(""));
        assert_eq!(
            empty_plan.metadata(),
            json!({"kind":"unavailable","reason":"outputInvalid"})
        );

        let mut oversized_plan = FinalReplySelection::new();
        observe(&mut oversized_plan, &completed_plan(&"p".repeat(1_048_577)));
        assert_eq!(
            oversized_plan.metadata(),
            json!({"kind":"unavailable","reason":"outputLimitExceeded"})
        );

        let mut invalid_plan = FinalReplySelection::new();
        observe(&mut invalid_plan, &completed_plan("bad\u{0001}plan"));
        assert_eq!(
            invalid_plan.metadata(),
            json!({"kind":"unavailable","reason":"outputInvalid"})
        );
    }
}
