use collaboration_protocol::{QuestionAnswerParams, QuestionListParams};
use message_board::Identity;
use serde_json::json;

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
    assert!(collaboration_protocol::control_error_is_valid(
        "question/answer",
        &json!({
            "jsonrpc":"2.0","id":"client-1","error":{"code":-32050,
                "message":"Question response rejected","data":{
                    "kind":"alreadySettled","stage":"inspect","message":"Question response rejected"
                }}
        })
    ));
}
