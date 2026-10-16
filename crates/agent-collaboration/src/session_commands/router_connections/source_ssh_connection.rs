//! Validated locators for the fixed source-side Sessions entrypoint.
use super::RouterRegistryError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SshDestination(String);

impl SshDestination {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SshDestination {
    type Error = RouterRegistryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let (user, host) = value
            .split_once('@')
            .map_or((None, value.as_str()), |(user, host)| (Some(user), host));
        let valid_user = user.is_none_or(|user| {
            !user.is_empty()
                && user
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        });
        let valid_host = host.parse::<std::net::IpAddr>().is_ok()
            || (!host.is_empty()
                && !host.starts_with('-')
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)));
        if value.len() > 1024 || value.starts_with('-') || !valid_user || !valid_host {
            return Err(RouterRegistryError::InvalidSshDestination);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct AbsoluteSourcePath(String);

impl AbsoluteSourcePath {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AbsoluteSourcePath {
    type Error = RouterRegistryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !value.starts_with('/') || value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(RouterRegistryError::InvalidSshPath);
        }
        Ok(Self(value))
    }
}

impl From<AbsoluteSourcePath> for String {
    fn from(value: AbsoluteSourcePath) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SourceClientProfile {
    Router,
    RouterDebug,
}

impl SourceClientProfile {
    pub(crate) fn native_profile(self) -> codex_native_integration::SessionProfile {
        match self {
            Self::Router => codex_native_integration::SessionProfile::Router,
            Self::RouterDebug => codex_native_integration::SessionProfile::RouterDebug,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceSshConnection {
    pub(crate) destination: SshDestination,
    pub(crate) executable: AbsoluteSourcePath,
    pub(crate) service_directory: AbsoluteSourcePath,
    pub(crate) client_profile: SourceClientProfile,
}
