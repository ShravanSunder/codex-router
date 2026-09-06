//! Read-only production identity snapshots; never signal a discovered process.
use std::{collections::BTreeMap, io, path::Path};
use tokio::process::Command;

#[derive(PartialEq, Eq)]
pub struct ProductionIdentity {
    listener_pid: u32,
    processes: BTreeMap<u32, String>,
}

pub async fn capture_production_identity() -> io::Result<ProductionIdentity> {
    let listener = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-iTCP:8787", "-sTCP:LISTEN", "-Fp"])
        .output()
        .await?;
    if !listener.status.success() {
        return Err(io::Error::other("production listener identity unavailable"));
    }
    let listeners: Vec<u32> = String::from_utf8_lossy(&listener.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('p').and_then(|value| value.parse().ok()))
        .collect();
    if listeners.len() != 1 {
        return Err(io::Error::other("production listener identity ambiguous"));
    }
    let listener_pid = *listeners
        .first()
        .ok_or_else(|| io::Error::other("production listener missing"))?;
    let process_list = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,lstart=,comm="])
        .output()
        .await?;
    if !process_list.status.success() {
        return Err(io::Error::other("process identity inspection unavailable"));
    }
    let mut rows = BTreeMap::new();
    for line in String::from_utf8_lossy(&process_list.stdout).lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let [pid, parent, _, _, _, _, _, executable @ ..] = fields.as_slice() else {
            continue;
        };
        if executable.is_empty() {
            continue;
        }
        if let (Ok(pid), Ok(parent)) = (pid.parse::<u32>(), parent.parse::<u32>()) {
            let identity = fields.iter().skip(2).copied().collect::<Vec<_>>().join(" ");
            rows.insert(pid, (parent, identity, executable.join(" ")));
        }
    }
    let router = rows
        .get(&listener_pid)
        .ok_or_else(|| io::Error::other("production router disappeared"))?;
    let parent_pid = router.0;
    let mut processes = BTreeMap::from([(listener_pid, router.1.clone())]);
    if let Some(parent) = rows.get(&parent_pid)
        && Path::new(&parent.2)
            .file_name()
            .is_some_and(|name| name == "codex-router")
    {
        processes.insert(parent_pid, parent.1.clone());
        for (pid, (parent, identity, executable)) in &rows {
            if *parent == parent_pid
                && Path::new(executable)
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("codex"))
            {
                processes.insert(*pid, identity.clone());
            }
        }
    }
    Ok(ProductionIdentity {
        listener_pid,
        processes,
    })
}
