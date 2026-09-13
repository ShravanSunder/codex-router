//! Opt-in unscripted developer-experience evaluation for the board communication skill.
#[path = "board_skill_dx_support/mod.rs"]
mod board_skill_dx_support;
#[allow(dead_code)]
#[path = "automation_live_support/proof_context.rs"]
mod proof_context;

use proof_context::ProofResult;

#[tokio::test]
#[ignore = "requires an explicitly launched isolated debug Host and two fresh Luna skill sessions"]
async fn luna_agents_use_board_skill_without_supplied_commands_or_resource_ids() -> ProofResult<()>
{
    board_skill_dx_support::exercise().await
}
