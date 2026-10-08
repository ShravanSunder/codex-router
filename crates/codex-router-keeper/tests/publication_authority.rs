use codex_router_descriptor_boundary::DescriptorGate;
use codex_router_keeper::{
    GenerationEndpointPublisher, ListenerRegistry, RegistryError, SingletonAuthority,
};
use codex_router_keeper_protocol::DefaultEndpointPath;
use std::{path::Path, process::Stdio};
use tokio::time::{Duration, timeout};
#[path = "support/publication_fixture.rs"]
mod publication_fixture;
use publication_fixture::{NativeSocketFixture, TestResult, private_directory};
#[tokio::test]
#[ignore = "compiled contender entrypoint executed by permanent same-OFD lifetime scenario"]
async fn publication_lock_contender_fixture() -> TestResult {
    match SingletonAuthority::acquire(Path::new(&std::env::var("PUBLICATION_LOCK")?)).await {
        Ok(_authority) => println!("PUBLICATION_AUTHORITY_ACQUIRED"),
        Err(RegistryError::AlreadyRunning) => println!("PUBLICATION_AUTHORITY_BUSY"),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
async fn contender(lock: &Path, expected: &str) -> TestResult {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "publication_lock_contender_fixture",
            "--nocapture",
        ])
        .env("PUBLICATION_LOCK", lock)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = DescriptorGate::global().spawn_child(&mut command).await?;
    let output = timeout(Duration::from_secs(2), child.wait_with_output()).await??;
    if !output.status.success() || !String::from_utf8_lossy(&output.stdout).contains(expected) {
        return Err(format!(
            "lock contender failed: {:?} {} {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}
#[tokio::test]
async fn publisher_same_ofd_duplicate_keeps_singleton_busy_after_original_registry_drop()
-> TestResult {
    let directory = private_directory()?;
    let first =
        NativeSocketFixture::start(directory.path(), FIRST_ID, "gen-11111111-1.sock", b"N1")
            .await?;
    let lock = directory.path().join("host.lock");
    let authority = SingletonAuthority::acquire(&lock).await?;
    let endpoint = DefaultEndpointPath::try_from(directory.path().join("app-server.sock"))?;
    let mut publisher = GenerationEndpointPublisher::new(endpoint.clone(), &authority).await?;
    publisher.publish(&first.generation, &first.alias)?;
    let registry = ListenerRegistry::new(authority);
    contender(&lock, "PUBLICATION_AUTHORITY_BUSY").await?;
    drop(registry);
    contender(&lock, "PUBLICATION_AUTHORITY_BUSY").await?;
    if std::fs::read_link(endpoint.as_path())? != Path::new("gen-11111111-1.sock") {
        return Err("publisher authority drop changed publication".into());
    }
    drop(publisher);
    if std::fs::symlink_metadata(endpoint.as_path()).is_ok() {
        return Err("publisher retained owned link after authority release".into());
    }
    contender(&lock, "PUBLICATION_AUTHORITY_ACQUIRED").await?;
    first.finish().await?;
    Ok(())
}

const FIRST_ID: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":1}"#;
