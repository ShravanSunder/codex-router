//! Explicit external process/image stand-in; never a Router or business-role entrypoint.
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(path) = std::env::var_os("LAUNCH_JOIN_MARKER") {
        let pid = rustix::process::getpid();
        if rustix::process::getpgrp() != pid {
            return Err("fixture original group is not its leader".into());
        }
        let parent = codex_router_keeper_protocol::ChildPid::new(
            std::env::var("LAUNCH_PARENT_GROUP")?.parse::<i64>()?,
        )?;
        rustix::process::setpgid(None, Some(parent.as_pid()))?;
        if rustix::process::getpgrp() != parent.as_pid() {
            return Err("actual parent group join failed".into());
        }
        let path = PathBuf::from(path);
        let temporary = path.with_extension("writing");
        let witness = serde_json::json!({"marker":"REAL_JOINED_FIXTURE", "pid":pid.as_raw_pid(), "original_group":pid.as_raw_pid(), "joined_group":parent.as_pid().as_raw_pid()});
        std::fs::write(&temporary, serde_json::to_vec(&witness)?)?;
        std::fs::rename(temporary, path)?;
        loop {
            std::thread::park();
        }
    }
    if std::env::args().skip(1).collect::<Vec<_>>() == ["build-info", "--json"] {
        // Literal fixture BuildInfo oracle independent of production encoding.
        println!(
            r#"{{"packageVersion":"1.2.3","fingerprints":{{"keeper":"1111111111111111111111111111111111111111111111111111111111111111","agentCollaborationServices":"1111111111111111111111111111111111111111111111111111111111111111","agentProxyServices":"1111111111111111111111111111111111111111111111111111111111111111","agentProviderServices":"1111111111111111111111111111111111111111111111111111111111111111"}}}}"#
        );
        return Ok(());
    }
    Err("unknown fixture entrypoint".into())
}
