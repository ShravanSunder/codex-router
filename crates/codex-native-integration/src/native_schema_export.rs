//! Non-runtime schema export bound to the managed executable's captured content identity.
use crate::{ExecutableIdentity, NativeSchemaBundle, NativeSchemaError, executable_identity};
use std::{os::unix::fs::DirBuilderExt, path::Path, process::Stdio, time::Duration};

pub struct NativeSchemaExport {
    executable: ExecutableIdentity,
    bundle: NativeSchemaBundle,
}

impl NativeSchemaExport {
    /// Creates a new private output directory and invokes only the schema generator.
    /// The caller retains the export directory for cache publication or owned cleanup.
    pub async fn generate(
        executable: &ExecutableIdentity,
        output_directory: &Path,
    ) -> Result<Self, NativeSchemaError> {
        if executable_identity(executable.canonical_path())
            .await
            .map_err(NativeSchemaError::ExecutableIdentity)?
            != *executable
        {
            return Err(NativeSchemaError::ExecutableChanged);
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(output_directory)
            .map_err(NativeSchemaError::Filesystem)?;
        let mut child = tokio::process::Command::new(executable.canonical_path())
            .args([
                "app-server",
                "generate-json-schema",
                "--experimental",
                "--out",
            ])
            .arg(output_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(NativeSchemaError::Filesystem)?;
        let status = match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
            Ok(status) => status.map_err(NativeSchemaError::Filesystem)?,
            Err(_) => {
                child.kill().await.map_err(NativeSchemaError::Filesystem)?;
                return Err(NativeSchemaError::ExportFailed);
            }
        };
        if !status.success() {
            return Err(NativeSchemaError::ExportFailed);
        }
        let path = output_directory.to_path_buf();
        let bundle =
            tokio::task::spawn_blocking(move || NativeSchemaBundle::from_export_directory(&path))
                .await
                .map_err(|_| NativeSchemaError::ExportFailed)??;
        if executable_identity(executable.canonical_path())
            .await
            .map_err(NativeSchemaError::ExecutableIdentity)?
            != *executable
        {
            return Err(NativeSchemaError::ExecutableChanged);
        }
        Ok(Self {
            executable: executable.clone(),
            bundle,
        })
    }

    #[must_use]
    pub fn executable(&self) -> &ExecutableIdentity {
        &self.executable
    }

    #[must_use]
    pub fn bundle(&self) -> &NativeSchemaBundle {
        &self.bundle
    }
}
