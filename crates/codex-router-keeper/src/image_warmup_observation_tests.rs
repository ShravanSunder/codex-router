use super::*;
use crate::{GroupStopError, GroupStopProgress};
use codex_router_keeper_protocol::{
    ChildPid, ComponentFingerprint, ComponentFingerprints, GroupStopResult,
};
use std::os::unix::process::ExitStatusExt;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn exact_owned_unfindable_unreaped_predicate() -> TestResult {
    let pid = ChildPid::new(42)?.as_pid();
    let exited = std::process::ExitStatus::from_raw(0);
    for (name, error, exit, lookup, expected) in [
        (
            "unfindable-owned-exit",
            GroupStopError::Probe(rustix::io::Errno::PERM),
            None,
            Err(rustix::io::Errno::SRCH),
            true,
        ),
        (
            "still-findable",
            GroupStopError::Probe(rustix::io::Errno::PERM),
            None,
            Ok(pid),
            false,
        ),
        (
            "lookup-permission",
            GroupStopError::Probe(rustix::io::Errno::PERM),
            None,
            Err(rustix::io::Errno::PERM),
            false,
        ),
        (
            "other-lookup",
            GroupStopError::Probe(rustix::io::Errno::PERM),
            None,
            Err(rustix::io::Errno::INVAL),
            false,
        ),
        (
            "already-reaped",
            GroupStopError::Probe(rustix::io::Errno::PERM),
            Some(exited),
            Err(rustix::io::Errno::SRCH),
            false,
        ),
        (
            "other-probe",
            GroupStopError::Probe(rustix::io::Errno::ACCESS),
            None,
            Err(rustix::io::Errno::SRCH),
            false,
        ),
        (
            "ownership-disqualified",
            GroupStopError::NotOwnedChild,
            None,
            Err(rustix::io::Errno::SRCH),
            false,
        ),
        (
            "wait-echild",
            GroupStopError::Wait(rustix::io::Errno::CHILD),
            None,
            Err(rustix::io::Errno::SRCH),
            false,
        ),
        (
            "signal-permission",
            GroupStopError::Signal(rustix::io::Errno::PERM),
            None,
            Err(rustix::io::Errno::SRCH),
            false,
        ),
    ] {
        if hold_owned_exit_observation(&error, exit, lookup) != expected {
            return Err(format!("exact observation decision differed: {name}").into());
        }
    }
    Ok(())
}
#[test]
fn deadline_preserves_typed_error_and_completion_requires_all_fences() -> TestResult {
    if !matches!(
        failure_at_deadline(Some(GroupStopError::Probe(rustix::io::Errno::PERM))),
        ImageError::Process(GroupStopError::Probe(rustix::io::Errno::PERM))
    ) || !matches!(failure_at_deadline(None), ImageError::WarmupTimedOut)
    {
        return Err("deadline rewrote uncertainty or reset timeout".into());
    }
    let fingerprint = ComponentFingerprint::from_bytes(&[1; 32])?;
    let info = BuildInfo {
        package_version: "1.2.3".parse()?,
        fingerprints: ComponentFingerprints {
            keeper: fingerprint,
            agent_collaboration_services: fingerprint,
            agent_proxy_services: fingerprint,
            agent_provider_services: fingerprint,
        },
    };
    let exit = std::process::ExitStatus::from_raw(0);
    let empty = GroupStopProgress::GroupEmpty {
        result: GroupStopResult::Graceful,
    };
    let held = GroupStopError::Probe(rustix::io::Errno::PERM);
    if !completion_fence_observed(Some(&info), None, Some(exit), &empty)
        || completion_fence_observed(None, None, Some(exit), &empty)
        || completion_fence_observed(Some(&info), Some(&held), Some(exit), &empty)
        || completion_fence_observed(Some(&info), None, None, &empty)
        || completion_fence_observed(Some(&info), None, Some(exit), &GroupStopProgress::Running)
    {
        return Err("completion accepted unresolved or missing evidence".into());
    }
    // The outer warmup owner still rejects a recorded nonzero exit; this helper
    // observes termination only, rather than changing that existing error boundary.
    Ok(())
}
