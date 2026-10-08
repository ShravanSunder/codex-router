use collaboration_protocol::{QuestionAnswerParams, QuestionListParams};
use message_board::Identity;
use serde_json::json;

#[test]
fn question_choice_views_preserve_distinct_ids_and_duplicate_labels() {
    let value = json!({"kind":"multiChoice","fieldId":"pick","label":"Pick two","description":null,
        "required":true,"options":[{"optionId":"first","label":"Same"},{"optionId":"second","label":"Same"}],
        "min":2,"max":2});
    let field: collaboration_protocol::QuestionFieldView =
        serde_json::from_value(value.clone()).expect("view");
    assert_eq!(serde_json::to_value(field).expect("encode"), value);
    let answer: collaboration_protocol::QuestionResponse = serde_json::from_value(json!({
        "action":"answered","content":{"pick":{"selectedOptionIds":["first","second"]}}
    }))
    .expect("answer");
    assert_eq!(
        serde_json::to_value(answer).expect("encode")["content"]["pick"]["selectedOptionIds"],
        json!(["first", "second"])
    );
}

#[test]
fn question_answer_keeps_typed_actor_and_distinct_actions() {
    let answer: QuestionAnswerParams = serde_json::from_value(json!({
        "requestId":"q-1", "actor":{"kind":"human","humanId":"owner"},
        "response":{"action":"answered","content":{"count":3,"dryRun":true,"color":"blue"}}
    }))
    .expect("typed answer");
    assert!(matches!(answer.actor, Identity::Human { .. }));
    assert_eq!(
        serde_json::to_value(answer.response).expect("response")["action"],
        "answered"
    );
    for action in ["declined", "cancelled"] {
        let response: collaboration_protocol::QuestionResponse =
            serde_json::from_value(json!({"action":action})).expect("action");
        assert_eq!(
            serde_json::to_value(response).expect("encode")["action"],
            action
        );
    }
    let list: QuestionListParams = serde_json::from_value(json!({"pending":true})).expect("list");
    assert!(list.pending);
    assert!(
        serde_json::from_value::<collaboration_protocol::QuestionResponse>(json!({
            "action":"answered","content":{"count":{"nested":"object"}}
        }))
        .is_err()
    );
}
