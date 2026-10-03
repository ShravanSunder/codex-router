//! Parse ACP content blocks into validated provider-neutral prompt content.
use crate::CommandContent;
use serde_json::Value;

pub(crate) fn parse_prompt_content(params: &Value) -> Result<Vec<CommandContent>, ()> {
    let blocks = params.get("prompt").and_then(Value::as_array).ok_or(())?;
    blocks
        .iter()
        .map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => block
                .get("text")
                .and_then(Value::as_str)
                .ok_or(())
                .and_then(|text| CommandContent::text(text.to_owned()).map_err(|_| ())),
            Some("resource_link") => CommandContent::resource_link(
                block
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
                block
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
                block
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )
            .map_err(|_| ()),
            Some("image") => CommandContent::image(
                block
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
                block
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
                block.get("uri").and_then(Value::as_str).map(str::to_owned),
            )
            .map_err(|_| ()),
            Some("audio") => CommandContent::audio(
                block
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
                block
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
            )
            .map_err(|_| ()),
            Some("resource") => {
                let resource = block.get("resource").ok_or(())?;
                let uri = resource
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned();
                let mime_type = resource
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(text) = resource.get("text").and_then(Value::as_str) {
                    CommandContent::embedded_text(uri, mime_type, text.to_owned()).map_err(|_| ())
                } else {
                    CommandContent::embedded_blob(
                        uri,
                        mime_type,
                        resource
                            .get("blob")
                            .and_then(Value::as_str)
                            .ok_or(())?
                            .to_owned(),
                    )
                    .map_err(|_| ())
                }
            }
            _ => Err(()),
        })
        .collect()
}

#[cfg(test)]
#[path = "provider_acp_session_route_content_tests.rs"]
mod tests;
