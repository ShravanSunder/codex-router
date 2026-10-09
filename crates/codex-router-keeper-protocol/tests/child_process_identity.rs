use codex_router_keeper_protocol::{ChildPgid, ChildPid};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn unsafe_group_numbers_and_unrepresentable_wire_values_are_rejected() {
    for wire in [
        "null",
        "-1",
        "0",
        "1",
        "2147483648",
        "18446744073709551615",
        "2.5",
        "\"42\"",
    ] {
        assert!(serde_json::from_str::<ChildPid>(wire).is_err());
        assert!(serde_json::from_str::<ChildPgid>(wire).is_err());
    }
}
#[test]
fn leader_group_identity_is_derived_from_validated_pid_and_roundtrips() -> TestResult {
    let pid = ChildPid::new(42)?;
    let group = ChildPgid::of_leader(pid);
    if pid.as_pid().as_raw_pid() != 42
        || group.as_pid().as_raw_pid() != 42
        || serde_json::to_string(&group)? != "42"
        || serde_json::from_str::<ChildPgid>("42")? != group
    {
        return Err("typed leader identity changed".into());
    }
    if ChildPid::new(-1).is_ok() || ChildPid::try_from(u32::MAX).is_ok() {
        return Err("invalid PID constructor accepted".into());
    }
    Ok(())
}
