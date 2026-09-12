//! Opt-in live board proof against an already running isolated debug Host.
#[path = "board_live_support/mod.rs"]
mod board_live_support;
#[allow(dead_code)]
#[path = "automation_live_support/proof_context.rs"]
mod proof_context;

use proof_context::ProofResult;

#[tokio::test]
#[ignore = "requires an explicitly launched isolated debug Host and two fresh Luna threads"]
async fn two_luna_agents_exchange_a_verified_finding_through_the_board_cli() -> ProofResult<()> {
    board_live_support::exercise().await
}
