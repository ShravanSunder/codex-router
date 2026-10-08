//! The response budget Control hands the typed operations reproduces its whole-frame check:
//! a result fits exactly when its complete JSON-RPC response fits one Control frame.
use super::control_result_budget;
use collaboration_protocol::{AutomationPage, MAX_CONTROL_FRAME_BYTES};
use serde_json::{Value, json};

fn response_bytes(id: &Value, result: &impl serde::Serialize) -> usize {
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":result}))
        .expect("response encodes")
        .len()
}

fn page_of(text_bytes: usize) -> AutomationPage<String> {
    AutomationPage {
        records: vec!["x".repeat(text_bytes)],
        next_cursor: None,
    }
}

#[test]
fn the_budget_admits_exactly_the_results_whose_response_fits_one_frame() {
    // Arrange: ids of different encoded sizes, including a maximally escaped one.
    let ids = [
        json!("1"),
        json!(7),
        json!(null),
        json!("\u{0001}".repeat(128)),
        json!("répondre-é"),
    ];
    for id in ids {
        let budget = control_result_budget(&id);
        let base = response_bytes(&id, &page_of(0));
        for frame_bytes in [
            MAX_CONTROL_FRAME_BYTES - 1,
            MAX_CONTROL_FRAME_BYTES,
            MAX_CONTROL_FRAME_BYTES + 1,
        ] {
            let page = page_of(frame_bytes - base);

            // Act.
            let admitted = budget.admits(&page);

            // Assert.
            assert_eq!(response_bytes(&id, &page), frame_bytes);
            assert_eq!(budget.response_bytes(&page), Some(frame_bytes), "id {id}");
            assert_eq!(admitted, frame_bytes <= MAX_CONTROL_FRAME_BYTES, "id {id}");
        }
    }
}
