//! Typed question listing and single-use responses for Router front doors.

use std::collections::BTreeMap;

use message_board::{Identity, SessionRef};
use serde::{Deserialize, Serialize};

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionListParams {
    pub pending: bool,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum QuestionFieldView {
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

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionState {
    Pending,
    Answered,
    Declined,
    Cancelled,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionRecord {
    pub request_id: String,
    pub requester: SessionRef,
    pub approver: Identity,
    pub prompt: String,
    pub fields: Vec<QuestionFieldView>,
    pub state: QuestionState,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionListResult {
    pub questions: Vec<QuestionRecord>,
}

#[derive(schemars::JsonSchema, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum QuestionResponse {
    Answered {
        content: BTreeMap<String, QuestionAnswerValue>,
    },
    Declined,
    Cancelled,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QuestionAnswerValue {
    Text(String),
    Number(serde_json::Number),
    Boolean(bool),
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionAnswerParams {
    pub request_id: String,
    #[serde(deserialize_with = "crate::interaction_actor::deserialize_interaction_actor")]
    pub actor: Identity,
    pub response: QuestionResponse,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionAnswerResult {
    pub request_id: String,
    pub state: QuestionState,
}
