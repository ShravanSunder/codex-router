use codex_router_keeper::{ListenerAddress, ListenerRegistry, RegistryError, SingletonAuthority};
use codex_router_keeper_protocol::ListenerKind;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
fn directory() -> Result<tempfile::TempDir, std::io::Error> {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}
#[tokio::test]
async fn registry_reuses_original_and_refuses_conflicting_associations() -> TestResult {
    let directory = directory()?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let mut registry = ListenerRegistry::new(authority);
    let path = directory.path().join("control.sock");
    let address = ListenerAddress::unix(path.clone())?;
    if (registry
        .bind(ListenerKind::CollaborationControl, address.clone())
        .await?)
        != (address)
    {
        return Err("listener_ownership scenario assertion failed".into());
    }
    let before = std::fs::symlink_metadata(&path)?;
    registry
        .bind(ListenerKind::CollaborationControl, address.clone())
        .await?;
    if (std::fs::symlink_metadata(&path)?.ino()) != (before.ino()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if (std::fs::metadata(&path)?.permissions().mode() & 0o777) != (0o600) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if !(matches!(
        registry.bind(ListenerKind::NativeRelay, address).await,
        Err(RegistryError::AssociationConflict)
    )) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if !(matches!(
        registry
            .bind(
                ListenerKind::CollaborationControl,
                ListenerAddress::unix(directory.path().join("other.sock"))?
            )
            .await,
        Err(RegistryError::AssociationConflict)
    )) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    let grant = registry
        .duplicate(&ListenerKind::CollaborationControl)
        .await?;
    if (grant.kind()) != (&ListenerKind::CollaborationControl) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if !(rustix::io::fcntl_getfd(grant.descriptor())?.contains(rustix::io::FdFlags::CLOEXEC)) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    drop(grant);
    drop(registry);
    if !(!path.exists()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    Ok(())
}
#[tokio::test]
async fn occupied_foreign_nodes_and_replaced_inode_are_never_removed() -> TestResult {
    let directory = directory()?;
    let mut registry = ListenerRegistry::new(
        SingletonAuthority::acquire(&directory.path().join("host.lock")).await?,
    );
    let foreign = directory.path().join("foreign.sock");
    std::fs::write(&foreign, b"foreign bytes")?;
    if !(registry
        .bind(
            ListenerKind::NativeRelay,
            ListenerAddress::unix(foreign.clone())?,
        )
        .await
        .is_err())
    {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if (std::fs::read(&foreign)?) != (b"foreign bytes") {
        return Err("listener_ownership scenario assertion failed".into());
    }
    let path = directory.path().join("control.sock");
    registry
        .bind(
            ListenerKind::CollaborationControl,
            ListenerAddress::unix(path.clone())?,
        )
        .await?;
    let old = std::fs::metadata(&path)?;
    let held_name = directory.path().join("old.sock");
    std::fs::rename(&path, &held_name)?;
    let replacement = std::os::unix::net::UnixListener::bind(&path)?;
    if (std::fs::metadata(&path)?.ino()) == (old.ino()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    drop(registry);
    if !(path.exists()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    drop(replacement);
    Ok(())
}
#[test]
fn address_boundary_rejects_relative_missing_public_parent_and_nonloopback() -> TestResult {
    if !(ListenerAddress::unix("relative.sock".into()).is_err()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    let directory = directory()?;
    if !(ListenerAddress::unix(directory.path().join("absent/socket")).is_err()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))?;
    if !(ListenerAddress::unix(directory.path().join("public.sock")).is_err()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    for address in ["0.0.0.0:1234", "192.0.2.1:1234", "[::]:1234"] {
        if !(ListenerAddress::tcp(address.parse()?).is_err()) {
            return Err("listener_ownership scenario assertion failed".into());
        }
    }
    if !(ListenerAddress::tcp("127.0.0.1:0".parse()?).is_ok()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if !(ListenerAddress::tcp("[::1]:0".parse()?).is_ok()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    Ok(())
}
#[tokio::test]
async fn kind_address_and_router_face_identity_are_checked_before_binding() -> TestResult {
    let directory = directory()?;
    let mut registry = ListenerRegistry::new(
        SingletonAuthority::acquire(&directory.path().join("host.lock")).await?,
    );
    let wrong = directory.path().join("wrong.sock");
    if !(matches!(
        registry
            .bind(
                ListenerKind::ProxyHttp,
                ListenerAddress::unix(wrong.clone())?
            )
            .await,
        Err(RegistryError::KindAddressMismatch)
    )) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    let face: ListenerKind =
        serde_json::from_str(r#"{"kind":"routerSessionFace","endpoint":"claude-local"}"#)?;
    if !(matches!(
        registry
            .bind(face.clone(), ListenerAddress::unix(wrong.clone())?)
            .await,
        Err(RegistryError::KindAddressMismatch)
    )) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if !(!wrong.exists()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    let faces = directory.path().join("router-sessions");
    std::fs::create_dir(&faces)?;
    std::fs::set_permissions(&faces, std::fs::Permissions::from_mode(0o700))?;
    registry
        .bind(
            face,
            ListenerAddress::unix(faces.join("claude-local.sock"))?,
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn router_face_requires_the_existing_router_sessions_address_relationship() -> TestResult {
    let directory = directory()?;
    let mut registry = ListenerRegistry::new(
        SingletonAuthority::acquire(&directory.path().join("host.lock")).await?,
    );
    let face: ListenerKind =
        serde_json::from_str(r#"{"kind":"routerSessionFace","endpoint":"claude-local"}"#)?;
    let wrong_parent = directory.path().join("claude-local.sock");
    if !(matches!(
        registry
            .bind(face, ListenerAddress::unix(wrong_parent.clone())?)
            .await,
        Err(RegistryError::KindAddressMismatch)
    )) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    if !(!wrong_parent.exists()) {
        return Err("listener_ownership scenario assertion failed".into());
    }
    Ok(())
}
