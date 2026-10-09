use crate::{ComponentFingerprint, ComponentKind, ListenerKind, SlotImage};
use codex_router_descriptor_boundary::MAX_RIGHTS;
use serde::{Deserialize, Serialize, ser::Error as _};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildLaunchContext {
    pub role: ComponentKind,
    pub image: SlotImage,
    pub fingerprint: ComponentFingerprint,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "ChildGrantFrameWire")]
pub enum ChildGrantFrame {
    Bootstrap {
        launch: ChildLaunchContext,
        listeners: Vec<ListenerKind>,
    },
    ListenerGrant {
        listeners: Vec<ListenerKind>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ChildGrantFrameError {
    #[error("child grant exceeds the descriptor rights limit")]
    TooManyRights,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum ChildGrantFrameWire {
    Bootstrap {
        launch: ChildLaunchContext,
        listeners: Vec<ListenerKind>,
    },
    ListenerGrant {
        listeners: Vec<ListenerKind>,
    },
}

impl ChildGrantFrame {
    pub fn bootstrap(
        launch: ChildLaunchContext,
        listeners: Vec<ListenerKind>,
    ) -> Result<Self, ChildGrantFrameError> {
        if listeners.len() > MAX_RIGHTS - 2 {
            return Err(ChildGrantFrameError::TooManyRights);
        }
        Ok(Self::Bootstrap { launch, listeners })
    }

    pub fn listener_grant(listeners: Vec<ListenerKind>) -> Result<Self, ChildGrantFrameError> {
        if listeners.len() > MAX_RIGHTS {
            return Err(ChildGrantFrameError::TooManyRights);
        }
        Ok(Self::ListenerGrant { listeners })
    }

    pub fn listener_kinds(&self) -> &[ListenerKind] {
        match self {
            Self::Bootstrap { listeners, .. } | Self::ListenerGrant { listeners } => listeners,
        }
    }

    pub(crate) fn is_bootstrap(&self) -> bool {
        matches!(self, Self::Bootstrap { .. })
    }

    pub(crate) fn validate(&self) -> Result<(), ChildGrantFrameError> {
        match self {
            Self::Bootstrap { listeners, .. } if listeners.len() > MAX_RIGHTS - 2 => {
                Err(ChildGrantFrameError::TooManyRights)
            }
            Self::ListenerGrant { listeners } if listeners.len() > MAX_RIGHTS => {
                Err(ChildGrantFrameError::TooManyRights)
            }
            _ => Ok(()),
        }
    }
}

impl TryFrom<ChildGrantFrameWire> for ChildGrantFrame {
    type Error = ChildGrantFrameError;

    fn try_from(value: ChildGrantFrameWire) -> Result<Self, Self::Error> {
        match value {
            ChildGrantFrameWire::Bootstrap { launch, listeners } => {
                Self::bootstrap(launch, listeners)
            }
            ChildGrantFrameWire::ListenerGrant { listeners } => Self::listener_grant(listeners),
        }
    }
}

impl From<&ChildGrantFrame> for ChildGrantFrameWire {
    fn from(value: &ChildGrantFrame) -> Self {
        match value {
            ChildGrantFrame::Bootstrap { launch, listeners } => Self::Bootstrap {
                launch: launch.clone(),
                listeners: listeners.clone(),
            },
            ChildGrantFrame::ListenerGrant { listeners } => Self::ListenerGrant {
                listeners: listeners.clone(),
            },
        }
    }
}

impl Serialize for ChildGrantFrame {
    fn serialize<TSerializer>(
        &self,
        serializer: TSerializer,
    ) -> Result<TSerializer::Ok, TSerializer::Error>
    where
        TSerializer: serde::Serializer,
    {
        self.validate().map_err(TSerializer::Error::custom)?;
        ChildGrantFrameWire::from(self).serialize(serializer)
    }
}
