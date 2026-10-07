use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

/// Validated Remote Control server display name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RemoteControlServerName(String);

const MAX_REMOTE_CONTROL_SERVER_NAME_BYTES: usize = 4096;

impl TryFrom<String> for RemoteControlServerName {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty()
            || value.len() > MAX_REMOTE_CONTROL_SERVER_NAME_BYTES
            || value.contains('\0')
        {
            return Err("invalid Remote Control server name");
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for RemoteControlServerName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

impl RemoteControlServerName {
    /// Returns the validated display name without changing its source characters.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
