//! Validated, provider-neutral content blocks for provider prompts and steers.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPromptContent {
    field: &'static str,
}

impl std::fmt::Display for InvalidPromptContent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid prompt {}", self.field)
    }
}

impl std::error::Error for InvalidPromptContent {}

macro_rules! prompt_string {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: String) -> Result<Self, InvalidPromptContent> {
                if value.is_empty() || value.contains('\0') {
                    Err(InvalidPromptContent { field: $field })
                } else {
                    Ok(Self(value))
                }
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }
    };
}

prompt_string!(PromptText, "text");
prompt_string!(PromptUri, "uri");
prompt_string!(PromptName, "name");
prompt_string!(PromptMimeType, "mime type");
prompt_string!(PromptData, "data");

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptEmbeddedSource {
    Text(PromptText),
    Blob(PromptData),
}

/// Images always carry base64 data; an optional URI preserves ACP provenance.
/// Embedded resources carry exactly one text or blob source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptContent {
    Text {
        text: PromptText,
    },
    ResourceLink {
        uri: PromptUri,
        name: PromptName,
        mime_type: Option<PromptMimeType>,
    },
    Image {
        mime_type: PromptMimeType,
        data: PromptData,
        uri: Option<PromptUri>,
    },
    Audio {
        mime_type: PromptMimeType,
        data: PromptData,
    },
    EmbeddedResource {
        uri: PromptUri,
        mime_type: Option<PromptMimeType>,
        source: PromptEmbeddedSource,
    },
}

impl PromptContent {
    pub fn text(text: String) -> Result<Self, InvalidPromptContent> {
        Ok(Self::Text {
            text: PromptText::try_new(text)?,
        })
    }

    pub fn resource_link(
        uri: String,
        name: String,
        mime_type: Option<String>,
    ) -> Result<Self, InvalidPromptContent> {
        Ok(Self::ResourceLink {
            uri: PromptUri::try_new(uri)?,
            name: PromptName::try_new(name)?,
            mime_type: mime_type.map(PromptMimeType::try_new).transpose()?,
        })
    }

    pub fn image(
        mime_type: String,
        data: String,
        uri: Option<String>,
    ) -> Result<Self, InvalidPromptContent> {
        Ok(Self::Image {
            mime_type: PromptMimeType::try_new(mime_type)?,
            data: PromptData::try_new(data)?,
            uri: uri.map(PromptUri::try_new).transpose()?,
        })
    }

    pub fn audio(mime_type: String, data: String) -> Result<Self, InvalidPromptContent> {
        Ok(Self::Audio {
            mime_type: PromptMimeType::try_new(mime_type)?,
            data: PromptData::try_new(data)?,
        })
    }

    pub fn embedded_text(
        uri: String,
        mime_type: Option<String>,
        text: String,
    ) -> Result<Self, InvalidPromptContent> {
        Ok(Self::EmbeddedResource {
            uri: PromptUri::try_new(uri)?,
            mime_type: mime_type.map(PromptMimeType::try_new).transpose()?,
            source: PromptEmbeddedSource::Text(PromptText::try_new(text)?),
        })
    }

    pub fn embedded_blob(
        uri: String,
        mime_type: Option<String>,
        blob: String,
    ) -> Result<Self, InvalidPromptContent> {
        Ok(Self::EmbeddedResource {
            uri: PromptUri::try_new(uri)?,
            mime_type: mime_type.map(PromptMimeType::try_new).transpose()?,
            source: PromptEmbeddedSource::Blob(PromptData::try_new(blob)?),
        })
    }
}
