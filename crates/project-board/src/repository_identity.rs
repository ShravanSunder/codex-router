use crate::ServiceId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("invalid repository {field}: {requirement}")]
pub struct RepositoryValidationError {
    pub field: &'static str,
    pub requirement: &'static str,
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct NormalizedOrigin(String);
impl TryFrom<String> for NormalizedOrigin {
    type Error = RepositoryValidationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let normalized = normalize_git_origin_url(&value).ok_or(RepositoryValidationError {
            field: "normalizedOrigin",
            requirement: "must identify a host and repository path",
        })?;
        if normalized != value {
            return Err(RepositoryValidationError {
                field: "normalizedOrigin",
                requirement: "must already be canonical",
            });
        }
        Ok(Self(value))
    }
}
impl From<NormalizedOrigin> for String {
    fn from(value: NormalizedOrigin) -> Self {
        value.0
    }
}
impl NormalizedOrigin {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct CommonDirectory(String);
impl TryFrom<String> for CommonDirectory {
    type Error = RepositoryValidationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.contains('\0') || !Path::new(&value).is_absolute() {
            return Err(RepositoryValidationError {
                field: "commonDirectory",
                requirement: "must be an absolute path without NUL",
            });
        }
        Ok(Self(value))
    }
}
impl From<CommonDirectory> for String {
    fn from(value: CommonDirectory) -> Self {
        value.0
    }
}
impl CommonDirectory {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RepositoryRef {
    #[serde(rename_all = "camelCase")]
    Origin { normalized_origin: NormalizedOrigin },
    #[serde(rename_all = "camelCase")]
    Local {
        service_id: ServiceId,
        common_directory: CommonDirectory,
    },
}

#[must_use]
pub fn normalize_git_origin_url(origin: &str) -> Option<String> {
    let origin = origin
        .trim()
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if origin.is_empty() {
        return None;
    }
    let (host, repository_path) = if let Some((_scheme, remainder)) = origin.split_once("://") {
        let remainder = remainder
            .rsplit_once('@')
            .map_or(remainder, |(_, value)| value);
        remainder.split_once('/')?
    } else if let Some((host_with_user, repository_path)) = origin.split_once(':') {
        let host = host_with_user
            .rsplit_once('@')
            .map_or(host_with_user, |(_, value)| value);
        (host, repository_path)
    } else {
        let (host, repository_path) = origin.split_once('/')?;
        if !host.contains('.') {
            return None;
        }
        (host, repository_path)
    };
    let host = host.trim().to_lowercase();
    let repository_path = repository_path.trim_matches('/');
    let repository_path = repository_path
        .strip_suffix(".git")
        .unwrap_or(repository_path);
    (!host.is_empty() && !repository_path.is_empty()).then(|| format!("{host}/{repository_path}"))
}
