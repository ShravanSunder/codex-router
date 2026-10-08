//! Owned retained-path launch projection; no registry borrow outlives the call.
use crate::retained_image_registry::validate_launch_record;
use crate::{GroupLaunchOutcome, ImageError, ImageLaunchOutcome, ImageLease, OwnedProcessGroup};
use codex_router_keeper_protocol::ComponentKind;
use std::{ffi::OsString, future::Future, path::PathBuf, pin::Pin, process::Stdio};
use tokio::process::Command;
pub(crate) type ProbeLaunchFuture =
    Pin<Box<dyn Future<Output = Result<ImageLaunchOutcome, ImageError>> + Send>>;
pub(crate) const NATIVE_PROBE_DISPATCH: &str = "--internal-native-probe";
#[derive(Clone)]
pub(crate) struct ProbeLauncher {
    root: PathBuf,
    image: ImageLease,
    prefix: Vec<OsString>,
}
impl ProbeLauncher {
    pub(crate) fn new(root: PathBuf, image: ImageLease) -> Self {
        Self {
            root,
            image,
            prefix: Vec::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture_prefix(mut self, prefix: Vec<OsString>) -> Self {
        self.prefix = prefix;
        self
    }
    pub(crate) fn image(&self) -> &ImageLease {
        &self.image
    }
    pub(crate) fn launch(&self) -> ProbeLaunchFuture {
        let root = self.root.clone();
        let image = self.image.clone();
        let prefix = self.prefix.clone();
        Box::pin(async move {
            validate_launch_record(&root, image.image()).await?;
            crate::image_identity::verify(image.image()).await?;
            let mut command = Command::new(image.image().retained_path());
            command
                .args(prefix)
                .arg(NATIVE_PROBE_DISPATCH)
                .arg(image.fingerprint(ComponentKind::Keeper).to_hex())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            Ok(match OwnedProcessGroup::spawn(command).await {
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
            })
        })
    }
}
