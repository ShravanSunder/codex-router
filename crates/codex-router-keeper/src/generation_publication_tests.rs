use super::*;
#[path = "../tests/support/publication_exchange.rs"]
mod publication_exchange;
#[path = "../tests/support/publication_fixture.rs"]
mod publication_fixture;
use publication_exchange::{connect, request_reply};
use publication_fixture::{NativeSocketFixture, TestResult, private_directory};
#[tokio::test]
async fn ordinary_rename_failure_preserves_current_reply_and_cleans_only_owned_temporary()
-> TestResult {
    let directory = private_directory()?;
    let first = NativeSocketFixture::start(
        directory.path(),
        r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":1}"#,
        "gen-11111111-1.sock",
        b"N1",
    )
    .await?;
    let next = NativeSocketFixture::start(
        directory.path(),
        r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":2}"#,
        "gen-11111111-2.sock",
        b"N2",
    )
    .await?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let endpoint = DefaultEndpointPath::try_from(directory.path().join("app-server.sock"))?;
    let mut publisher = GenerationEndpointPublisher::new(endpoint.clone(), &authority).await?;
    publisher.publish(&first.generation, &first.alias)?;
    let blocked = directory.path().join("not-a-symlink");
    fs::create_dir(&blocked)?;
    fs::write(blocked.join("foreign"), b"do not replace")?;
    let result = publisher.publish_with_rename(&next.generation, &next.alias, |source, _target| {
        if fs::read_link(source)? != Path::new("gen-11111111-2.sock") {
            return Err(std::io::Error::other("temporary target changed"));
        }
        // Exercise an actual filesystem rename failure, with the real prepared temporary node.
        fs::rename(source, &blocked)
    });
    if !matches!(result, Err(PublicationError::Filesystem(_)))
        || fs::read_link(endpoint.as_path())? != Path::new("gen-11111111-1.sock")
        || publisher.temporary_path()?.symlink_metadata().is_ok()
        || fs::read(blocked.join("foreign"))? != b"do not replace"
    {
        return Err("failed rename changed committed/foreign node or leaked temporary".into());
    }
    let old = connect(endpoint.as_path()).await?;
    if request_reply(&old).await? != *b"N1" {
        return Err("failed rename stopped predecessor replies".into());
    }
    drop(old);
    first.finish().await?;
    next.finish().await?;
    Ok(())
}
