use super::*;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::{fs::MetadataExt, process::ExitStatusExt},
    process::Stdio,
};
use tokio::{io::AsyncBufReadExt, time::timeout};

use super::generation_publication_tests::{publication_exchange, publication_fixture};
use publication_exchange::{connect, request_reply};
use publication_fixture::{NativeSocketFixture, TestResult, private_directory};

const FIRST_ID: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":1}"#;
const NEXT_ID: &str = r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":2}"#;
const FIXTURE_TEST: &str =
    "generation_publication::generation_publication_crash_tests::staged_publisher_fixture";

#[derive(Serialize, Deserialize, Debug)]
struct StagedWitness {
    marker: String,
    pid: u32,
    path: PathBuf,
    target: PathBuf,
}

#[tokio::test]
#[ignore = "compiled crash entrypoint executed by both permanent parent scenarios"]
async fn staged_publisher_fixture() -> TestResult {
    let root = PathBuf::from(std::env::var_os("PUBLICATION_CRASH_ROOT").ok_or("root absent")?);
    let after_first = std::env::var("PUBLICATION_CRASH_AFTER_FIRST")? == "true";
    let authority = SingletonAuthority::acquire(&root.join("host.lock")).await?;
    let endpoint = DefaultEndpointPath::try_from(root.join("app-server.sock"))?;
    let mut publisher = GenerationEndpointPublisher::new(endpoint, &authority).await?;
    let first: GenerationId = serde_json::from_str(FIRST_ID)?;
    let next: GenerationId = serde_json::from_str(NEXT_ID)?;
    let first_alias = GenerationAliasPath::try_from(root.join("gen-11111111-1.sock"))?;
    let next_alias = GenerationAliasPath::try_from(root.join("gen-11111111-2.sock"))?;
    if after_first {
        publisher.publish(&first, &first_alias)?;
    }
    let (generation, alias) = if after_first {
        (&next, &next_alias)
    } else {
        (&first, &first_alias)
    };
    publisher.publish_with_rename(generation, alias, |source, _target| {
        let witness = StagedWitness {
            marker: "PHYSICAL_STAGING_CAPTURED".to_owned(),
            pid: std::process::id(),
            path: source.to_owned(),
            target: fs::read_link(source)?,
        };
        let mut output = std::io::stdout().lock();
        writeln!(
            output,
            "PUBLICATION_STAGED {}",
            serde_json::to_string(&witness)?
        )?;
        output.flush()?;
        // The parent kills this exact child while the captured node is still uncommitted.
        let mut release = [0];
        std::io::stdin().read_exact(&mut release)?;
        Err(std::io::Error::other("crash fixture unexpectedly released"))
    })?;
    Err("crash fixture returned without abrupt death".into())
}

async fn read_staged_witness(output: tokio::process::ChildStdout) -> TestResultWitness {
    timeout(std::time::Duration::from_secs(5), async {
        let mut lines = tokio::io::BufReader::new(output).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(json) = line.strip_prefix("PUBLICATION_STAGED ") {
                return Ok(serde_json::from_str(json)?);
            }
        }
        Err("child ended before the physical staging marker".into())
    })
    .await?
}
type TestResultWitness = Result<StagedWitness, Box<dyn std::error::Error + Send + Sync>>;

async fn crash_then_publish(after_first: bool) -> TestResult {
    let directory = private_directory()?;
    let first =
        NativeSocketFixture::start(directory.path(), FIRST_ID, "gen-11111111-1.sock", b"N1")
            .await?;
    let next =
        NativeSocketFixture::start(directory.path(), NEXT_ID, "gen-11111111-2.sock", b"N2").await?;
    let endpoint = DefaultEndpointPath::try_from(directory.path().join("app-server.sock"))?;
    let foreign = directory
        .path()
        .join(".app-server.sock.publication.foreign");
    fs::write(&foreign, b"FOREIGN_STAGING_NODE")?;
    let foreign_before = fs::symlink_metadata(&foreign)?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("--exact")
        .arg(FIXTURE_TEST)
        .arg("--ignored")
        .arg("--nocapture")
        .env("PUBLICATION_CRASH_ROOT", directory.path())
        .env("PUBLICATION_CRASH_AFTER_FIRST", after_first.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut child = DescriptorGate::global().spawn_child(&mut command).await?;
    let pid = child.id().ok_or("spawned child PID absent")?;
    let observed = async {
        let witness = read_staged_witness(child.stdout.take().ok_or("stdout absent")?).await?;
        if witness.marker != "PHYSICAL_STAGING_CAPTURED"
            || witness.pid != pid
            || witness.path.parent() != Some(directory.path())
            || witness.path == endpoint.as_path()
            || witness.target
                != Path::new(if after_first {
                    "gen-11111111-2.sock"
                } else {
                    "gen-11111111-1.sock"
                })
            || fs::read_link(&witness.path)? != witness.target
        {
            return Err("physical crash boundary or child identity differs".into());
        }
        let staged_before = fs::symlink_metadata(&witness.path)?;
        if !staged_before.file_type().is_symlink()
            || !matches!(
                SingletonAuthority::acquire(&directory.path().join("host.lock")).await,
                Err(crate::RegistryError::AlreadyRunning)
            )
        {
            return Err("staged child did not retain singleton authority".into());
        }
        let committed_before = if after_first {
            Some(fs::symlink_metadata(endpoint.as_path())?)
        } else {
            None
        };
        let old = if after_first {
            if fs::read_link(endpoint.as_path())? != Path::new("gen-11111111-1.sock") {
                return Err("uncommitted stage changed predecessor target".into());
            }
            let connection = connect(endpoint.as_path()).await?;
            if request_reply(&connection).await? != *b"N1" {
                return Err("predecessor did not answer before child crash".into());
            }
            Some(connection)
        } else {
            if endpoint.as_path().symlink_metadata().is_ok() {
                return Err("crash before first commit created endpoint".into());
            }
            None
        };
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
            witness,
            staged_before,
            committed_before,
            old,
        ))
    }
    .await;
    // Even a failed marker assertion cannot leave the fixture child running.
    child.start_kill()?;
    let status = timeout(std::time::Duration::from_secs(5), child.wait()).await??;
    if status.signal() != Some(9) {
        return Err(format!("fixture death was not exact SIGKILL: {status:?}").into());
    }
    let (witness, staged_before, committed_before, old) = observed?;
    let staged_after = fs::symlink_metadata(&witness.path)?;
    if (staged_before.dev(), staged_before.ino()) != (staged_after.dev(), staged_after.ino())
        || fs::read_link(&witness.path)? != witness.target
    {
        return Err("crash did not preserve the actual captured staging inode/target".into());
    }
    eprintln!(
        "PUBLICATION_CRASH_REAPED after_first={after_first} pid={pid} signal=9 staging={:?} inode={} target={:?}",
        witness.path,
        staged_after.ino(),
        witness.target
    );
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let mut publisher = if after_first {
        GenerationEndpointPublisher::adopt(
            endpoint.clone(),
            &authority,
            &first.generation,
            &first.alias,
        )
        .await?
    } else {
        GenerationEndpointPublisher::new(endpoint.clone(), &authority).await?
    };
    if let Some(before) = committed_before {
        let after = fs::symlink_metadata(endpoint.as_path())?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino())
            || fs::read_link(endpoint.as_path())? != Path::new("gen-11111111-1.sock")
        {
            return Err("crash or adoption changed predecessor inode/target".into());
        }
    }
    let candidate = if after_first { &next } else { &first };
    let result = publisher.publish(&candidate.generation, &candidate.alias);
    eprintln!("PUBLICATION_AFTER_CRASH after_first={after_first} result={result:?}");
    result?;
    let new = connect(endpoint.as_path()).await?;
    if request_reply(&new).await? != *if after_first { b"N2" } else { b"N1" } {
        return Err("post-crash committed endpoint returned wrong literal reply".into());
    }
    if let Some(old) = &old
        && request_reply(old).await? != *b"N1"
    {
        return Err("post-crash rename interrupted established predecessor connection".into());
    }
    let staged_final = fs::symlink_metadata(&witness.path)?;
    let foreign_after = fs::symlink_metadata(&foreign)?;
    if (staged_before.dev(), staged_before.ino()) != (staged_final.dev(), staged_final.ino())
        || fs::read_link(&witness.path)? != witness.target
        || (foreign_before.dev(), foreign_before.ino())
            != (foreign_after.dev(), foreign_after.ino())
        || fs::read(&foreign)? != b"FOREIGN_STAGING_NODE"
    {
        return Err("successful restart changed crashed or foreign staging node".into());
    }
    drop(publisher);
    let stale_after_drop = fs::symlink_metadata(&witness.path)?;
    let foreign_after_drop = fs::symlink_metadata(&foreign)?;
    if endpoint.as_path().symlink_metadata().is_ok()
        || (staged_before.dev(), staged_before.ino())
            != (stale_after_drop.dev(), stale_after_drop.ino())
        || fs::read_link(&witness.path)? != witness.target
        || (foreign_before.dev(), foreign_before.ino())
            != (foreign_after_drop.dev(), foreign_after_drop.ino())
        || fs::read(&foreign)? != b"FOREIGN_STAGING_NODE"
    {
        return Err("owned cleanup touched stale staging or leaked endpoint".into());
    }
    eprintln!(
        "PUBLICATION_CRASH_RECOVERY after_first={after_first} stale_inode={} foreign_inode={} old_reply={} new_reply={} singleton_reacquired=true stale_foreign_preserved=true",
        staged_final.ino(),
        foreign_after.ino(),
        if after_first { "N1" } else { "none" },
        if after_first { "N2" } else { "N1" }
    );
    drop(new);
    drop(old);
    first.finish().await?;
    next.finish().await?;
    Ok(())
}

#[tokio::test]
async fn killed_staging_publisher_allows_adopted_predecessor_to_publish_next() -> TestResult {
    crash_then_publish(true).await
}

#[tokio::test]
async fn killed_staging_before_first_commit_allows_new_publication() -> TestResult {
    crash_then_publish(false).await
}
