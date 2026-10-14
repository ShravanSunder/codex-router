//! Select only the private peer registry established by the actual normal-HOME Host.
use super::ProofResult;
use serde_json::Value;
use std::{
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

pub(super) fn selected_directory(root: &Path) -> ProofResult<PathBuf> {
    let context: Value =
        serde_json::from_slice(&std::fs::read(root.join("debug-host-context.json"))?)?;
    if context.get("kind").and_then(Value::as_str) != Some("debugHostPrepared") {
        return Err("peer fixture requires the normal-HOME acceptance Host context".into());
    }
    let selected = context
        .get("peerRegistryDirectory")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or("Host did not select a task-private peer registry")?;
    let expected = root.join("peer-registry");
    let metadata = std::fs::symlink_metadata(&selected)?;
    if !selected.is_absolute()
        || !metadata.file_type().is_dir()
        || metadata.uid() != std::fs::metadata(root)?.uid()
        || metadata.permissions().mode() & 0o077 != 0
        || selected.canonicalize()? != expected.canonicalize()?
        || selected.canonicalize()?.parent() != Some(root.canonicalize()?.as_path())
    {
        return Err("Host peer registry must be its owner-private peer-registry child".into());
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::{DirBuilderExt as _, symlink};

    fn context(root: &Path, selected: Option<&Path>) -> ProofResult<()> {
        std::fs::write(
            root.join("debug-host-context.json"),
            serde_json::to_vec(&json!({
                "kind":"debugHostPrepared", "peerRegistryDirectory":selected,
            }))?,
        )?;
        Ok(())
    }

    #[test]
    fn actual_private_host_selection_is_consumed_without_a_fake_home() -> ProofResult<()> {
        let root = tempfile::tempdir()?;
        let registry = root.path().join("peer-registry");
        std::fs::DirBuilder::new().mode(0o700).create(&registry)?;
        context(root.path(), Some(&registry))?;
        if selected_directory(root.path())? != registry {
            return Err("selected registry differs from the actual private Host selection".into());
        }
        if root.path().join("home").exists() {
            return Err("registry selection created an unexpected fake home".into());
        }
        Ok(())
    }

    #[test]
    fn absent_foreign_public_and_symlinked_registry_are_refused() -> ProofResult<()> {
        let root = tempfile::tempdir()?;
        let foreign = tempfile::tempdir()?;
        context(root.path(), None)?;
        if selected_directory(root.path()).is_ok() {
            return Err("absent Host registry selection was accepted".into());
        }
        context(root.path(), Some(foreign.path()))?;
        if selected_directory(root.path()).is_ok() {
            return Err("foreign Host registry selection was accepted".into());
        }
        let registry = root.path().join("peer-registry");
        symlink(foreign.path(), &registry)?;
        context(root.path(), Some(&registry))?;
        if selected_directory(root.path()).is_ok() {
            return Err("symlinked Host registry selection was accepted".into());
        }
        std::fs::remove_file(&registry)?;
        std::fs::DirBuilder::new().mode(0o755).create(&registry)?;
        std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o755))?;
        if selected_directory(root.path()).is_ok() {
            return Err("public Host registry selection was accepted".into());
        }
        Ok(())
    }
}
