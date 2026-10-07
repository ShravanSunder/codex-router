use codex_router_keeper::{GenerationEndpointPublisher, SingletonAuthority};
use codex_router_keeper_protocol::{DefaultEndpointPath, GenerationAliasPath, GenerationId};
use std::os::unix::fs::PermissionsExt;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[tokio::test]
async fn publication_absent_alias_and_foreign_endpoint_refuse_without_effect() -> TestResult {
    let directory = tempfile::Builder::new().prefix("pub-").tempdir_in("/tmp")?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let path = directory.path().join("app-server.sock");
    let endpoint = DefaultEndpointPath::try_from(path.clone())?;
    let id: GenerationId =
        serde_json::from_str(r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":1}"#)?;
    let alias = GenerationAliasPath::try_from(directory.path().join("gen-11111111-1.sock"))?;
    let mut publisher = GenerationEndpointPublisher::new(endpoint.clone(), &authority).await?;
    if publisher.publish(&id, &alias).is_ok() || path.exists() {
        return Err("absent alias was published".into());
    }
    drop(publisher);
    std::fs::write(&path, b"foreign endpoint")?;
    if GenerationEndpointPublisher::new(endpoint.clone(), &authority)
        .await
        .is_ok()
        || std::fs::read(&path)? != b"foreign endpoint"
    {
        return Err("foreign endpoint changed".into());
    }
    std::fs::rename(&path, directory.path().join("preserved-foreign-file"))?;
    std::os::unix::fs::symlink("unrelated-dangling.sock", &path)?;
    if GenerationEndpointPublisher::new(endpoint.clone(), &authority)
        .await
        .is_ok()
        || std::fs::read_link(&path)? != std::path::Path::new("unrelated-dangling.sock")
    {
        return Err("foreign dangling link was adopted or deleted".into());
    }
    std::fs::rename(&path, directory.path().join("preserved-foreign-link"))?;
    std::fs::create_dir(&path)?;
    std::fs::write(path.join("foreign"), b"foreign directory")?;
    if GenerationEndpointPublisher::new(endpoint, &authority)
        .await
        .is_ok()
        || std::fs::read(path.join("foreign"))? != b"foreign directory"
    {
        return Err("foreign directory was adopted or deleted".into());
    }
    Ok(())
}

#[path = "support/publication_fixture.rs"]
mod publication_fixture;
use publication_fixture::{NativeSocketFixture, private_directory};
#[tokio::test]
async fn actual_relative_swap_preserves_old_connection_and_routes_new_request_to_next() -> TestResult
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
    if std::fs::read_link(endpoint.as_path())? != std::path::Path::new("gen-11111111-1.sock") {
        return Err("first relative target changed".into());
    }
    let old = connect(endpoint.as_path()).await?;
    if request_reply(&old).await? != *b"N1" {
        return Err("first generation reply changed".into());
    }
    publisher.publish(&next.generation, &next.alias)?;
    if std::fs::read_link(endpoint.as_path())? != std::path::Path::new("gen-11111111-2.sock") {
        return Err("next relative target changed".into());
    }
    let new = connect(endpoint.as_path()).await?;
    if request_reply(&new).await? != *b"N2" || request_reply(&old).await? != *b"N1" {
        return Err("rename changed established connection or new destination".into());
    }
    drop(publisher);
    if endpoint.as_path().symlink_metadata().is_ok()
        || !first.alias.as_path().exists()
        || !next.alias.as_path().exists()
    {
        return Err("publisher cleanup changed generation alias lifetime".into());
    }
    if request_reply(&old).await? != *b"N1" {
        return Err("publisher cleanup interrupted old connection".into());
    }
    drop(old);
    drop(new);
    first.finish().await?;
    next.finish().await?;
    Ok(())
}
#[tokio::test]
async fn continuous_literal_probe_has_zero_missing_refused_or_wrong_replies_across_swaps()
-> TestResult {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::{
        sync::mpsc,
        time::{Duration, Instant, timeout},
    };
    let directory = private_directory()?;
    let first =
        NativeSocketFixture::start(directory.path(), FIRST_ID, "gen-11111111-1.sock", b"N1")
            .await?;
    let next =
        NativeSocketFixture::start(directory.path(), NEXT_ID, "gen-11111111-2.sock", b"N2").await?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let endpoint = DefaultEndpointPath::try_from(directory.path().join("app-server-control.sock"))?;
    let mut publisher = GenerationEndpointPublisher::new(endpoint.clone(), &authority).await?;
    publisher.publish(&first.generation, &first.alias)?;
    let finished = Arc::new(AtomicBool::new(false));
    let (observed, mut observations) = mpsc::channel(1);
    let client_finished = finished.clone();
    let probe = async {
        let mut count = 0;
        let mut longest = Duration::ZERO;
        while !client_finished.load(Ordering::SeqCst) {
            let started = Instant::now();
            let peer = connect(endpoint.as_path()).await?;
            let reply = request_reply(&peer).await?;
            if reply != *b"N1" && reply != *b"N2" {
                return Err("continuous probe wrong generation reply".into());
            }
            longest = longest.max(started.elapsed());
            count += 1;
            if observed.send(()).await.is_err() {
                if client_finished.load(Ordering::SeqCst) {
                    break;
                }
                return Err("publication observer lost before completion".into());
            }
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((count, longest))
    };
    let publishing = async {
        for swap in 0..128 {
            let candidate = if swap % 2 == 0 { &next } else { &first };
            publisher.publish(&candidate.generation, &candidate.alias)?;
            timeout(Duration::from_secs(1), observations.recv())
                .await?
                .ok_or("continuous client ended during publication")?;
        }
        finished.store(true, Ordering::SeqCst);
        drop(observations);
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (probed, published) = tokio::join!(probe, publishing);
    published?;
    let (count, longest) = probed?;
    println!(
        "publication swaps=128 requests={count} missing=0 refused=0 wrong=0 longest_first_reply_us={}",
        longest.as_micros()
    );
    if count < 128 || longest > Duration::from_secs(1) {
        return Err("continuous fixture probe failed coverage/timing bound".into());
    }
    first.finish().await?;
    next.finish().await?;
    Ok(())
}

const FIRST_ID: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":1}"#;

const NEXT_ID: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":2}"#;
#[path = "support/publication_exchange.rs"]
mod publication_exchange;
use publication_exchange::{connect, request_reply};
