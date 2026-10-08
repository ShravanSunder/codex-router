use codex_router_keeper::{GenerationEndpointPublisher, PublicationError, SingletonAuthority};
use codex_router_keeper_protocol::{DefaultEndpointPath, GenerationAliasPath};
use std::{os::unix::fs::MetadataExt, path::Path};
#[path = "support/publication_fixture.rs"]
mod publication_fixture;
use publication_fixture::{NativeSocketFixture, TestResult, private_directory};
#[tokio::test]
async fn candidate_mismatch_absence_regular_file_and_occupied_temp_preserve_answering_predecessor()
-> TestResult {
    let directory = private_directory()?;
    let first =
        NativeSocketFixture::start(directory.path(), FIRST_ID, "gen-11111111-1.sock", b"N1")
            .await?;
    let next =
        NativeSocketFixture::start(directory.path(), NEXT_ID, "gen-11111111-2.sock", b"N2").await?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let endpoint = DefaultEndpointPath::try_from(directory.path().join("app-server.sock"))?;
    let mut publisher = GenerationEndpointPublisher::new(endpoint.clone(), &authority).await?;
    publisher.publish(&first.generation, &first.alias)?;
    let before = std::fs::symlink_metadata(endpoint.as_path())?;
    let absent = GenerationAliasPath::try_from(directory.path().join("gen-11111111-3.sock"))?;
    if publisher.publish(&next.generation, &first.alias).is_ok()
        || publisher.publish(&next.generation, &absent).is_ok()
    {
        return Err("invalid candidate committed".into());
    }
    let physical = std::fs::read_link(next.alias.as_path())?;
    std::fs::remove_file(next.alias.as_path())?;
    if publisher.publish(&next.generation, &next.alias).is_ok() {
        return Err("absent matching candidate committed".into());
    }
    std::fs::write(next.alias.as_path(), b"not a native alias")?;
    if publisher.publish(&next.generation, &next.alias).is_ok() {
        return Err("regular candidate file committed".into());
    }
    std::fs::remove_file(next.alias.as_path())?;
    std::os::unix::fs::symlink(&physical, next.alias.as_path())?;
    let after = std::fs::symlink_metadata(endpoint.as_path())?;
    if (before.dev(), before.ino()) != (after.dev(), after.ino())
        || std::fs::read_link(endpoint.as_path())? != Path::new("gen-11111111-1.sock")
    {
        return Err("pre-publication failure changed predecessor link".into());
    }
    let old = connect(endpoint.as_path()).await?;
    if request_reply(&old).await? != *b"N1" {
        return Err("failed candidate broke predecessor reply".into());
    }
    let temporary = directory.path().join(".app-server.sock.publication");
    std::fs::write(&temporary, b"occupied foreign temporary")?;
    let foreign_before = std::fs::symlink_metadata(&temporary)?;
    publisher.publish(&next.generation, &next.alias)?;
    let foreign_after = std::fs::symlink_metadata(&temporary)?;
    let new = connect(endpoint.as_path()).await?;
    if std::fs::read_link(endpoint.as_path())? != Path::new("gen-11111111-2.sock")
        || request_reply(&new).await? != *b"N2"
        || request_reply(&old).await? != *b"N1"
        || (foreign_before.dev(), foreign_before.ino())
            != (foreign_after.dev(), foreign_after.ino())
        || std::fs::read(&temporary)? != b"occupied foreign temporary"
    {
        return Err(
            "foreign staging blocked publication or changed owned/foreign replies and nodes".into(),
        );
    }
    drop(new);
    drop(old);
    first.finish().await?;
    next.finish().await?;
    Ok(())
}
#[tokio::test]
async fn foreign_replacement_link_and_directory_are_neither_overwritten_nor_unlinked() -> TestResult
{
    let directory = private_directory()?;
    let first =
        NativeSocketFixture::start(directory.path(), FIRST_ID, "gen-11111111-1.sock", b"N1")
            .await?;
    let next =
        NativeSocketFixture::start(directory.path(), NEXT_ID, "gen-11111111-2.sock", b"N2").await?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let endpoint = DefaultEndpointPath::try_from(directory.path().join("app-server.sock"))?;
    let mut publisher = GenerationEndpointPublisher::new(endpoint.clone(), &authority).await?;
    publisher.publish(&first.generation, &first.alias)?;
    let owned = std::fs::symlink_metadata(endpoint.as_path())?;
    std::fs::rename(endpoint.as_path(), directory.path().join("old-owned-link"))?;
    // Keep the old inode alive: inode reuse cannot make this a false replacement test.
    std::os::unix::fs::symlink("gen-11111111-1.sock", endpoint.as_path())?;
    let replacement = std::fs::symlink_metadata(endpoint.as_path())?;
    if owned.ino() == replacement.ino() {
        return Err("fixture failed to replace inode".into());
    }
    if !matches!(
        publisher.publish(&next.generation, &next.alias),
        Err(PublicationError::OwnershipLost)
    ) {
        return Err("replacement inode was overwritten".into());
    }
    drop(publisher);
    if std::fs::symlink_metadata(endpoint.as_path())?.ino() != replacement.ino() {
        return Err("cleanup unlinked another owner with identical target".into());
    }
    let mut adopted = GenerationEndpointPublisher::adopt(
        endpoint.clone(),
        &authority,
        &first.generation,
        &first.alias,
    )
    .await?;
    std::fs::rename(
        endpoint.as_path(),
        directory.path().join("second-owned-link"),
    )?;
    std::fs::create_dir(endpoint.as_path())?;
    std::fs::write(endpoint.as_path().join("foreign"), b"foreign directory")?;
    if adopted.publish(&next.generation, &next.alias).is_ok() {
        return Err("foreign directory overwritten".into());
    }
    drop(adopted);
    if std::fs::read(endpoint.as_path().join("foreign"))? != b"foreign directory" {
        return Err("foreign directory changed by cleanup".into());
    }
    first.finish().await?;
    next.finish().await?;
    Ok(())
}
#[tokio::test]
async fn adoption_requires_exact_relative_link_generation_and_parent_then_can_publish_next()
-> TestResult {
    let directory = private_directory()?;
    let first =
        NativeSocketFixture::start(directory.path(), FIRST_ID, "gen-11111111-1.sock", b"N1")
            .await?;
    let next =
        NativeSocketFixture::start(directory.path(), NEXT_ID, "gen-11111111-2.sock", b"N2").await?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let endpoint = DefaultEndpointPath::try_from(directory.path().join("app-server.sock"))?;
    std::os::unix::fs::symlink("gen-11111111-1.sock", endpoint.as_path())?;
    let before = std::fs::symlink_metadata(endpoint.as_path())?;
    if GenerationEndpointPublisher::adopt(
        endpoint.clone(),
        &authority,
        &next.generation,
        &next.alias,
    )
    .await
    .is_ok()
    {
        return Err("mismatched adoption accepted".into());
    }
    let wrong_parent =
        GenerationAliasPath::try_from(Path::new("/other/private/gen-11111111-1.sock").to_owned())?;
    if GenerationEndpointPublisher::adopt(
        endpoint.clone(),
        &authority,
        &first.generation,
        &wrong_parent,
    )
    .await
    .is_ok()
        || std::fs::symlink_metadata(endpoint.as_path())?.ino() != before.ino()
    {
        return Err("mismatched adoption affected endpoint".into());
    }
    let mut adopted = GenerationEndpointPublisher::adopt(
        endpoint.clone(),
        &authority,
        &first.generation,
        &first.alias,
    )
    .await?;
    let old = connect(endpoint.as_path()).await?;
    if request_reply(&old).await? != *b"N1" {
        return Err("adopted predecessor did not reply".into());
    }
    adopted.publish(&next.generation, &next.alias)?;
    let new = connect(endpoint.as_path()).await?;
    if request_reply(&new).await? != *b"N2" || request_reply(&old).await? != *b"N1" {
        return Err("adopted publisher changed old/new connection association".into());
    }
    drop(old);
    drop(new);
    drop(adopted);
    first.finish().await?;
    next.finish().await?;
    Ok(())
}

const FIRST_ID: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":1}"#;

const NEXT_ID: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":2}"#;
#[path = "support/publication_exchange.rs"]
mod publication_exchange;
use publication_exchange::{connect, request_reply};
