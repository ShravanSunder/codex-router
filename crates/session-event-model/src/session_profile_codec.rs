//! The versioned layer-2 ACP session profile. Provider-specific wire methods
//! are translated before their events leave the back door.

use std::collections::BTreeSet;

use message_board::{EndpointId, Identity, SessionRef};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::{
    ApprovalChoice, ApprovalEffect, ApprovalScope, CapabilityReport, InteractionKind,
    ProviderAuthStatus, SessionItem, SessionItemKind,
};

pub const PROFILE_VERSION: u32 = 1;
pub const STEERING_METHOD: &str = "_session/steering";
pub const QUEUE_ADD_METHOD: &str = "_session/queue/add";
pub const QUEUE_LIST_METHOD: &str = "_session/queue/list";
pub const QUEUE_CANCEL_METHOD: &str = "_session/queue/cancel";
pub const STATE_METHOD: &str = "_session/state";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProfileElement {
    Steer,
    Queue,
    State,
    ApprovalChoice,
    Identity,
    Capabilities,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileAdvertisement {
    pub version: u32,
    pub elements: BTreeSet<ProfileElement>,
}

impl ProfileAdvertisement {
    #[must_use]
    pub fn new(elements: impl IntoIterator<Item = ProfileElement>) -> Self {
        Self {
            version: PROFILE_VERSION,
            elements: elements.into_iter().collect(),
        }
    }

    pub fn supports(&self, element: ProfileElement) -> Result<(), ProfileError> {
        if self.version != PROFILE_VERSION {
            return Err(ProfileError::UnsupportedVersion(self.version));
        }
        if !self.elements.contains(&element) {
            return Err(ProfileError::Unsupported(element));
        }
        Ok(())
    }
}

/// `initialize._meta` advertises the versioned profile and the de facto
/// steering flag understood by both surveyed ACP adapters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeProfileMetadata {
    pub session_profile: ProfileAdvertisement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering: Option<SteeringSupport>,
}

impl InitializeProfileMetadata {
    #[must_use]
    pub fn new(session_profile: ProfileAdvertisement) -> Self {
        let steering = session_profile
            .elements
            .contains(&ProfileElement::Steer)
            .then_some(SteeringSupport { supported: true });
        Self {
            session_profile,
            steering,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SteeringSupport {
    pub supported: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    Unsupported(ProfileElement),
    UnsupportedVersion(u32),
    InvalidChoice,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(element) => {
                write!(formatter, "unsupported profile element: {element:?}")
            }
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported profile version: {version}")
            }
            Self::InvalidChoice => formatter.write_str("invalid approval choice metadata"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// ACP v1 content types, represented without an ACP SDK dependency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PromptContent {
    Text { text: String },
    ResourceLink { uri: String, name: String },
    Image { data: String, mime_type: String },
    Audio { data: String, mime_type: String },
    EmbeddedResource { uri: String, text: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SteeringRequest {
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SteeringMetadata>,
}

impl SteeringRequest {
    #[must_use]
    pub const fn method(&self) -> &'static str {
        STEERING_METHOD
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SteeringMetadata {
    pub steering: SteeringOptions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SteeringOptions {
    pub idle_behavior: IdleSteeringBehavior,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdleSteeringBehavior {
    PromptRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SteeringOutcome {
    pub outcome: SteeringResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SteeringResult {
    Injected,
    StartedNewTurn,
    PromptRequired,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueAddRequest {
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
}

impl QueueAddRequest {
    #[must_use]
    pub const fn method(&self) -> &'static str {
        QUEUE_ADD_METHOD
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueAddResult {
    pub input_id: String,
    pub position: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueListRequest {
    pub session_id: String,
}

impl QueueListRequest {
    #[must_use]
    pub const fn method(&self) -> &'static str {
        QUEUE_LIST_METHOD
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueListResult {
    pub items: Vec<QueuedInput>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueuedInput {
    pub input_id: String,
    pub position: u64,
    pub preview: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueCancelRequest {
    pub session_id: String,
    pub input_id: String,
}

impl QueueCancelRequest {
    #[must_use]
    pub const fn method(&self) -> &'static str {
        QUEUE_CANCEL_METHOD
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueCancelResult {
    pub cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileState {
    Running,
    Idle,
    RequiresAction {
        kind: InteractionKind,
    },
    Lost {
        reason: String,
        stop_reason: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateNotification {
    pub session_id: String,
    pub state: ProfileState,
}

impl StateNotification {
    #[must_use]
    pub const fn method(&self) -> &'static str {
        STATE_METHOD
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateNotificationWire {
    session_id: String,
    state: StateName,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_action: Option<InteractionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StateName {
    Running,
    Idle,
    RequiresAction,
    Lost,
}

impl Serialize for StateNotification {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (state, requires_action, stop_reason, reason) = match &self.state {
            ProfileState::Running => (StateName::Running, None, None, None),
            ProfileState::Idle => (StateName::Idle, None, None, None),
            ProfileState::RequiresAction { kind } => {
                (StateName::RequiresAction, Some(*kind), None, None)
            }
            ProfileState::Lost {
                reason,
                stop_reason,
            } => (
                StateName::Lost,
                None,
                stop_reason.clone(),
                Some(reason.clone()),
            ),
        };
        StateNotificationWire {
            session_id: self.session_id.clone(),
            state,
            requires_action,
            stop_reason,
            reason,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StateNotification {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = StateNotificationWire::deserialize(deserializer)?;
        let state = match (
            wire.state,
            wire.requires_action,
            wire.stop_reason,
            wire.reason,
        ) {
            (StateName::Running, None, None, None) => ProfileState::Running,
            (StateName::Idle, None, None, None) => ProfileState::Idle,
            (StateName::RequiresAction, Some(kind), None, None) => {
                ProfileState::RequiresAction { kind }
            }
            (StateName::Lost, None, stop_reason, Some(reason)) => ProfileState::Lost {
                reason,
                stop_reason,
            },
            _ => return Err(serde::de::Error::custom("invalid session state fields")),
        };
        Ok(Self {
            session_id: wire.session_id,
            state,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalPromptMetadata {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalSubject {
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: ToolCallSubject,
    },
    Command {
        command: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_id: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallSubject {
    pub tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// ACP tool-call updates can add fields beyond this profile's identity.
    #[serde(flatten)]
    pub additional_fields: std::collections::BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouterIdentityMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<SessionRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approver: Option<Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<EndpointId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<Identity>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChoiceMetadataWire {
    effect: ApprovalEffect,
    scope: ChoiceScopeName,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_stored: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ChoiceScopeName {
    Once,
    Session,
    Persistent,
}

#[must_use]
pub fn encode_choice_metadata(choice: &ApprovalChoice) -> Value {
    let effect = match choice.effect {
        ApprovalEffect::Allow => "allow",
        ApprovalEffect::Decline => "decline",
        ApprovalEffect::Abort => "abort",
    };
    let (scope, where_stored) = match &choice.scope {
        ApprovalScope::Once => ("once", None),
        ApprovalScope::Session => ("session", None),
        ApprovalScope::Persistent { where_stored } => ("persistent", Some(where_stored.as_str())),
    };
    let mut fields = serde_json::Map::new();
    fields.insert("effect".to_owned(), Value::String(effect.to_owned()));
    fields.insert("scope".to_owned(), Value::String(scope.to_owned()));
    if let Some(where_stored) = where_stored {
        fields.insert("where".to_owned(), Value::String(where_stored.to_owned()));
    }
    Value::Object(fields)
}

pub fn decode_choice_metadata(value: Value) -> Result<ApprovalChoice, ProfileError> {
    let wire: ChoiceMetadataWire =
        serde_json::from_value(value).map_err(|_| ProfileError::InvalidChoice)?;
    let scope = match (wire.scope, wire.where_stored) {
        (ChoiceScopeName::Once, None) => ApprovalScope::Once,
        (ChoiceScopeName::Session, None) => ApprovalScope::Session,
        (ChoiceScopeName::Persistent, Some(where_stored)) => {
            ApprovalScope::persistent(where_stored).map_err(|_| ProfileError::InvalidChoice)?
        }
        _ => return Err(ProfileError::InvalidChoice),
    };
    Ok(ApprovalChoice::new(wire.effect, scope))
}

/// Session capabilities are placed at `_meta.sessionProfile.capabilities`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileSessionMetadata {
    pub capabilities: CapabilityReport,
}

/// `_meta` on session/new, load, and resume responses.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionResponseProfileMetadata {
    pub session_profile: ProfileSessionMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub router: Option<RouterIdentityMetadata>,
}

/// Decode the connection-scoped Claude notification at the back door. Extra
/// account details are intentionally ignored and never enter the model.
pub fn decode_connection_auth_status(
    value: Value,
) -> Result<ProviderAuthStatus, serde_json::Error> {
    let notification: AuthStatusNotification = serde_json::from_value(value)?;
    let status = match notification.auth_status.kind {
        AuthStatusKind::None => ProviderAuthStatus::LoggedOut,
        AuthStatusKind::Account => ProviderAuthStatus::Account {
            label: notification.auth_status.label,
        },
        AuthStatusKind::ApiKey => ProviderAuthStatus::ApiKey {
            label: notification.auth_status.label,
        },
        AuthStatusKind::Gateway => ProviderAuthStatus::Gateway {
            label: notification.auth_status.label,
        },
        AuthStatusKind::External => ProviderAuthStatus::External {
            label: notification.auth_status.label,
        },
    };
    Ok(status)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatusNotification {
    auth_status: AuthStatusFields,
}

#[derive(Deserialize)]
struct AuthStatusFields {
    kind: AuthStatusKind,
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthStatusKind {
    None,
    Account,
    ApiKey,
    Gateway,
    External,
}

/// Parsed provider edge observations. The ACP runtime owns method parsing;
/// this type owns their provider-neutral meaning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderEdge {
    CursorPlan {
        item_id: String,
        text: String,
    },
    CursorTodos {
        item_id: String,
        items: Vec<CursorTodo>,
        merge: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorTodo {
    pub id: String,
    pub content: String,
    pub status: CursorTodoStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorTodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanChangeMode {
    Replace,
    Merge,
}

/// The event-model Item is provider neutral; the entry changes remain at the
/// back door until the runtime can apply merge semantics to its current plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranslatedPlanChange {
    pub item: SessionItem,
    pub entries: Vec<CursorTodo>,
    pub mode: PlanChangeMode,
}

#[must_use]
pub fn translate_provider_edge(edge: ProviderEdge) -> TranslatedPlanChange {
    match edge {
        ProviderEdge::CursorPlan { item_id, text } => TranslatedPlanChange {
            item: SessionItem {
                item_id,
                kind: SessionItemKind::Plan,
                text: Some(text),
            },
            entries: Vec::new(),
            mode: PlanChangeMode::Replace,
        },
        ProviderEdge::CursorTodos {
            item_id,
            items,
            merge,
        } => TranslatedPlanChange {
            item: SessionItem {
                item_id,
                kind: SessionItemKind::Plan,
                text: Some(
                    items
                        .iter()
                        .map(|item| item.content.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            },
            entries: items,
            mode: if merge {
                PlanChangeMode::Merge
            } else {
                PlanChangeMode::Replace
            },
        },
    }
}
