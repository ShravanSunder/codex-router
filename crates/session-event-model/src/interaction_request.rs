//! Complete pending interactions, suitable for replay to a late subscriber.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{ApprovalChoice, InteractionKind};

/// ACP option IDs are opaque. Validation never trims or rewrites them.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OfferedOptionId(String);

impl OfferedOptionId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidOfferedOptionId> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidOfferedOptionId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OfferedOptionId {
    type Error = InvalidOfferedOptionId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<OfferedOptionId> for String {
    fn from(value: OfferedOptionId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidOfferedOptionId;

impl std::fmt::Display for InvalidOfferedOptionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("approval option ID is empty")
    }
}

impl std::error::Error for InvalidOfferedOptionId {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferedOption {
    pub option_id: OfferedOptionId,
    pub label: String,
    pub choice: ApprovalChoice,
}

/// Nonempty, ordered options with one entry per agent option ID.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<OfferedOption>", into = "Vec<OfferedOption>")]
pub struct OfferedOptions {
    first: OfferedOption,
    remaining: Vec<OfferedOption>,
}

impl OfferedOptions {
    pub fn new(options: Vec<OfferedOption>) -> Result<Self, InvalidOfferedOptions> {
        let mut seen = BTreeSet::new();
        if options.is_empty()
            || options
                .iter()
                .any(|option| !seen.insert(option.option_id.clone()))
        {
            return Err(InvalidOfferedOptions);
        }
        let mut options = options.into_iter();
        let first = options.next().ok_or(InvalidOfferedOptions)?;
        Ok(Self {
            first,
            remaining: options.collect(),
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &OfferedOption> {
        std::iter::once(&self.first).chain(self.remaining.iter())
    }
}

impl TryFrom<Vec<OfferedOption>> for OfferedOptions {
    type Error = InvalidOfferedOptions;

    fn try_from(value: Vec<OfferedOption>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<OfferedOptions> for Vec<OfferedOption> {
    fn from(value: OfferedOptions) -> Self {
        std::iter::once(value.first)
            .chain(value.remaining)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidOfferedOptions;

impl std::fmt::Display for InvalidOfferedOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("approval options must be nonempty with unique option IDs")
    }
}

impl std::error::Error for InvalidOfferedOptions {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalSubject {
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: ApprovalToolCallSubject,
    },
    Command {
        command: String,
        cwd: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalToolCallSubject {
    pub tool_call_id: String,
    pub kind: String,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<ApprovalSubject>,
    pub options: OfferedOptions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum QuestionField {
    #[serde(rename_all = "camelCase")]
    Text {
        field_id: String,
        label: String,
        description: Option<String>,
        required: bool,
    },
    #[serde(rename_all = "camelCase")]
    Number {
        field_id: String,
        label: String,
        description: Option<String>,
        required: bool,
    },
    #[serde(rename_all = "camelCase")]
    Boolean {
        field_id: String,
        label: String,
        description: Option<String>,
        required: bool,
    },
    #[serde(rename_all = "camelCase")]
    SingleChoice {
        field_id: String,
        label: String,
        description: Option<String>,
        required: bool,
        options: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<QuestionField>", into = "Vec<QuestionField>")]
pub struct QuestionFields {
    first: QuestionField,
    remaining: Vec<QuestionField>,
}

impl QuestionFields {
    pub fn new(fields: Vec<QuestionField>) -> Result<Self, EmptyQuestionFields> {
        let mut fields = fields.into_iter();
        let first = fields.next().ok_or(EmptyQuestionFields)?;
        Ok(Self {
            first,
            remaining: fields.collect(),
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &QuestionField> {
        std::iter::once(&self.first).chain(self.remaining.iter())
    }
}

impl TryFrom<Vec<QuestionField>> for QuestionFields {
    type Error = EmptyQuestionFields;

    fn try_from(value: Vec<QuestionField>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<QuestionFields> for Vec<QuestionField> {
    fn from(value: QuestionFields) -> Self {
        std::iter::once(value.first)
            .chain(value.remaining)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyQuestionFields;

impl std::fmt::Display for EmptyQuestionFields {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a question requires at least one field")
    }
}

impl std::error::Error for EmptyQuestionFields {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionRequest {
    pub request_id: String,
    pub prompt: String,
    pub fields: QuestionFields,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PendingInteraction {
    Approval { request: ApprovalRequest },
    Question { request: QuestionRequest },
}

impl PendingInteraction {
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::Approval { request } => &request.request_id,
            Self::Question { request } => &request.request_id,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> InteractionKind {
        match self {
            Self::Approval { .. } => InteractionKind::Approval,
            Self::Question { .. } => InteractionKind::Question,
        }
    }
}
