use super::*;
use std::time::Duration;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn exact_normal_and_forced_boundaries_and_no_early_kill() -> TestResult {
    let start = Instant::now();
    for (timing, term, observe) in [
        (
            GroupStopTiming::normal(),
            Duration::from_secs(1),
            Duration::from_secs(2),
        ),
        (
            GroupStopTiming::forced_handover(),
            Duration::from_millis(150),
            Duration::from_millis(100),
        ),
    ] {
        if timing.term_grace != term || timing.kill_observe != observe {
            return Err("production bounds differ from literal contract".into());
        }
        let state = GroupStopProgress::TermSent { at: start, timing };
        if state.action(start + term - Duration::from_nanos(1))? != StopAction::Poll
            || state.action(start + term)? != StopAction::SendKill
        {
            return Err("TERM grace boundary changed".into());
        }
        let killed = GroupStopProgress::KillSent { at: start, timing };
        if killed.action(start + observe - Duration::from_nanos(1))? != StopAction::Poll
            || killed.action(start + observe)? != StopAction::ObservationExpired
        {
            return Err("kill observation boundary changed".into());
        }
        if state.action(start - Duration::from_nanos(1)).is_ok() {
            return Err("backwards clock accepted".into());
        }
    }
    Ok(())
}
#[test]
fn forced_expiry_is_not_group_empty_and_later_empty_keeps_actual_kill_result() -> TestResult {
    // Stand-in: uncommon delayed-empty observation after actual kill, not runtime proof.
    let start = Instant::now();
    let timing = GroupStopTiming::forced_handover();
    let mut state = GroupStopProgress::TimedOutStillRunning {
        kill_at: start,
        timing,
    };
    if state.status() != GroupStopStatus::ForcedKillObservationExpired {
        return Err("forced expiry became retirement fence".into());
    }
    state.note_empty();
    if state.status()
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Killed,
        })
    {
        return Err("ESRCH did not independently complete group".into());
    }
    Ok(())
}
#[test]
fn normal_timeout_retains_owned_group_and_cannot_resend_kill() -> TestResult {
    let start = Instant::now();
    let timing = GroupStopTiming::normal();
    let mut state = GroupStopProgress::TimedOutStillRunning {
        kill_at: start,
        timing,
    };
    if state.status() != GroupStopStatus::TimedOutStillRunning
        || state.action(start + Duration::from_secs(20))? != StopAction::Poll
    {
        return Err("timed out state forgot or resent stop".into());
    }
    state.note_empty();
    if state.status()
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Killed,
        })
    {
        return Err("later empty lost kill outcome".into());
    }
    Ok(())
}
#[test]
fn fixture_bounds_are_positive_and_cannot_widen_production_profiles() {
    for profile in [GroupStopProfile::Normal, GroupStopProfile::ForcedHandover] {
        assert!(
            GroupStopTiming::shortened(profile, Duration::ZERO, Duration::from_millis(1)).is_err()
        );
        assert!(
            GroupStopTiming::shortened(profile, Duration::from_secs(2), Duration::from_secs(3))
                .is_err()
        );
    }
}
