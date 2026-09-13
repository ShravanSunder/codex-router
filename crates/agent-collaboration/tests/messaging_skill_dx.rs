//! Two-phase, opt-in proof of goal-driven direct messaging through the repo skill and real CLI.
#[path = "messaging_skill_dx_support/mod.rs"]
mod messaging_skill_dx_support;
#[allow(dead_code)]
#[path = "automation_live_support/proof_context.rs"]
mod proof_context;

use proof_context::ProofResult;

#[tokio::test]
#[ignore = "phase one requires an explicitly launched isolated debug Host and one fresh Luna recipient"]
async fn prepare_unloaded_recipient_for_messaging_skill_dx() -> ProofResult<()> {
    messaging_skill_dx_support::prepare_recipient().await
}

#[tokio::test]
#[ignore = "phase two requires the same proof root after its owner restarts the isolated debug Host"]
async fn luna_sender_discovers_unloaded_recipient_and_completes_cli_round_trip() -> ProofResult<()>
{
    messaging_skill_dx_support::exercise_round_trip().await
}
