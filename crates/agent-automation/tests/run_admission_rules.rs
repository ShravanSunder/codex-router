use agent_automation::RunPhase;

#[test]
fn unresolved_execution_and_required_summary_block_successor_admission() {
    // Arrange: every phase whose native work or required summary is unresolved.
    let occupying = [
        RunPhase::Preparing,
        RunPhase::Executing,
        RunPhase::Stopping,
        RunPhase::SummaryRequired,
        RunPhase::SummaryRunning,
        RunPhase::SummaryBlocked,
        RunPhase::Uncertain,
    ];
    // Act / Assert: no successor may treat these Runs as finished.
    for phase in occupying {
        assert!(
            phase.occupies_execution(),
            "{phase:?} must retain occupancy"
        );
    }
}

#[test]
fn waiting_and_safely_finished_work_do_not_occupy_execution() {
    // Arrange: waiting work has not been admitted; terminal work has resolved.
    let nonoccupying = [
        RunPhase::Waiting,
        RunPhase::Finished,
        RunPhase::PreparationFailed,
    ];
    // Act / Assert: another Run can be considered for admission.
    for phase in nonoccupying {
        assert!(
            !phase.occupies_execution(),
            "{phase:?} must not occupy execution"
        );
    }
}
