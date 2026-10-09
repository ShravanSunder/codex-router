//! Pure effective endpoint/alias validation; filesystem authority belongs to the publisher.
use crate::GenerationId;
use serde::{Deserialize, Serialize};
use std::{
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};
#[derive(Debug, thiserror::Error)]
pub enum EndpointPathError {
    #[error("endpoint path must be an absolute file path")]
    InvalidEndpointPath,
    #[error("generation alias must use gen-<epoch8>-<positive-number>.sock")]
    InvalidAliasName,
    #[error("generation alias does not match its endpoint sibling and generation")]
    AliasRelationship,
}
fn validate_file_path(path: &Path) -> Result<(), EndpointPathError> {
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute()
        || path.file_name().is_none()
        || bytes.contains(&0)
        || bytes.last() == Some(&b'/')
        || bytes.ends_with(b"/.")
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(EndpointPathError::InvalidEndpointPath);
    }
    Ok(())
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct DefaultEndpointPath(PathBuf);
impl DefaultEndpointPath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}
impl TryFrom<PathBuf> for DefaultEndpointPath {
    type Error = EndpointPathError;
    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        validate_file_path(&value)?;
        Ok(Self(value))
    }
}
impl From<DefaultEndpointPath> for PathBuf {
    fn from(value: DefaultEndpointPath) -> Self {
        value.0
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct GenerationAliasPath(PathBuf);
fn alias_name(generation: &GenerationId) -> String {
    // The prefix is a pathname convention, never a reconstruction of the full epoch identity.
    let epoch8: String = generation
        .epoch
        .as_uuid()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("gen-{epoch8}-{}.sock", generation.number.get())
}
impl GenerationAliasPath {
    pub fn for_generation(
        endpoint: &DefaultEndpointPath,
        generation: &GenerationId,
    ) -> Result<Self, EndpointPathError> {
        let alias = Self::try_from(endpoint.as_path().with_file_name(alias_name(generation)))?;
        alias.validate_for(endpoint, generation)?;
        Ok(alias)
    }
    pub fn as_path(&self) -> &Path {
        &self.0
    }
    pub fn validate_for(
        &self,
        endpoint: &DefaultEndpointPath,
        generation: &GenerationId,
    ) -> Result<(), EndpointPathError> {
        if self.0 != endpoint.as_path().with_file_name(alias_name(generation))
            || self.0 == endpoint.as_path()
        {
            return Err(EndpointPathError::AliasRelationship);
        }
        Ok(())
    }
    pub(crate) fn validate_for_generation(
        &self,
        generation: &GenerationId,
    ) -> Result<(), EndpointPathError> {
        let expected_name = alias_name(generation);
        if self.0.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
            return Err(EndpointPathError::AliasRelationship);
        }
        Ok(())
    }
}
impl TryFrom<PathBuf> for GenerationAliasPath {
    type Error = EndpointPathError;
    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        validate_file_path(&value)?;
        let name = value
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(EndpointPathError::InvalidAliasName)?;
        let name = name
            .strip_prefix("gen-")
            .and_then(|name| name.strip_suffix(".sock"))
            .ok_or(EndpointPathError::InvalidAliasName)?;
        let (epoch8, number) = name
            .split_once('-')
            .ok_or(EndpointPathError::InvalidAliasName)?;
        let parsed = number
            .parse::<u64>()
            .map_err(|_| EndpointPathError::InvalidAliasName)?;
        if epoch8.len() != 8
            || !epoch8
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || parsed == 0
            || parsed.to_string() != number
        {
            return Err(EndpointPathError::InvalidAliasName);
        }
        Ok(Self(value))
    }
}
impl From<GenerationAliasPath> for PathBuf {
    fn from(value: GenerationAliasPath) -> Self {
        value.0
    }
}
