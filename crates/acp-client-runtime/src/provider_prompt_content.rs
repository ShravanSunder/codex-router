//! Validate ACP prompt blocks against a Session's advertised content types.

use crate::ProviderCapabilityReport;
use crate::agent_session_client::ExternalProviderRuntimeError;
use agent_client_protocol::schema::v1::{
    AudioContent, BlobResourceContents, ContentBlock, EmbeddedResource, EmbeddedResourceResource,
    ImageContent, ResourceLink, TextContent, TextResourceContents,
};
use session_event_model::{PromptContent, PromptEmbeddedSource};

/// Translate validated Session content only at the ACP client edge.
pub(crate) fn acp_blocks_from_prompt_content(contents: Vec<PromptContent>) -> Vec<ContentBlock> {
    contents
        .into_iter()
        .map(|content| match content {
            PromptContent::Text { text } => {
                ContentBlock::Text(TextContent::new(text.into_string()))
            }
            PromptContent::ResourceLink {
                uri,
                name,
                mime_type,
            } => ContentBlock::ResourceLink(
                ResourceLink::new(name.into_string(), uri.into_string())
                    .mime_type(mime_type.map(|value| value.into_string())),
            ),
            PromptContent::Image {
                mime_type,
                data,
                uri,
            } => ContentBlock::Image(
                ImageContent::new(data.into_string(), mime_type.into_string())
                    .uri(uri.map(|value| value.into_string())),
            ),
            PromptContent::Audio { mime_type, data } => ContentBlock::Audio(AudioContent::new(
                data.into_string(),
                mime_type.into_string(),
            )),
            PromptContent::EmbeddedResource {
                uri,
                mime_type,
                source,
            } => {
                let resource_uri = uri.into_string();
                let resource_mime_type = mime_type.map(|value| value.into_string());
                let resource = match source {
                    PromptEmbeddedSource::Text(text) => {
                        EmbeddedResourceResource::TextResourceContents(
                            TextResourceContents::new(text.into_string(), resource_uri)
                                .mime_type(resource_mime_type),
                        )
                    }
                    PromptEmbeddedSource::Blob(blob) => {
                        EmbeddedResourceResource::BlobResourceContents(
                            BlobResourceContents::new(blob.into_string(), resource_uri)
                                .mime_type(resource_mime_type),
                        )
                    }
                };
                ContentBlock::Resource(EmbeddedResource::new(resource))
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct ProviderPromptContent {
    blocks: Vec<ContentBlock>,
}

impl ProviderPromptContent {
    pub(crate) fn new(
        blocks: Vec<ContentBlock>,
        capabilities: &ProviderCapabilityReport,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        for block in &blocks {
            let unsupported = match block {
                ContentBlock::Text(_) | ContentBlock::ResourceLink(_) => None,
                ContentBlock::Image(_) if !capabilities.accepts_image => Some("image"),
                ContentBlock::Audio(_) if !capabilities.accepts_audio => Some("audio"),
                ContentBlock::Resource(_) if !capabilities.accepts_embedded_resource => {
                    Some("embeddedResource")
                }
                ContentBlock::Image(_) | ContentBlock::Audio(_) | ContentBlock::Resource(_) => None,
                _ => Some("unknown"),
            };
            if let Some(content_type) = unsupported {
                return Err(ExternalProviderRuntimeError::UnsupportedContent { content_type });
            }
        }
        Ok(Self { blocks })
    }

    pub(crate) fn into_blocks(self) -> Vec<ContentBlock> {
        self.blocks
    }

    #[cfg(feature = "test-observation")]
    pub fn new_for_test(
        blocks: Vec<ContentBlock>,
        capabilities: &ProviderCapabilityReport,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::new(blocks, capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical variants retain their exact ACP type and nested payload
    /// shape at the single SDK decoding edge.
    #[test]
    fn audio_and_embedded_resources_keep_payload_and_capability_gate() {
        let blocks = acp_blocks_from_prompt_content(vec![
            PromptContent::audio("audio/wav".to_owned(), "aGVsbG8=".to_owned()).expect("audio"),
            PromptContent::embedded_text(
                "file:///note.txt".to_owned(),
                Some("text/plain".to_owned()),
                "hello".to_owned(),
            )
            .expect("text resource"),
            PromptContent::embedded_blob(
                "file:///data.bin".to_owned(),
                Some("application/octet-stream".to_owned()),
                "aGVsbG8=".to_owned(),
            )
            .expect("blob resource"),
        ]);
        assert_eq!(
            serde_json::to_value(&blocks).expect("ACP blocks"),
            serde_json::json!([
                {"type":"audio","mimeType":"audio/wav","data":"aGVsbG8="},
                {"type":"resource","resource":{"uri":"file:///note.txt",
                    "mimeType":"text/plain","text":"hello"}},
                {"type":"resource","resource":{"uri":"file:///data.bin",
                    "mimeType":"application/octet-stream","blob":"aGVsbG8="}}
            ])
        );
        assert!(matches!(
            ProviderPromptContent::new(blocks.clone(), &ProviderCapabilityReport::default()),
            Err(ExternalProviderRuntimeError::UnsupportedContent {
                content_type: "audio"
            })
        ));
        let mut capabilities = ProviderCapabilityReport {
            accepts_audio: true,
            ..ProviderCapabilityReport::default()
        };
        assert!(matches!(
            ProviderPromptContent::new(blocks.clone(), &capabilities),
            Err(ExternalProviderRuntimeError::UnsupportedContent {
                content_type: "embeddedResource"
            })
        ));
        capabilities.accepts_embedded_resource = true;
        assert!(ProviderPromptContent::new(blocks, &capabilities).is_ok());
    }
}
