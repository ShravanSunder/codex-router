//! Reusable instruction text and its current materialized revision.
use crate::{InstructionId, RevisionId};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
#[error("instruction text must be nonempty, NUL-free and at most 1048576 UTF-8 bytes")]
pub struct InstructionTextError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InstructionText(String);
impl TryFrom<String> for InstructionText {
    type Error = InstructionTextError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.contains('\0') || value.len() > 1_048_576 {
            return Err(InstructionTextError);
        }
        Ok(Self(value))
    }
}
impl From<InstructionText> for String {
    fn from(value: InstructionText) -> Self {
        value.0
    }
}
impl InstructionText {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstructionDocument {
    pub instruction_id: InstructionId,
    pub revision_id: RevisionId,
    pub text: InstructionText,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
