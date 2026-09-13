use std::collections::BTreeSet;
use std::path::Path;

pub(super) async fn reap_fixture_host(
    child: &mut tokio::process::Child,
) -> Result<(), Box<dyn std::error::Error>> {
    if child.try_wait()?.is_none() {
        child.start_kill()?;
        let _status = child.wait().await?;
    }
    Ok(())
}

pub(super) fn terminate_logged_fixture_processes(
    private_logs: &[&Path],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut process_ids = BTreeSet::new();
    for private_log in private_logs {
        let contents = std::fs::read_to_string(private_log).unwrap_or_default();
        process_ids.extend(contents.lines().filter_map(|line| line.parse::<u32>().ok()));
    }

    for process_id in process_ids {
        if process_id == std::process::id() {
            return Err("fixture log unexpectedly named the test process".into());
        }
        let Some(process_id) = i32::try_from(process_id)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            continue;
        };
        match rustix::process::kill_process(process_id, rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
