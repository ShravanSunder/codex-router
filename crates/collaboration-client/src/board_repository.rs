//! Local repository discovery before a board request is transmitted.
use crate::{ClientError, ControlClient};
use message_board::{
    CommonDirectory, NormalizedOrigin, RepositoryRef, ServiceId, normalize_git_origin_url,
};
use std::{path::Path, process::Command};

#[derive(Clone, Debug)]
pub enum BoardRepositoryLocation {
    Origin(NormalizedOrigin),
    Local(CommonDirectory),
}

#[derive(Debug, thiserror::Error)]
pub enum BoardRepositoryError {
    #[error("repository origin must identify a host and repository path")]
    InvalidOrigin,
    #[error("repository origin could not be canonicalized")]
    InvalidNormalizedOrigin,
    #[error("repository path must resolve to a readable local path")]
    UnreadablePath,
    #[error("discovered Git origin is invalid")]
    InvalidDiscoveredOrigin,
    #[error("repository path is not inside a Git repository with an origin or common directory")]
    MissingRepository,
    #[error("discovered Git common directory is not readable")]
    UnreadableCommonDirectory,
    #[error("discovered Git common directory is not UTF-8")]
    NonUtf8CommonDirectory,
    #[error("discovered Git common directory is invalid")]
    InvalidCommonDirectory,
}

impl BoardRepositoryLocation {
    pub fn from_origin(origin: &str) -> Result<Self, BoardRepositoryError> {
        let normalized =
            normalize_git_origin_url(origin).ok_or(BoardRepositoryError::InvalidOrigin)?;
        Ok(Self::Origin(normalized.try_into().map_err(|_| {
            BoardRepositoryError::InvalidNormalizedOrigin
        })?))
    }

    /// Reads local Git metadata; never probes a remote repository.
    pub fn discover(path: &Path) -> Result<Self, BoardRepositoryError> {
        let canonical =
            std::fs::canonicalize(path).map_err(|_| BoardRepositoryError::UnreadablePath)?;
        if let Some(origin) = git_stdout(&canonical, &["remote", "get-url", "origin"])
            && let Some(normalized) = normalize_git_origin_url(&origin)
        {
            return Ok(Self::Origin(
                normalized
                    .try_into()
                    .map_err(|_| BoardRepositoryError::InvalidDiscoveredOrigin)?,
            ));
        }
        let common = git_stdout(
            &canonical,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .ok_or(BoardRepositoryError::MissingRepository)?;
        let common = std::fs::canonicalize(common)
            .map_err(|_| BoardRepositoryError::UnreadableCommonDirectory)?;
        let common = common
            .to_str()
            .ok_or(BoardRepositoryError::NonUtf8CommonDirectory)?
            .to_owned();
        Ok(Self::Local(common.try_into().map_err(|_| {
            BoardRepositoryError::InvalidCommonDirectory
        })?))
    }

    /// Binds a local locator to the service verified by the connected client.
    pub fn for_client(self, client: &ControlClient) -> Result<RepositoryRef, ClientError> {
        match self {
            Self::Origin(normalized_origin) => Ok(RepositoryRef::Origin { normalized_origin }),
            Self::Local(common_directory) => {
                let service_id: String = client.identity().service_id.clone().into();
                let service_id = ServiceId::try_from(service_id)
                    .map_err(|_| ClientError::Protocol("invalid selected service identity"))?;
                Ok(RepositoryRef::Local {
                    service_id,
                    common_directory,
                })
            }
        }
    }
}

fn git_stdout(path: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}
