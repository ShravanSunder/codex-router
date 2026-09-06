//! Baseline ACP content becomes ordinary native user text, never privileged instructions.
use crate::AcpSchemaCatalog;
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum PromptContentError {
    #[error("invalid ACP prompt parameters")]
    InvalidParameters,
    #[error("prompt content capability is not supported")]
    UnsupportedContent,
    #[error("prompt input capacity exceeded")]
    Capacity,
    #[error("ACP schema unavailable")]
    SchemaUnavailable,
}
pub struct TranslatedPrompt {
    session_id: String,
    input: Vec<Value>,
}
impl TranslatedPrompt {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    #[must_use]
    pub fn input(&self) -> &[Value] {
        &self.input
    }
}
pub fn translate_prompt_content(
    catalog: &mut AcpSchemaCatalog,
    params: &Value,
) -> Result<TranslatedPrompt, PromptContentError> {
    if !catalog
        .validate("PromptRequest", params)
        .map_err(|_| PromptContentError::SchemaUnavailable)?
    {
        return Err(PromptContentError::InvalidParameters);
    }
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or(PromptContentError::InvalidParameters)?;
    let prompt = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or(PromptContentError::InvalidParameters)?;
    if prompt.len() > 256 {
        return Err(PromptContentError::Capacity);
    }
    let mut input = Vec::with_capacity(prompt.len());
    let mut total_bytes = 0_usize;
    for content in prompt {
        let text = match content.get("type").and_then(Value::as_str) {
            Some("text") => content
                .get("text")
                .and_then(Value::as_str)
                .ok_or(PromptContentError::InvalidParameters)?
                .to_owned(),
            Some("resource_link") => {
                let uri = content
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or(PromptContentError::InvalidParameters)?;
                let name = content
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(PromptContentError::InvalidParameters)?;
                let description = content.get("description").and_then(Value::as_str);
                let mut text = format!("Resource: {name}\nURI: {uri}");
                if let Some(description) = description {
                    text.push('\n');
                    text.push_str(description);
                }
                text
            }
            _ => return Err(PromptContentError::UnsupportedContent),
        };
        total_bytes = total_bytes
            .checked_add(text.len())
            .filter(|bytes| *bytes <= 64 * 1024 * 1024)
            .ok_or(PromptContentError::Capacity)?;
        input.push(json!({"type":"text","text":text,"text_elements":[]}));
    }
    Ok(TranslatedPrompt {
        session_id: session_id.to_owned(),
        input,
    })
}
