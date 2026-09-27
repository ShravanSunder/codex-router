//! Canonical response to a provider Session question.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(JsonSchema, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum QuestionResponse {
    Answered {
        content: BTreeMap<String, QuestionAnswerValue>,
    },
    Declined,
    Cancelled,
}

#[derive(JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QuestionAnswerValue {
    Text(String),
    Number(serde_json::Number),
    Boolean(bool),
}
