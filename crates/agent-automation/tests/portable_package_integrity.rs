use agent_automation::{
    ExecutionDestination, InstructionId, InstructionText, PortableInstruction,
    PortableSchedulePackage, ScheduleDefinition, ScheduleId, TimingRule, decode_schedule_package,
    encode_schedule_package,
};
#[test]
fn package_round_trip_preserves_identity_but_strips_live_destination()
-> Result<(), Box<dyn std::error::Error>> {
    let instruction_id = InstructionId::generate();
    let package = PortableSchedulePackage::<String, String> {
        schedule_id: ScheduleId::generate(),
        source_change_id: "source-change".into(),
        instruction: PortableInstruction {
            instruction_id: instruction_id.clone(),
            text: InstructionText::try_from("Check build".to_owned())?,
            source_revision_id: "source-revision".into(),
        },
        definition: ScheduleDefinition {
            instruction_id,
            timing: TimingRule::Interval { seconds: 600 },
            enabled: true,
            destination: ExecutionDestination::OwnedThread {
                target: "live-native-thread".into(),
                cwd: "/private/source-workspace".into(),
            },
            execution_timeout_seconds: Some(120),
        },
        continuity: None,
    };
    let text = encode_schedule_package(&package, 1_048_576)?;
    if text.contains("live-native-thread") || text.contains("/private/source-workspace") {
        return Err("live destination leaked into reusable package".into());
    }
    let decoded: PortableSchedulePackage<String, String> =
        decode_schedule_package(&text, 1_048_576)?;
    if decoded.schedule_id != package.schedule_id
        || decoded.definition.enabled
        || !matches!(
            decoded.definition.destination,
            ExecutionDestination::Unprepared
        )
    {
        return Err("portable identity/disabled boundary changed".into());
    }
    let altered = text.replace("Check build", "Check something else");
    if decode_schedule_package::<String, String>(&altered, 1_048_576).is_ok() {
        return Err("damaged package passed footer verification".into());
    }
    if decode_schedule_package::<String, String>(text.trim_end(), 1_048_576).is_ok() {
        return Err("truncated final newline was accepted".into());
    }
    Ok(())
}
