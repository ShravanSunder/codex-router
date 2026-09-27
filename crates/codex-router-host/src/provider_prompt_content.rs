//! Validate ACP prompt blocks against a Session's advertised content types.

use crate::external_provider_runtime::ExternalProviderRuntimeError;
use crate::provider_capability_report::ProviderCapabilityReport;
use agent_client_protocol::schema::v1::ContentBlock;

#[derive(Clone, Debug)]
pub(crate) struct ProviderPromptContent {
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
}
