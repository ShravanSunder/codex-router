use super::*;
use crate::GroupStopProfile;
use codex_router_descriptor_boundary::PipeReader;
use serde::{Deserialize, Serialize};
use std::{io::Write, os::unix::process::CommandExt, process::Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::Duration;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum ImageStage {
    BeforeExec,
    AfterExec,
    Stopped,
}
#[derive(Debug, Serialize, Deserialize)]
struct ImageRecord {
    stage: ImageStage,
    parent_pid: ChildPid,
    child_pid: ChildPid,
    exit_signal: Option<i32>,
}
fn fixture_command(name: &str) -> Result<Command, std::io::Error> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", name, "--nocapture"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    Ok(command)
}
fn child_command(mode: &str) -> Result<Command, std::io::Error> {
    let mut command = fixture_command("owned_process_group::exec_tests::exec_fixture_child")?;
    command.env("OWNED_GROUP_CHILD_MODE", mode);
    Ok(command)
}
fn image_record(stage: ImageStage, child_pid: ChildPid, exit_signal: Option<i32>) -> TestResult {
    let record = ImageRecord {
        stage,
        parent_pid: ChildPid::new(i64::from(rustix::process::getpid().as_raw_pid()))?,
        child_pid,
        exit_signal,
    };
    println!("IMAGE {}", serde_json::to_string(&record)?);
    std::io::stdout().flush()?;
    Ok(())
}
async fn stdin_marker(expected: u8) -> TestResult {
    let gate = DescriptorGate::global();
    let input = PipeReader::from_owned(gate.duplicate(rustix::stdio::stdin()).await?, gate).await?;
    let mut marker = [0];
    timeout(Duration::from_secs(4), input.read_exact(&mut marker)).await??;
    if marker != [expected] {
        return Err("unexpected image acknowledgement".into());
    }
    Ok(())
}
async fn next_record(
    output: &mut BufReader<ChildStdout>,
) -> Result<ImageRecord, Box<dyn std::error::Error + Send + Sync>> {
    timeout(Duration::from_secs(5), async {
        let mut line = String::new();
        loop {
            line.clear();
            if output.read_line(&mut line).await? == 0 {
                return Err("image ended without record".into());
            }
            if let Some(body) = line.strip_prefix("IMAGE ") {
                return Ok(serde_json::from_str(body)?);
            }
        }
    })
    .await?
}
async fn child_ready(output: &mut BufReader<ChildStdout>) -> TestResult {
    timeout(Duration::from_secs(3), async {
        let mut line = String::new();
        loop {
            line.clear();
            if output.read_line(&mut line).await? == 0 {
                return Err("child ended before readiness".into());
            }
            if line.trim() == "CHILD_READY" {
                return Ok(());
            }
        }
    })
    .await?
}

#[tokio::test]
#[ignore = "compiled child actually executed by owned process/exec tests"]
async fn exec_fixture_child() -> TestResult {
    let _term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    println!("CHILD_READY");
    std::io::stdout().flush()?;
    match std::env::var("OWNED_GROUP_CHILD_MODE")?.as_str() {
        "ignore" => std::future::pending::<()>().await,
        "exit" => std::process::exit(42),
        "controlled-exit" => {
            stdin_marker(b'X').await?;
            std::process::exit(42);
        }
        "reply" => {
            stdin_marker(b'P').await?;
            println!("PONG");
            eprintln!("STDERR_REPLY");
        }
        _ => return Err("unknown child mode".into()),
    }
    Ok(())
}
#[tokio::test]
#[ignore = "compiled image actually self-executes in the permanent parent scenario"]
async fn exec_fixture_image() -> TestResult {
    if let Ok(claim) = std::env::var("OWNED_GROUP_EXEC_CHILD_PID") {
        let pid = ChildPid::new(claim.parse::<i64>()?)?;
        let mut group = OwnedProcessGroup::adopt_owned_child(pid)?;
        rustix::process::test_kill_process(pid.as_pid())?;
        image_record(ImageStage::AfterExec, pid, None)?;
        stdin_marker(b'S').await?;
        group.begin_stop(
            GroupStopTiming::shortened(
                GroupStopProfile::Normal,
                Duration::from_millis(80),
                Duration::from_secs(1),
            )?,
            Instant::now(),
        )?;
        let stopped = timeout(
            Duration::from_secs(3),
            group.wait_for_stop(&CancellationToken::new()),
        )
        .await??;
        if !matches!(stopped, GroupStopStatus::GroupEmpty { .. })
            || rustix::process::test_kill_process_group(group.process_group_id().as_pid())
                != Err(rustix::io::Errno::SRCH)
        {
            return Err("post-exec group is not empty".into());
        }
        let status = group.leader_exit_status().ok_or("post-exec exit missing")?;
        image_record(ImageStage::Stopped, pid, status.signal())?;
        return Ok(());
    }
    let mut group = OwnedProcessGroup::spawn(child_command("ignore")?).await?;
    let pid = group.leader_pid();
    let mut output = BufReader::new(group.take_stdout().ok_or("child stdout absent")?);
    child_ready(&mut output).await?;
    image_record(ImageStage::BeforeExec, pid, None)?;
    stdin_marker(b'E').await?;
    // Exec replaces this image while its production owner is still alive. Its CLOEXEC
    // pipes close; no Tokio Child/orphan queue has ever registered the retained child.
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "owned_process_group::exec_tests::exec_fixture_image",
            "--nocapture",
        ])
        .env(
            "OWNED_GROUP_EXEC_CHILD_PID",
            pid.as_pid().as_raw_pid().to_string(),
        );
    let error = command.exec();
    group.begin_stop(GroupStopTiming::normal(), Instant::now())?;
    group.wait_for_stop(&CancellationToken::new()).await?;
    Err(error.into())
}
struct FixtureGroupCleanup(Option<ChildPgid>);
impl Drop for FixtureGroupCleanup {
    fn drop(&mut self) {
        if let Some(group) = self.0 {
            let _cleanup = rustix::process::kill_process_group(group.as_pid(), Signal::KILL);
        }
    }
}
#[tokio::test]
async fn same_pid_exec_preserves_same_child_and_raw_reaping_authority() -> TestResult {
    let mut command = fixture_command("owned_process_group::exec_tests::exec_fixture_image")?;
    command.env_remove("OWNED_GROUP_EXEC_CHILD_PID");
    let mut helper = OwnedProcessGroup::spawn(command).await?;
    let mut input = helper.take_stdin().ok_or("helper stdin missing")?;
    let mut output = BufReader::new(helper.take_stdout().ok_or("helper stdout missing")?);
    let before = next_record(&mut output).await?;
    let mut cleanup = FixtureGroupCleanup(Some(ChildPgid::of_leader(before.child_pid)));
    if before.stage != ImageStage::BeforeExec || before.parent_pid != helper.leader_pid() {
        return Err("old image identity mismatch".into());
    }
    rustix::process::test_kill_process(before.child_pid.as_pid())?;
    input.write_all(b"E").await?;
    let after = next_record(&mut output).await?;
    if after.stage != ImageStage::AfterExec
        || after.parent_pid != before.parent_pid
        || after.child_pid != before.child_pid
    {
        return Err("exec changed parent or child PID".into());
    }
    rustix::process::test_kill_process(after.child_pid.as_pid())?;
    rustix::process::test_kill_process_group(after.child_pid.as_pid())?;
    input.write_all(b"S").await?;
    let stopped = next_record(&mut output).await?;
    if stopped.stage != ImageStage::Stopped
        || stopped.parent_pid != before.parent_pid
        || stopped.child_pid != before.child_pid
        || stopped.exit_signal != Some(9)
        || rustix::process::test_kill_process_group(before.child_pid.as_pid())
            != Err(rustix::io::Errno::SRCH)
    {
        return Err("post-exec child was not actually killed/reaped/empty".into());
    }
    cleanup.0 = None;
    timeout(Duration::from_secs(3), async {
        let mut ticks =
            tokio::time::interval_at(Instant::now() + GROUP_POLL_INTERVAL, GROUP_POLL_INTERVAL);
        loop {
            ticks.tick().await;
            helper.tick(Instant::now())?;
            if let Some(status) = helper.leader_exit_status() {
                if !status.success() {
                    return Err("exec helper failed".into());
                }
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
        }
    })
    .await??;
    eprintln!("EXEC_PROOF before={before:?} after={after:?} stopped={stopped:?}");
    Ok(())
}
#[tokio::test]
async fn non_child_claims_reject_without_signalling_and_invalid_ids_never_enter() -> TestResult {
    for raw in [0, 1, -1, i64::MAX] {
        if ChildPid::new(raw).is_ok() {
            return Err("invalid child PID entered the ownership boundary".into());
        }
    }
    for pid in [
        rustix::process::getpid(),
        rustix::process::getppid().ok_or("parent PID absent")?,
    ] {
        let claim = ChildPid::new(i64::from(pid.as_raw_pid()))?;
        if !(matches!(
            OwnedProcessGroup::adopt_owned_child(claim),
            Err(GroupStopError::NotOwnedChild)
        )) {
            return Err("non-child claim was not rejected with ECHILD".into());
        }
        rustix::process::test_kill_process(pid)?;
    }
    Ok(())
}

async fn wait_until_reaped(group: &mut OwnedProcessGroup) -> TestResult {
    timeout(Duration::from_secs(3), async {
        let mut ticks =
            tokio::time::interval_at(Instant::now() + GROUP_POLL_INTERVAL, GROUP_POLL_INTERVAL);
        loop {
            ticks.tick().await;
            let status = group.tick(Instant::now())?;
            if matches!(status, GroupStopStatus::GroupEmpty { .. })
                && group.leader_exit_status().is_some()
            {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
        }
    })
    .await?
}
#[tokio::test]
async fn converted_stdio_remains_cloexec_nonblocking_and_carries_real_bytes() -> TestResult {
    use tokio::io::AsyncReadExt;
    let mut command = child_command("reply")?;
    command.stderr(Stdio::piped());
    let mut group = OwnedProcessGroup::spawn(command).await?;
    let mut input = group.take_stdin().ok_or("stdin missing")?;
    let output = group.take_stdout().ok_or("stdout missing")?;
    let mut error_output = group.take_stderr().ok_or("stderr missing")?;
    if !(rustix::io::fcntl_getfd(&input)?.contains(rustix::io::FdFlags::CLOEXEC)) {
        return Err("stdin lost CLOEXEC".into());
    }
    if !(rustix::io::fcntl_getfd(&output)?.contains(rustix::io::FdFlags::CLOEXEC)) {
        return Err("stdout lost CLOEXEC".into());
    }
    if !(rustix::io::fcntl_getfd(&error_output)?.contains(rustix::io::FdFlags::CLOEXEC)) {
        return Err("stderr lost CLOEXEC".into());
    }
    if !(rustix::fs::fcntl_getfl(&input)?.contains(rustix::fs::OFlags::NONBLOCK)) {
        return Err("stdin was not nonblocking".into());
    }
    if !(rustix::fs::fcntl_getfl(&output)?.contains(rustix::fs::OFlags::NONBLOCK)) {
        return Err("stdout was not nonblocking".into());
    }
    if !(rustix::fs::fcntl_getfl(&error_output)?.contains(rustix::fs::OFlags::NONBLOCK)) {
        return Err("stderr was not nonblocking".into());
    }
    let mut output = BufReader::new(output);
    child_ready(&mut output).await?;
    input.write_all(b"P").await?;
    drop(input);
    let mut response = String::new();
    let mut error_response = String::new();
    timeout(Duration::from_secs(3), async {
        let (read_output, read_error) = tokio::join!(
            output.read_to_string(&mut response),
            error_output.read_to_string(&mut error_response)
        );
        read_output?;
        read_error?;
        Ok::<_, std::io::Error>(())
    })
    .await??;
    if !(response.lines().any(|line| line == "PONG")) {
        return Err("stdout did not carry literal PONG".into());
    }
    if !(error_response.lines().any(|line| line == "STDERR_REPLY")) {
        return Err("stderr did not carry its literal reply".into());
    }
    wait_until_reaped(&mut group).await?;
    if !(group.leader_exit_status().ok_or("exit missing")?.success()) {
        return Err("stdio fixture exit was unsuccessful".into());
    }
    if !((rustix::process::test_kill_process_group(group.process_group_id().as_pid()))
        == (Err(rustix::io::Errno::SRCH)))
    {
        return Err("stdio fixture group was not ESRCH".into());
    }
    Ok(())
}
#[tokio::test]
async fn owned_child_in_wrong_group_rejects_without_signal() -> TestResult {
    let mut command = child_command("ignore")?.into_std();
    // Deliberately inherit this test's group rather than establish a group leader.
    let mut child = DescriptorGate::global()
        .spawn_blocking(move || {
            command
                .spawn()
                .map_err(codex_router_descriptor_boundary::BoundaryError::Io)
        })
        .await?;
    let claim = ChildPid::try_from(child.id())?;
    let mut output = {
        let _creation = DescriptorGate::global().creation().await;
        BufReader::new(ChildStdout::from_std(
            child.stdout.take().ok_or("stdout absent")?,
        )?)
    };
    child_ready(&mut output).await?;
    let actual_group = rustix::process::getpgid(Some(claim.as_pid()))?;
    let rejected = matches!(
        OwnedProcessGroup::adopt_owned_child(claim),
        Err(GroupStopError::GroupMismatch)
    );
    let still_alive = rustix::process::test_kill_process(claim.as_pid()).is_ok();
    let group_unchanged = rustix::process::getpgid(Some(claim.as_pid()))? == actual_group;
    // Test owns this exact std child, which was never adopted or registered with Tokio.
    cleanup_failed_spawn(&mut child).await?;
    if !(rejected && still_alive && group_unchanged) {
        return Err("wrong-group rejection changed process liveness or group".into());
    }
    Ok(())
}
#[tokio::test]
async fn already_exited_owned_child_adopts_actual_status_once() -> TestResult {
    use rustix::process::{WaitId, WaitIdOptions};
    let mut command = child_command("exit")?.into_std();
    command.process_group(0);
    let child = DescriptorGate::global()
        .spawn_blocking(move || {
            command
                .spawn()
                .map_err(codex_router_descriptor_boundary::BoundaryError::Io)
        })
        .await?;
    let claim = ChildPid::try_from(child.id())?;
    // NOWAIT supplies an independent literal exit oracle without taking wait authority.
    timeout(Duration::from_secs(3), async {
        let mut ticks =
            tokio::time::interval_at(Instant::now() + GROUP_POLL_INTERVAL, GROUP_POLL_INTERVAL);
        loop {
            ticks.tick().await;
            if let Some(status) = rustix::process::waitid(
                WaitId::Pid(claim.as_pid()),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )? {
                if status.exit_status() != Some(42) {
                    return Err("independent NOWAIT exit oracle was not 42".into());
                }
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
        }
    })
    .await??;
    let mut group = OwnedProcessGroup::adopt_owned_child(claim)?;
    drop(child);
    if group.leader_exit_status().and_then(|status| status.code()) != Some(42) {
        return Err("adoption lost actual exit status 42".into());
    }
    if !(matches!(
        rustix::process::waitpid(Some(claim.as_pid()), WaitOptions::NOHANG),
        Err(rustix::io::Errno::CHILD)
    )) {
        return Err("adoption did not consume the child status exactly once".into());
    }
    // Recorded exit is never re-waited; repeated ticks preserve the literal status.
    wait_until_reaped(&mut group).await?;
    group.tick(Instant::now())?;
    if group.leader_exit_status().and_then(|status| status.code()) != Some(42) {
        return Err("later ticks changed recorded exit status 42".into());
    }
    Ok(())
}
#[tokio::test]
async fn unrecorded_echild_disqualifies_authority_and_never_fabricates_exit() -> TestResult {
    let mut group = OwnedProcessGroup::spawn(child_command("exit")?).await?;
    let mut output = BufReader::new(group.take_stdout().ok_or("stdout absent")?);
    child_ready(&mut output).await?;
    let pid = group.leader_pid();
    // Deliberately steal only this test-owned raw child's status to exercise the gap.
    timeout(Duration::from_secs(3), async {
        let mut ticks =
            tokio::time::interval_at(Instant::now() + GROUP_POLL_INTERVAL, GROUP_POLL_INTERVAL);
        loop {
            ticks.tick().await;
            if let Some((actual_pid, status)) =
                rustix::process::waitpid(Some(pid.as_pid()), WaitOptions::NOHANG)?
            {
                if actual_pid != pid.as_pid() {
                    return Err("exact-PID wait returned another PID".into());
                }
                if status.exit_status() != Some(42) {
                    return Err("stolen child status was not 42".into());
                }
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
        }
    })
    .await??;
    if !(matches!(
        group.tick(Instant::now()),
        Err(GroupStopError::NotOwnedChild)
    )) {
        return Err("unrecorded ECHILD was not an authority error".into());
    }
    if group.leader_exit_status().is_some() {
        return Err("ECHILD fabricated an exit status".into());
    }
    if !(matches!(
        group.begin_stop(GroupStopTiming::normal(), Instant::now()),
        Err(GroupStopError::NotOwnedChild)
    )) {
        return Err("disqualified owner attempted group stop".into());
    }
    if *group.progress() != GroupStopProgress::Running {
        return Err("authority error manufactured stop progress".into());
    }
    drop(group);
    Ok(())
}

#[tokio::test]
async fn controlled_exit_group_probe_before_after_sole_reap() -> TestResult {
    use rustix::process::{WaitId, WaitIdOptions};
    let mut group = OwnedProcessGroup::spawn(child_command("controlled-exit")?).await?;
    let mut output = BufReader::new(group.take_stdout().ok_or("controlled stdout absent")?);
    let mut input = group.take_stdin().ok_or("controlled stdin absent")?;
    child_ready(&mut output).await?;
    let pid = group.leader_pid().as_pid();
    if group.process_group_id().as_pid() != pid
        || rustix::process::getpgid(Some(pid))? != pid
        || pid == rustix::process::getpid()
    {
        return Err("controlled fixture is not the distinct owned group leader".into());
    }
    group.reap_leader()?;
    if group.leader_exit_status().is_some() || *group.progress() != GroupStopProgress::Running {
        return Err("NOHANG alive observation manufactured an exit or stop".into());
    }
    eprintln!(
        "CONTROLLED_ALIVE pid={pid:?} stored_exit={:?} progress={:?} process_probe={:?} group_probe={:?}",
        group.leader_exit_status(),
        group.progress(),
        rustix::process::test_kill_process(pid),
        rustix::process::test_kill_process_group(pid)
    );
    input.write_all(b"X").await?;
    let exited = timeout(Duration::from_secs(3), async {
        let mut ticks =
            tokio::time::interval_at(Instant::now() + GROUP_POLL_INTERVAL, GROUP_POLL_INTERVAL);
        loop {
            ticks.tick().await;
            if let Some(status) = rustix::process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )? {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(status);
            }
        }
    })
    .await??;
    if exited.exit_status() != Some(42) || group.leader_exit_status().is_some() {
        return Err("NOWAIT exit42 oracle missing or status was stolen".into());
    }
    let before_process = rustix::process::test_kill_process(pid);
    let before_group = rustix::process::test_kill_process_group(pid);
    let before_pgid = rustix::process::getpgid(Some(pid));
    eprintln!(
        "CONTROLLED_BEFORE_REAP pid={pid:?} nowait={exited:?} stored_exit={:?} progress={:?} process_probe={before_process:?} group_probe={before_group:?} getpgid={before_pgid:?}",
        group.leader_exit_status(),
        group.progress()
    );
    let observation = group.group_exists();
    eprintln!(
        "PRODUCTION_GROUP_OBSERVATION pid={pid:?} first_probe={before_group:?} result={observation:?} stored_exit={:?}",
        group.leader_exit_status()
    );
    if matches!(before_group, Err(rustix::io::Errno::PERM)) {
        if observation? || group.leader_exit_status().and_then(|status| status.code()) != Some(42) {
            return Err(
                "terminal PERM observation did not reconcile actual exit42 and actual empty probe"
                    .into(),
            );
        }
    } else {
        observation?;
    }
    group.reap_leader()?;
    let after_process = rustix::process::test_kill_process(pid);
    let after_group = rustix::process::test_kill_process_group(pid);
    let after_pgid = rustix::process::getpgid(Some(pid));
    let after_nowait = rustix::process::waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    );
    eprintln!(
        "CONTROLLED_AFTER_REAP pid={pid:?} stored_exit={:?} progress={:?} process_probe={after_process:?} group_probe={after_group:?} getpgid={after_pgid:?} nowait={after_nowait:?}",
        group.leader_exit_status(),
        group.progress()
    );
    if group.leader_exit_status().and_then(|status| status.code()) != Some(42)
        || *group.progress() != GroupStopProgress::Running
        || !matches!(after_nowait, Err(rustix::io::Errno::CHILD))
    {
        return Err(
            "sole reaper lost actual status, manufactured stop progress or failed to consume exit"
                .into(),
        );
    }
    // Actual group probe values are observations, not cross-platform EPERM assertions
    // or an invitation to infer a retirement fence without the production observation.
    Ok(())
}
