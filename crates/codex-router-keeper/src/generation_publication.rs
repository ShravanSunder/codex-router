//! Atomic E1 filesystem publication. Readiness, services and retirement remain caller policy.
use crate::{
    PublicationError, SingletonAuthority, listener_address::validate_private_path,
    publication_node::PublicationNode,
};
use codex_router_descriptor_boundary::DescriptorGate;
use codex_router_keeper_protocol::{DefaultEndpointPath, GenerationAliasPath, GenerationId};
use std::{
    ffi::OsString,
    fs,
    os::{fd::OwnedFd, unix::fs::FileTypeExt},
    path::{Path, PathBuf},
};

pub struct GenerationEndpointPublisher {
    current: Option<PublicationNode>,
    endpoint: DefaultEndpointPath,
    // Keep the same locked open file description until owned publication cleanup finishes.
    _singleton: OwnedFd,
}
fn require_absent(path: &Path) -> Result<(), PublicationError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(PublicationError::EndpointOccupied),
        Err(error) => Err(error.into()),
    }
}
fn validate_candidate(
    endpoint: &DefaultEndpointPath,
    generation: &GenerationId,
    alias: &GenerationAliasPath,
) -> Result<PathBuf, PublicationError> {
    alias.validate_for(endpoint, generation)?;
    if !fs::symlink_metadata(alias.as_path())?
        .file_type()
        .is_symlink()
        || !fs::metadata(alias.as_path())?.file_type().is_socket()
    {
        return Err(PublicationError::AliasNotSocket);
    }
    alias
        .as_path()
        .file_name()
        .map(PathBuf::from)
        .ok_or(PublicationError::OwnershipLost)
}
impl GenerationEndpointPublisher {
    pub async fn new(
        endpoint: DefaultEndpointPath,
        authority: &SingletonAuthority,
    ) -> Result<Self, PublicationError> {
        validate_private_path(endpoint.as_path())?;
        require_absent(endpoint.as_path())?;
        let singleton = DescriptorGate::global()
            .duplicate(authority.as_fd())
            .await?;
        Ok(Self {
            current: None,
            endpoint,
            _singleton: singleton,
        })
    }
    /// Only validated handoff authority may choose adoption; this establishes no process health.
    pub async fn adopt(
        endpoint: DefaultEndpointPath,
        authority: &SingletonAuthority,
        generation: &GenerationId,
        alias: &GenerationAliasPath,
    ) -> Result<Self, PublicationError> {
        validate_private_path(endpoint.as_path())?;
        let target = validate_candidate(&endpoint, generation, alias)?;
        let singleton = DescriptorGate::global()
            .duplicate(authority.as_fd())
            .await?;
        let current = PublicationNode::capture(endpoint.as_path().to_owned(), target)?;
        Ok(Self {
            current: Some(current),
            endpoint,
            _singleton: singleton,
        })
    }
    /// There is no suspension between the rename commit point and its infallible ownership update.
    pub fn publish(
        &mut self,
        generation: &GenerationId,
        alias: &GenerationAliasPath,
    ) -> Result<(), PublicationError> {
        self.publish_with_rename(generation, alias, |source, target| {
            fs::rename(source, target)
        })
    }
    fn validate_endpoint_ownership(&self) -> Result<(), PublicationError> {
        match &self.current {
            Some(current) if current.matches_current() => Ok(()),
            Some(_) => Err(PublicationError::OwnershipLost),
            None => require_absent(self.endpoint.as_path()),
        }
    }
    fn temporary_path(&self) -> Result<PathBuf, PublicationError> {
        let name = self
            .endpoint
            .as_path()
            .file_name()
            .ok_or(PublicationError::OwnershipLost)?;
        let mut temporary = OsString::from(".");
        temporary.push(name);
        temporary.push(".publication");
        Ok(self.endpoint.as_path().with_file_name(temporary))
    }
    // The plan's narrow filesystem seam enables real-node ordinary rename failure proof.
    fn publish_with_rename(
        &mut self,
        generation: &GenerationId,
        alias: &GenerationAliasPath,
        rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<(), PublicationError> {
        validate_private_path(self.endpoint.as_path())?;
        let target = validate_candidate(&self.endpoint, generation, alias)?;
        self.validate_endpoint_ownership()?;
        let temporary_path = self.temporary_path()?;
        std::os::unix::fs::symlink(&target, &temporary_path)?;
        let mut temporary = PublicationNode::capture(temporary_path, target)?;
        self.validate_endpoint_ownership()?;
        // All metadata/target ownership was captured before rename. No fallible work follows commit.
        rename(&temporary.path, self.endpoint.as_path())?;
        temporary.path = self.endpoint.as_path().to_owned();
        self.current = Some(temporary);
        Ok(())
    }
}
#[cfg(test)]
#[path = "generation_publication_tests.rs"]
mod generation_publication_tests;
