use collaboration_service::ServiceIdentity;
use serde_json::{Value, json};
#[path = "../support/served_api.rs"]
pub mod served_api;

/// Sends each `{"id", "method", "params"}` request as one call of the matching board tool and
/// answers `{"id", "result"}` or `{"id", "error": {"code", "message", "data"}}` in order.
pub async fn send_raw_board_requests(
    identity: ServiceIdentity,
    requests: Vec<Value>,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let served = served_api::ServedApi::start(identity).await?;
    let mut responses = Vec::with_capacity(requests.len());
    for request in requests {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or("board request method missing")?;
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let mut response = served.call(&tool_name(method), params).await?;
        if let (Some(fields), Some(id)) = (response.as_object_mut(), request.get("id")) {
            fields.insert("id".to_owned(), id.clone());
        }
        responses.push(response);
    }
    served.stop().await?;
    Ok(responses)
}

/// `board/projectCreate` names the tool `board_project_create`.
fn tool_name(method: &str) -> String {
    let mut tool = String::with_capacity(method.len() + 4);
    for character in method.chars() {
        if character == '/' {
            tool.push('_');
        } else if character.is_ascii_uppercase() {
            tool.push('_');
            tool.push(character.to_ascii_lowercase());
        } else {
            tool.push(character);
        }
    }
    tool
}
