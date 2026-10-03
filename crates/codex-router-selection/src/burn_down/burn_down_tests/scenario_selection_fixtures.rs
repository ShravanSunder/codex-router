//! Shared mutating account-selection scenarios.

use super::*;

pub(super) struct AccountSelectionScenario {
    pub(super) id: &'static str,
    pub(super) starts_to_simulate: usize,
    pub(super) accounts: Vec<ScenarioAccountFixture>,
    pub(super) expected_sequence: Vec<&'static str>,
    pub(super) expected_final_active_sessions: Vec<(&'static str, u32)>,
    pub(super) expected_final_account_states: Vec<ExpectedAccountState>,
    pub(super) expected_selected_weekly_runouts: Option<Vec<(&'static str, Option<u64>)>>,
}

pub(super) struct ScenarioAccountFixture {
    pub(super) account_id: &'static str,
    pub(super) initial_active_sessions: u32,
    pub(super) build_account: fn(u32) -> BurnDownAccountInput,
}

pub(super) struct ExpectedAccountState {
    pub(super) account_id: &'static str,
    pub(super) availability: AccountAvailability,
    pub(super) routing_reason: RoutingReason,
}

#[derive(Debug)]
pub(super) struct ScenarioRunResult {
    pub(super) selected_accounts: Vec<String>,
    pub(super) selected_routing_reasons: Vec<RoutingReason>,
    pub(super) selected_weekly_runouts: Vec<(String, Option<u64>)>,
    pub(super) final_active_sessions: Vec<(String, u32)>,
    pub(super) final_assessment: BurnDownRouteBandAssessmentResult,
}

pub(super) fn run_account_selection_scenario(
    scenario: &AccountSelectionScenario,
) -> ScenarioRunResult {
    let mut active_sessions_by_account = scenario
        .accounts
        .iter()
        .map(|account| (account.account_id, account.initial_active_sessions))
        .collect::<Vec<_>>();
    let mut selected_accounts = Vec::new();
    let mut selected_routing_reasons = Vec::new();
    let mut selected_weekly_runouts = Vec::new();

    for _session_start in 0..scenario.starts_to_simulate {
        let assessment = assess_scenario_accounts(scenario, active_sessions_by_account.as_slice());
        let selected_account = assessment
            .preferred_next()
            .unwrap_or_else(|| panic!("{} should have a quota candidate", scenario.id))
            .as_str()
            .to_owned();
        let selected_weekly_runout = account_assessment(&assessment, &selected_account)
            .weekly_projected_exhaustion_unix_seconds()
            .map(|projected_exhaustion_unix_seconds| {
                projected_exhaustion_unix_seconds.saturating_sub(NOW)
            });
        selected_routing_reasons
            .push(account_assessment(&assessment, &selected_account).routing_reason());
        let (_, selected_active_sessions) = active_sessions_by_account
            .iter_mut()
            .find(|(account_id, _)| *account_id == selected_account)
            .unwrap_or_else(|| {
                panic!(
                    "{} selected account outside fixture: {}",
                    scenario.id, selected_account
                )
            });
        *selected_active_sessions += 1;
        selected_weekly_runouts.push((selected_account.clone(), selected_weekly_runout));
        selected_accounts.push(selected_account);
    }

    let final_assessment =
        assess_scenario_accounts(scenario, active_sessions_by_account.as_slice());
    let final_active_sessions = active_sessions_by_account
        .into_iter()
        .map(|(account_id, active_sessions)| (account_id.to_owned(), active_sessions))
        .collect::<Vec<_>>();

    ScenarioRunResult {
        selected_accounts,
        selected_routing_reasons,
        selected_weekly_runouts,
        final_active_sessions,
        final_assessment,
    }
}

pub(super) fn assess_scenario_accounts(
    scenario: &AccountSelectionScenario,
    active_sessions_by_account: &[(&'static str, u32)],
) -> BurnDownRouteBandAssessmentResult {
    assess_route_band(input(
        scenario
            .accounts
            .iter()
            .map(|account| {
                let active_sessions = active_sessions_by_account
                    .iter()
                    .find(|(account_id, _)| *account_id == account.account_id)
                    .map(|(_, active_sessions)| *active_sessions)
                    .unwrap_or_else(|| {
                        panic!(
                            "{} fixture missing active counter for {}",
                            scenario.id, account.account_id
                        )
                    });
                (account.build_account)(active_sessions)
            })
            .collect(),
    ))
}

pub(super) fn assert_account_selection_scenario(
    scenario: &AccountSelectionScenario,
) -> ScenarioRunResult {
    let result = run_account_selection_scenario(scenario);

    assert_eq!(
        result.selected_accounts,
        scenario
            .expected_sequence
            .iter()
            .map(|account_id| (*account_id).to_owned())
            .collect::<Vec<_>>(),
        "{} selected sequence; selected reasons={:?}; projected runouts={:?}",
        scenario.id,
        result.selected_routing_reasons,
        result.selected_weekly_runouts,
    );
    assert_eq!(
        result.final_active_sessions,
        scenario
            .expected_final_active_sessions
            .iter()
            .map(|(account_id, active_sessions)| ((*account_id).to_owned(), *active_sessions))
            .collect::<Vec<_>>(),
        "{} final active sessions",
        scenario.id
    );
    for expected_state in &scenario.expected_final_account_states {
        let account = account_assessment(&result.final_assessment, expected_state.account_id);
        assert_eq!(
            account.availability(),
            expected_state.availability,
            "{} final availability for {}",
            scenario.id,
            expected_state.account_id
        );
        assert_eq!(
            account.routing_reason(),
            expected_state.routing_reason,
            "{} final routing reason for {}",
            scenario.id,
            expected_state.account_id
        );
    }
    if let Some(expected_selected_weekly_runouts) = &scenario.expected_selected_weekly_runouts {
        assert_eq!(
            result.selected_weekly_runouts,
            expected_selected_weekly_runouts
                .iter()
                .map(|(account_id, runout)| ((*account_id).to_owned(), *runout))
                .collect::<Vec<_>>(),
            "{} selected weekly projection trace",
            scenario.id
        );
    }

    result
}
