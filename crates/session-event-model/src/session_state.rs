use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InteractionKind {
    Approval,
    Question,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingInteraction {
    pub request_id: String,
    pub kind: InteractionKind,
}

/// A nonempty set is required before a Session can report requiresAction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<PendingInteraction>", into = "Vec<PendingInteraction>")]
pub struct PendingInteractions {
    first: PendingInteraction,
    remaining: Vec<PendingInteraction>,
}

impl PendingInteractions {
    #[must_use]
    pub fn new(interactions: Vec<PendingInteraction>) -> Option<Self> {
        let mut interactions = interactions.into_iter();
        let first = interactions.next()?;
        Some(Self {
            first,
            remaining: interactions.collect(),
        })
    }

    #[must_use]
    pub fn first(&self) -> &PendingInteraction {
        &self.first
    }

    pub fn iter(&self) -> impl Iterator<Item = &PendingInteraction> {
        std::iter::once(&self.first).chain(self.remaining.iter())
    }
}

impl TryFrom<Vec<PendingInteraction>> for PendingInteractions {
    type Error = EmptyPendingInteractions;

    fn try_from(value: Vec<PendingInteraction>) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(EmptyPendingInteractions)
    }
}

impl From<PendingInteractions> for Vec<PendingInteraction> {
    fn from(value: PendingInteractions) -> Self {
        std::iter::once(value.first)
            .chain(value.remaining)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyPendingInteractions;

impl std::fmt::Display for EmptyPendingInteractions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("requiresAction needs a pending interaction")
    }
}

impl std::error::Error for EmptyPendingInteractions {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SessionState {
    Unloaded,
    Idle,
    Running,
    RequiresAction { pending: PendingInteractions },
    AuthenticationRequired,
    Closed,
}

impl SessionState {
    #[must_use]
    pub fn requires_action_kind(&self) -> Option<InteractionKind> {
        match self {
            Self::RequiresAction { pending } => Some(pending.first().kind),
            _ => None,
        }
    }
}
