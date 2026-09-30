//! Typed per-root range locators for subscription notices and their settlement.
use super::{
    BatchId, InvalidSubscriptionField, SubscriptionGeneration, SubscriptionScope,
    SubscriptionWindowId, invalid_subscription_field,
};
use crate::{ActivitySequence, Identity, MessageId, TopicId};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Activity locator and summary for the pending messages in one subscribed root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingRootNotice {
    /// The Thread whose pending messages this notice locates.
    pub root_id: MessageId,
    /// The Topic containing the Thread.
    pub topic_id: TopicId,
    /// The first pending message sequence, greater than the Reader's Delivered cursor.
    pub from_sequence: ActivitySequence,
    /// The latest pending message sequence included by this notice.
    pub through_sequence: ActivitySequence,
    /// Number of pending messages between the two sequence bounds.
    pub message_count: u64,
    /// Distinct pending-message authors, ordered by their first appearance in the range.
    pub authors: Vec<Identity>,
    /// Raw first line of the root message, trimmed and capped at 80 characters.
    pub root_title_excerpt: String,
}

impl PendingRootNotice {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root_id: MessageId,
        topic_id: TopicId,
        from_sequence: ActivitySequence,
        through_sequence: ActivitySequence,
        message_count: u64,
        authors: Vec<Identity>,
        root_title_excerpt: String,
    ) -> Result<Self, InvalidSubscriptionField> {
        if from_sequence > through_sequence {
            return Err(invalid_subscription_field(
                "fromSequence",
                "must not exceed throughSequence",
            ));
        }
        if message_count == 0 {
            return Err(invalid_subscription_field(
                "messageCount",
                "must be greater than zero",
            ));
        }
        if authors.is_empty() {
            return Err(invalid_subscription_field(
                "authors",
                "must include at least one pending-message author",
            ));
        }
        if authors.iter().enumerate().any(|(index, author)| {
            authors
                .get(..index)
                .is_some_and(|earlier_authors| earlier_authors.contains(author))
        }) {
            return Err(invalid_subscription_field(
                "authors",
                "must contain each identity once",
            ));
        }
        if root_title_excerpt.trim() != root_title_excerpt
            || root_title_excerpt.chars().count() > 80
        {
            return Err(invalid_subscription_field(
                "rootTitleExcerpt",
                "must be trimmed and at most 80 characters",
            ));
        }
        Ok(Self {
            root_id,
            topic_id,
            from_sequence,
            through_sequence,
            message_count,
            authors,
            root_title_excerpt,
        })
    }
}

/// Selection state shared by the per-Reader delivery owner and its storage settlement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionBatch {
    /// Identifier for this selected notice operation.
    pub batch_id: BatchId,
    /// Whether the notice has been held while its target was not running.
    pub held: bool,
    /// First time the notice entered the held state.
    pub held_since: Option<DateTime<Utc>>,
    /// Whether any selected root is draining after resolution.
    pub draining: bool,
    /// Per-root range locators; no message bodies are included.
    pub roots: Vec<PendingRootNotice>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionBatchRootSettlement {
    pub root_message_id: MessageId,
    pub window_id: SubscriptionWindowId,
    pub delivered_through: ActivitySequence,
    pub subscription_scope: SubscriptionScope,
    pub subscription_generation: SubscriptionGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionBatchSettlement {
    pub roots: Vec<SubscriptionBatchRootSettlement>,
}
