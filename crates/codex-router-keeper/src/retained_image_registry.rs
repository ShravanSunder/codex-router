use crate::{
    GroupLaunchOutcome, ImageError, OwnedProcessGroup, OwnedSpawnCleanup,
    image_directory::{OwnedImageNode, ensure_private, private_directory, validate_root},
    image_identity::{capture, digest_hex, same_node, verify},
    image_warmup::{WarmupCleanup, WarmupOutcome, warmup_with_checkpoint},
    lifecycle_bounds::PREPARE_DEADLINE,
    owned_process_group::PostSpawnCheckpoint,
};
use codex_router_descriptor_boundary::DescriptorGate;
use codex_router_keeper_protocol::{BuildInfo, ComponentFingerprint, ComponentKind, SlotImage};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::process::Command;
#[derive(Clone, Debug)]
pub struct ImageLease(Arc<LeasedImage>);
#[derive(Debug)]
struct LeasedImage {
    record: SlotImage,
    build_info: BuildInfo,
}
impl ImageLease {
    pub fn image(&self) -> &SlotImage {
        &self.0.record
    }
    pub fn build_info(&self) -> &BuildInfo {
        &self.0.build_info
    }
    pub fn fingerprint(&self, kind: ComponentKind) -> ComponentFingerprint {
        let fingerprints = &self.0.build_info.fingerprints;
        match kind {
            ComponentKind::Keeper => fingerprints.keeper,
            ComponentKind::AgentCollaborationServices => fingerprints.agent_collaboration_services,
            ComponentKind::AgentProxyServices => fingerprints.agent_proxy_services,
            ComponentKind::AgentProviderServices => fingerprints.agent_provider_services,
        }
    }
}
struct CachedImage {
    record: SlotImage,
    build_info: BuildInfo,
    live: Weak<LeasedImage>,
}
pub struct ImageRegistry {
    images_root: PathBuf,
    entries: BTreeMap<[u8; 32], CachedImage>,
    pending_warmups: Vec<PendingWarmup>,
    #[cfg(test)]
    last_warmup_observation: Option<crate::image_warmup::WarmupTestObservation>,
    #[cfg(test)]
    warmup_reap_observations: Vec<(
        codex_router_keeper_protocol::ChildPid,
        std::process::ExitStatus,
    )>,
}
struct PendingWarmup {
    cleanup: WarmupCleanup,
    _image: OwnedImageNode,
}
#[path = "image_cache_discovery.rs"]
mod cache_discovery;
use cache_discovery::{discover, validate_layout};

enum ImageLinkOutcome {
    Existing,
    Linked,
    CrossDevice,
}
type LinkOperation = fn(&Path, &Path) -> std::io::Result<()>;
fn hard_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::hard_link(source, target)
}
impl ImageRegistry {
    pub async fn new(root: &Path) -> Result<Self, ImageError> {
        let root = root.to_owned();
        let images_root = tokio::task::spawn_blocking(move || {
            validate_root(&root)?;
            let keeper = root.join("keeper");
            ensure_private(&keeper)?;
            let images = keeper.join("images");
            ensure_private(&images)?;
            Ok::<_, ImageError>(images)
        })
        .await??;
        Ok(Self {
            images_root,
            entries: BTreeMap::new(),
            pending_warmups: Vec::new(),
            #[cfg(test)]
            last_warmup_observation: None,
            #[cfg(test)]
            warmup_reap_observations: Vec::new(),
        })
    }
    pub async fn pin(
        &mut self,
        source: &Path,
        expected: &BuildInfo,
    ) -> Result<ImageLease, ImageError> {
        self.pin_with_link(source, expected, hard_link, PREPARE_DEADLINE)
            .await
    }
    async fn pin_with_link(
        &mut self,
        source: &Path,
        expected: &BuildInfo,
        link: LinkOperation,
        budget: Duration,
    ) -> Result<ImageLease, ImageError> {
        self.pin_with_checkpoint(
            source,
            expected,
            link,
            budget,
            PostSpawnCheckpoint::Immediate,
        )
        .await
    }
    async fn pin_with_checkpoint(
        &mut self,
        source: &Path,
        expected: &BuildInfo,
        link: LinkOperation,
        budget: Duration,
        checkpoint: PostSpawnCheckpoint,
    ) -> Result<ImageLease, ImageError> {
        #[cfg(test)]
        {
            self.last_warmup_observation = None;
        }
        let source = capture(source, true).await?;
        if let Some(record) = self
            .entries
            .get(&source.digest)
            .map(|entry| entry.record.clone())
        {
            self.validate_record_path(&record).await?;
            verify(&record).await?;
            let entry = self
                .entries
                .get_mut(&source.digest)
                .ok_or(ImageError::ForeignNode)?;
            if &entry.build_info != expected {
                return Err(ImageError::BuildInfoMismatch);
            }
            return Ok(lease_entry(entry));
        }
        let directory = self.images_root.join(digest_hex(&source.digest));
        let directory_check = directory.clone();
        let root = self.images_root.clone();
        tokio::task::spawn_blocking(move || {
            validate_root(&root)?;
            private_directory(root.parent().ok_or(ImageError::PrivateDirectory)?)?;
            ensure_private(&directory_check)
        })
        .await??;
        let retained = directory.join("codex-router");
        let source_path = source.path.clone();
        let target = retained.clone();
        let linked = tokio::task::spawn_blocking(move || {
            // A prior EXDEV copy must be recognized even when linking its original
            // source would still report EXDEV before a destination-exists check.
            match std::fs::symlink_metadata(&target) {
                Ok(_) => return Ok(ImageLinkOutcome::Existing),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            match link(&source_path, &target) {
                Ok(()) => Ok(ImageLinkOutcome::Linked),
                Err(error)
                    if error.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error()) =>
                {
                    Ok(ImageLinkOutcome::CrossDevice)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    Ok(ImageLinkOutcome::Existing)
                }
                Err(error) => Err(error),
            }
        })
        .await??;
        let node = match linked {
            ImageLinkOutcome::Linked => {
                let metadata_path = retained.clone();
                let metadata =
                    tokio::task::spawn_blocking(move || std::fs::symlink_metadata(metadata_path))
                        .await??;
                let guard = OwnedImageNode::new(retained.clone(), &metadata);
                let actual = capture(&retained, false).await?;
                if actual.metadata.dev() != source.metadata.dev()
                    || actual.metadata.ino() != source.metadata.ino()
                    || actual.digest != source.digest
                    || actual.metadata.mode() != source.metadata.mode()
                {
                    return Err(ImageError::ImageUnavailable);
                }
                guard
            }
            ImageLinkOutcome::CrossDevice => {
                let candidate = directory.join(".candidate");
                let create_path = candidate.clone();
                let mode = source.metadata.mode() & 0o7777;
                let creation = DescriptorGate::global().creation().await;
                let file = tokio::task::spawn_blocking(move || {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(mode)
                        .open(create_path)
                })
                .await??;
                drop(creation);
                let guard = OwnedImageNode::new(candidate.clone(), &file.metadata()?);
                let bytes = source.bytes.clone();
                tokio::task::spawn_blocking(move || {
                    let mut file = file;
                    file.write_all(&bytes)?;
                    file.set_permissions(Permissions::from_mode(mode))?;
                    Ok::<_, std::io::Error>(())
                })
                .await??;
                let actual = capture(&candidate, false).await?;
                if actual.digest != source.digest
                    || actual.bytes.len() != source.bytes.len()
                    || actual.metadata.mode() & 0o7777 != mode
                {
                    return Err(ImageError::ImageUnavailable);
                }
                match warmup_with_checkpoint(
                    &candidate,
                    expected,
                    budget,
                    checkpoint.clone(),
                    #[cfg(test)]
                    &mut self.last_warmup_observation,
                )
                .await
                {
                    WarmupOutcome::Verified => {}
                    WarmupOutcome::Refused {
                        reason,
                        finished_pid,
                    } => {
                        let _reaped_child = finished_pid;
                        return Err(reason);
                    }
                    WarmupOutcome::CleanupPending { cleanup, failure } => {
                        self.pending_warmups.push(PendingWarmup {
                            cleanup,
                            _image: guard,
                        });
                        return Err(failure);
                    }
                }
                let after_warmup = capture(&candidate, false).await?;
                if after_warmup.digest != source.digest
                    || after_warmup.metadata.dev() != actual.metadata.dev()
                    || after_warmup.metadata.ino() != actual.metadata.ino()
                {
                    #[cfg(test)]
                    if let Some(witness) = self.last_warmup_observation.as_mut() {
                        witness.stage = crate::image_warmup::WarmupObservedStage::Rejected(
                            crate::image_warmup::WarmupRejectionKind::ImageUnavailable,
                        );
                    }
                    return Err(ImageError::ImageUnavailable);
                }
                // Publish without replacing a pre-existing or raced foreign node.
                let from = candidate.clone();
                let target = retained.clone();
                tokio::task::spawn_blocking(move || std::fs::hard_link(from, target)).await??;
                let retained_guard = OwnedImageNode::new(retained.clone(), &actual.metadata);
                drop(guard);
                retained_guard
            }
            ImageLinkOutcome::Existing => {
                validate_layout(&directory).await?;
                let actual = capture(&retained, false).await?;
                if actual.metadata.uid() != rustix::process::geteuid().as_raw()
                    || actual.digest != source.digest
                    || actual.bytes != source.bytes
                    || actual.metadata.mode() & 0o7777 != source.metadata.mode() & 0o7777
                {
                    return Err(ImageError::ImageUnavailable);
                }
                // A prior copy may have a different inode. It is captured anew,
                // but never acquires a candidate's unlink-on-failure authority.
                let guard = OwnedImageNode::observed_existing(&actual.metadata);
                match warmup_with_checkpoint(
                    &retained,
                    expected,
                    budget,
                    checkpoint.clone(),
                    #[cfg(test)]
                    &mut self.last_warmup_observation,
                )
                .await
                {
                    WarmupOutcome::Verified => {}
                    WarmupOutcome::Refused {
                        reason,
                        finished_pid,
                    } => {
                        let _reaped_child = finished_pid;
                        return Err(reason);
                    }
                    WarmupOutcome::CleanupPending { cleanup, failure } => {
                        self.pending_warmups.push(PendingWarmup {
                            cleanup,
                            _image: guard,
                        });
                        return Err(failure);
                    }
                }
                let after = capture(&retained, false).await?;
                if after.digest != actual.digest
                    || after.bytes != source.bytes
                    || after.metadata.dev() != actual.metadata.dev()
                    || after.metadata.ino() != actual.metadata.ino()
                    || after.metadata.mode() != actual.metadata.mode()
                    || after.metadata.uid() != actual.metadata.uid()
                {
                    return Err(ImageError::ImageUnavailable);
                }
                guard
            }
        };
        let original = capture(&source.path, false).await?;
        if original.digest != source.digest
            || original.metadata.dev() != source.metadata.dev()
            || original.metadata.ino() != source.metadata.ino()
            || original.metadata.mode() != source.metadata.mode()
        {
            return Err(ImageError::ImageUnavailable);
        }
        let record = SlotImage::new(retained, source.digest, node.device, node.inode)?;
        verify(&record).await?;
        validate_layout(&directory).await?;
        // Only registry collection owns removal of a ready image.
        let record_key = *record.file_sha256();
        let mut entry = CachedImage {
            record,
            build_info: expected.clone(),
            live: Weak::new(),
        };
        let lease = lease_entry(&mut entry);
        self.entries.insert(record_key, entry);
        node.disarm();
        Ok(lease)
    }
    pub async fn reacquire(
        &mut self,
        record: SlotImage,
        expected: BuildInfo,
    ) -> Result<ImageLease, ImageError> {
        self.validate_record_path(&record).await?;
        verify(&record).await?;
        let key = *record.file_sha256();
        if let Some(entry) = self.entries.get_mut(&key) {
            if entry.record != record || entry.build_info != expected {
                return Err(ImageError::ForeignNode);
            }
            return Ok(lease_entry(entry));
        }
        let mut entry = CachedImage {
            record,
            build_info: expected,
            live: Weak::new(),
        };
        let lease = lease_entry(&mut entry);
        self.entries.insert(key, entry);
        Ok(lease)
    }
    async fn validate_record_path(&self, record: &SlotImage) -> Result<(), ImageError> {
        validate_launch_record(&self.images_root, record).await
    }
    pub(crate) fn native_probe_launcher(
        &self,
        image: &ImageLease,
    ) -> crate::native_probe_launch::ProbeLauncher {
        crate::native_probe_launch::ProbeLauncher::new(self.images_root.clone(), image.clone())
    }
    pub async fn spawn(
        &self,
        lease: &ImageLease,
        arguments: &[OsString],
    ) -> Result<ImageLaunchOutcome, ImageError> {
        self.spawn_with_checkpoint(lease, arguments, PostSpawnCheckpoint::Immediate)
            .await
    }
    async fn spawn_with_checkpoint(
        &self,
        lease: &ImageLease,
        arguments: &[OsString],
        checkpoint: PostSpawnCheckpoint,
    ) -> Result<ImageLaunchOutcome, ImageError> {
        // Clone before any asynchronous validation/spawn; the future itself also
        // retains the exact ready image while queued behind the descriptor gate.
        let image = lease.clone();
        self.validate_record_path(image.image()).await?;
        verify(image.image()).await?;
        let mut command = Command::new(image.image().retained_path());
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(
            match OwnedProcessGroup::spawn_with_checkpoint(command, checkpoint).await {
                GroupLaunchOutcome::Launched(group) => {
                    ImageLaunchOutcome::Launched { group, image }
                }
                GroupLaunchOutcome::Refused { reason } => ImageLaunchOutcome::Refused { reason },
                GroupLaunchOutcome::CleanupPending { reason, cleanup } => {
                    ImageLaunchOutcome::CleanupPending {
                        reason,
                        cleanup,
                        image,
                    }
                }
            },
        )
    }
    pub async fn collect(&mut self) -> Result<Vec<SlotImage>, ImageError> {
        // Failed warmup still owns its process and unpublished inode until the
        // existing raw reaper observes true emptiness. No retry or readiness is inferred.
        let mut failure = None;
        #[cfg(test)]
        let reap_observations = &mut self.warmup_reap_observations;
        self.pending_warmups.retain_mut(|pending| {
            match pending.cleanup.tick(tokio::time::Instant::now()) {
                Ok(true) => {
                    #[cfg(test)]
                    if let Some(observation) = pending.cleanup.reaped_status() {
                        reap_observations.push(observation);
                    }
                    false
                }
                Ok(false) => true,
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(ImageError::Process(error));
                    }
                    true
                }
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        // Discovery never replaces an existing record or its live-reference authority.
        // Fresh callers must reconstruct all carried references before collection.
        let discovered = discover(&self.images_root).await?;
        let mut unused: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.live.strong_count() == 0)
            .map(|(key, entry)| (*key, entry.record.clone()))
            .collect();
        unused.extend(discovered.into_iter().filter_map(|record| {
            let key = *record.file_sha256();
            (!self.entries.contains_key(&key)).then_some((key, record))
        }));
        unused.retain(|(_, record)| {
            !self.pending_warmups.iter().any(|pending| {
                pending._image.device == record.device() && pending._image.inode == record.inode()
            })
        });
        let mut removed = Vec::new();
        for (key, record) in unused {
            self.validate_record_path(&record).await?;
            verify(&record).await?;
            let image = record.clone();
            let scan_parent = image
                .retained_path()
                .parent()
                .ok_or(ImageError::ForeignNode)?
                .to_owned();
            let creation = DescriptorGate::global().creation().await;
            let entries = tokio::task::spawn_blocking(move || {
                std::fs::read_dir(scan_parent)?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<Result<Vec<_>, std::io::Error>>()
            })
            .await??;
            drop(creation);
            tokio::task::spawn_blocking(move || {
                let path = image.retained_path();
                let parent = path.parent().ok_or(ImageError::ForeignNode)?;
                let [entry] = entries.as_slice() else {
                    return Err(ImageError::ForeignNode);
                };
                if entry != path || !same_node(&std::fs::symlink_metadata(path)?, &image) {
                    return Err(ImageError::ForeignNode);
                }
                std::fs::remove_file(path)?;
                std::fs::remove_dir(parent)?;
                Ok::<_, ImageError>(())
            })
            .await??;
            self.entries.remove(&key);
            removed.push(record);
        }
        Ok(removed)
    }
}
fn lease_entry(entry: &mut CachedImage) -> ImageLease {
    let live = match entry.live.upgrade() {
        Some(live) => live,
        None => Arc::new(LeasedImage {
            record: entry.record.clone(),
            build_info: entry.build_info.clone(),
        }),
    };
    entry.live = Arc::downgrade(&live);
    ImageLease(live)
}
#[cfg(test)]
#[path = "retained_image_registry_tests.rs"]
mod registry_tests;

/// The same retained image accompanies either live execution or failed-launch debt.
pub enum ImageLaunchOutcome {
    Launched {
        group: OwnedProcessGroup,
        image: ImageLease,
    },
    Refused {
        reason: crate::GroupStopError,
    },
    CleanupPending {
        reason: crate::GroupStopError,
        cleanup: OwnedSpawnCleanup,
        image: ImageLease,
    },
}

pub(crate) async fn validate_launch_record(
    images_root: &Path,
    record: &SlotImage,
) -> Result<(), ImageError> {
    let parent = images_root.join(digest_hex(record.file_sha256()));
    if record.retained_path() != parent.join("codex-router") {
        return Err(ImageError::ForeignNode);
    }
    let root = images_root.to_owned();
    tokio::task::spawn_blocking(move || {
        validate_root(&root)?;
        private_directory(root.parent().ok_or(ImageError::PrivateDirectory)?)?;
        private_directory(&parent)
    })
    .await?
}
