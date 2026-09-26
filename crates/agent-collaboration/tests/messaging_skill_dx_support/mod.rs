mod inventory_evidence;
mod state;
mod trace_evidence;

use crate::proof_context::{ProofContext, ProofResult};
use collaboration_client::protocol::{
    MessageContent, MessageDelivery, NativeSessionView, OperationId, SessionMessageSendParams,
};
use serde_json::json;
use state::MessagingProofState;
use std::path::{Path, PathBuf};

const PREPARE_TRACE: &str = "messaging-skill-dx-prepare-trace.json";
const ROUND_TRIP_TRACE: &str = "messaging-skill-dx-round-trip-trace.json";
const RECIPIENT_COMPLETION_TEXT: &str = "RECIPIENT_REPLY_ACCEPTED";
const SENDER_COMPLETION_TEXT: &str = "MESSAGING_ROUND_TRIP_COMPLETE";

pub(super) async fn prepare_recipient() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let proof_identity = OperationId::generate();
    let recipient_cwd = proof
        .workspace
        .join(format!("recipient-{}", proof_identity.as_str()));
    let sender_cwd = proof
        .workspace
        .join(format!("sender-{}", proof_identity.as_str()));
    std::fs::create_dir(&recipient_cwd)?;
    let skill_path = copy_current_skill(&recipient_cwd)?;
    let readiness_marker = format!("RECIPIENT_READY_{}", proof_identity.as_str());
    let message_marker = format!("DISCOVERY_MESSAGE_{}", proof_identity.as_str());
    let reply_marker = format!("DISCOVERY_REPLY_{}", proof_identity.as_str());
    let recipient = proof
        .start_messaging_skill_thread("the persistent direct-message recipient", &recipient_cwd)
        .await?;
    let task = format!(
        "Read the communication skill completely at {skill_path}, including its relevant direct-messaging reference. The CLI executable is {cli_path}, and the selected isolated service directory is {service_directory}. The permitted future sender has exact working directory {sender_cwd}. Prepare to receive one later agent message after this owned debug Host is restarted. When that message arrives, use the incoming communication header and the skill to discover your own exact address, then send one CLI reply containing {reply_marker} to the declared sender. Choose the required CLI commands yourself. Do not create sessions or inspect sessions outside this owned test. A successful request is only accepted evidence; after the CLI accepts the reply, output exactly {RECIPIENT_COMPLETION_TEXT}. For this preparation turn, make no messaging request and output exactly {readiness_marker} after reading the skill.",
        sender_cwd = sender_cwd.display(),
        skill_path = skill_path.display(),
        cli_path = env!("CARGO_BIN_EXE_agent-collaboration"),
        service_directory = proof.service_directory.display(),
    );
    let receipt = proof
        .client
        .send_human_input(SessionMessageSendParams {
            target: recipient.clone(),
            generation_guard: Some(proof.generation.clone()),
            message: MessageContent::HumanUser {
                text: task.try_into()?,
            },
            mode: MessageDelivery::Auto,
            correlation: None,
        })
        .await?;
    proof.record("messagingRecipientPreparationAccepted", json!(receipt))?;
    let turns = trace_evidence::wait_for_terminal_with_text(
        &mut proof,
        &recipient,
        Some(&readiness_marker),
    )
    .await?;
    trace_evidence::write_trace(
        &proof.root,
        PREPARE_TRACE,
        json!({
            "phase":"recipientTurnObserved",
            "receipt":receipt,
            "recipient":trace_evidence::trace_value(&recipient, turns.clone()),
        }),
    )?;
    trace_evidence::require_terminal(&turns, "Prepared recipient")?;
    if !trace_evidence::read_skill_and_messaging_reference(&turns) {
        return Err(
            "Recipient readiness lacked successful reads of the skill and messaging reference"
                .into(),
        );
    }
    let stored = inventory_evidence::wait_until_stored(&mut proof, &recipient).await?;
    inventory_evidence::require_single_session_at_cwd(&stored, &recipient_cwd, &recipient)?;
    let prior_host_pid = state::host_pid(&proof.root)?;
    let proof_state = MessagingProofState {
        prior_host_pid,
        recipient: recipient.clone(),
        recipient_cwd: recipient_cwd.to_string_lossy().into_owned(),
        sender_cwd: sender_cwd.to_string_lossy().into_owned(),
        skill_path: skill_path.to_string_lossy().into_owned(),
        cli_path: env!("CARGO_BIN_EXE_agent-collaboration").to_owned(),
        service_directory: proof.service_directory.to_string_lossy().into_owned(),
        readiness_marker,
        message_marker,
        reply_marker,
    };
    state::write(&proof.root, &proof_state)?;
    trace_evidence::write_trace(
        &proof.root,
        PREPARE_TRACE,
        json!({
            "phase":"recipientPrepared",
            "state":proof_state,
            "receipt":receipt,
            "recipient":trace_evidence::trace_value(&recipient, turns),
            "storedSessionsAtPreparation":stored,
        }),
    )?;
    proof.record(
        "messagingRecipientPrepared",
        json!({"target":recipient,"state":state::STATE_FILENAME,"trace":PREPARE_TRACE}),
    )?;
    proof.client.close().await?;
    Ok(())
}

pub(super) async fn exercise_round_trip() -> ProofResult<()> {
    let root = proof_root()?;
    let proof_state = state::read(&root)?;
    let mut proof =
        ProofContext::reconnect_after_owned_host_restart(proof_state.prior_host_pid).await?;
    require_matching_persisted_context(&proof, &proof_state)?;
    let stored_before =
        inventory_evidence::wait_until_stored(&mut proof, &proof_state.recipient).await?;
    let recipient_cwd = PathBuf::from(&proof_state.recipient_cwd).canonicalize()?;
    inventory_evidence::require_single_session_at_cwd(
        &stored_before,
        &recipient_cwd,
        &proof_state.recipient,
    )?;
    let loaded_before = inventory_evidence::list_all(&mut proof, NativeSessionView::Loaded).await?;
    if loaded_before
        .iter()
        .any(|session| session.target == proof_state.recipient)
    {
        return Err("Prepared recipient was loaded before the sender attempted discovery".into());
    }
    proof.record(
        "messagingRecipientStoredButNotLoaded",
        json!({"target":proof_state.recipient,"stored":true,"loaded":false}),
    )?;

    let sender_cwd = PathBuf::from(&proof_state.sender_cwd);
    std::fs::create_dir(&sender_cwd)?;
    let prepared_skill_directory = Path::new(&proof_state.skill_path)
        .parent()
        .ok_or("Prepared skill path omitted its parent directory")?;
    let sender_skill_directory = sender_cwd.join("agent-collaboration-skill");
    copy_directory(prepared_skill_directory, &sender_skill_directory)?;
    let sender_skill_path = sender_skill_directory.join("SKILL.md").canonicalize()?;
    let sender = proof
        .start_messaging_skill_thread("the goal-driven direct-message sender", &sender_cwd)
        .await?;
    let task = format!(
        "Use the communication skill at {skill_path}. The CLI executable is {cli_path}, the selected isolated service directory is {service_directory}, and the intended existing recipient has exact working directory {recipient_cwd}. Discover that existing recipient from session inventory, discover your own full address from the current native thread identity and inventory, and send exactly one CLI agent message containing {message_marker}. Ask the recipient to reply with {reply_marker}. Choose and verify the CLI commands yourself; no address or command has been supplied. Do not create a replacement recipient, use human input, inspect unrelated sessions, or resend. Wait for the actual incoming declared-agent reply rather than treating request acceptance as a reply. After observing that reply, output exactly {SENDER_COMPLETION_TEXT}.",
        skill_path = sender_skill_path.display(),
        cli_path = proof_state.cli_path,
        service_directory = proof_state.service_directory,
        recipient_cwd = proof_state.recipient_cwd,
        message_marker = proof_state.message_marker,
        reply_marker = proof_state.reply_marker,
    );
    let initial_receipt = proof
        .client
        .send_human_input(SessionMessageSendParams {
            target: sender.clone(),
            generation_guard: Some(proof.generation.clone()),
            message: MessageContent::HumanUser {
                text: task.try_into()?,
            },
            mode: MessageDelivery::Auto,
            correlation: None,
        })
        .await?;
    proof.record("messagingSenderGoalAccepted", json!(initial_receipt))?;

    let recipient_turns = trace_evidence::wait_for_terminal_with_text(
        &mut proof,
        &proof_state.recipient,
        Some(RECIPIENT_COMPLETION_TEXT),
    )
    .await?;
    let sender_turns = trace_evidence::wait_for_terminal_with_text(
        &mut proof,
        &sender,
        Some(SENDER_COMPLETION_TEXT),
    )
    .await?;
    trace_evidence::write_trace(
        &proof.root,
        ROUND_TRIP_TRACE,
        json!({
            "phase":"roundTripTurnsObserved",
            "initialSenderReceipt":initial_receipt,
            "sender":trace_evidence::trace_value(&sender, sender_turns.clone()),
            "recipient":trace_evidence::trace_value(&proof_state.recipient, recipient_turns.clone()),
        }),
    )?;
    grade_round_trip(&proof_state, &sender, &sender_turns, &recipient_turns)?;

    let stored_after = inventory_evidence::wait_until_stored(&mut proof, &sender).await?;
    inventory_evidence::require_single_session_at_cwd(
        &stored_after,
        &recipient_cwd,
        &proof_state.recipient,
    )?;
    inventory_evidence::require_single_session_at_cwd(
        &stored_after,
        &sender_cwd.canonicalize()?,
        &sender,
    )?;
    trace_evidence::write_trace(
        &proof.root,
        ROUND_TRIP_TRACE,
        json!({
            "phase":"roundTripVerified",
            "recipientStoredBeforeRestartedUse":true,
            "recipientLoadedBeforeRestartedUse":false,
            "initialSenderReceipt":initial_receipt,
            "sender":trace_evidence::trace_value(&sender, sender_turns),
            "recipient":trace_evidence::trace_value(&proof_state.recipient, recipient_turns),
            "storedSessionsAfterRoundTrip":stored_after,
        }),
    )?;
    proof.record(
        "messagingSkillRoundTripVerified",
        json!({"sender":sender,"recipient":proof_state.recipient,"trace":ROUND_TRIP_TRACE}),
    )?;
    proof.client.close().await?;
    Ok(())
}

fn grade_round_trip(
    proof_state: &MessagingProofState,
    sender: &collaboration_client::protocol::SessionRef,
    sender_turns: &[serde_json::Value],
    recipient_turns: &[serde_json::Value],
) -> ProofResult<()> {
    if *sender == proof_state.recipient {
        return Err("Sender and recipient unexpectedly resolved to the same native session".into());
    }
    trace_evidence::require_terminal(sender_turns, "Sender")?;
    if !trace_evidence::read_skill_and_messaging_reference(sender_turns) {
        return Err("Sender did not demonstrate reading the skill and messaging reference".into());
    }
    trace_evidence::require_terminal(recipient_turns, "Recipient")?;
    if !trace_evidence::used_messaging_cli(sender_turns)
        || !trace_evidence::successful_cli_receipts(sender_turns)
            .iter()
            .any(|receipt| receipt.target == proof_state.recipient)
    {
        return Err(
            "Sender produced no successful real CLI request to the discovered recipient".into(),
        );
    }
    if !trace_evidence::has_incoming_message_followed_by_agent_text(
        recipient_turns,
        sender,
        &proof_state.recipient,
        &proof_state.message_marker,
    ) {
        return Err(
            "Recipient history lacks the actual correctly addressed incoming request".into(),
        );
    }
    if !trace_evidence::used_messaging_cli(recipient_turns)
        || !trace_evidence::successful_cli_receipts(recipient_turns)
            .iter()
            .any(|receipt| receipt.target == *sender)
    {
        return Err("Recipient produced no successful real CLI reply request to the sender".into());
    }
    if !trace_evidence::has_incoming_message_followed_by_agent_text(
        sender_turns,
        &proof_state.recipient,
        sender,
        &proof_state.reply_marker,
    ) {
        return Err(
            "Sender history lacks an actual declared-agent reply followed by sender handling"
                .into(),
        );
    }
    Ok(())
}

fn require_matching_persisted_context(
    proof: &ProofContext,
    proof_state: &MessagingProofState,
) -> ProofResult<()> {
    if proof.service_directory.canonicalize()?
        != PathBuf::from(&proof_state.service_directory).canonicalize()?
        || !Path::new(&proof_state.skill_path).is_file()
        || Path::new(&proof_state.cli_path).canonicalize()?
            != Path::new(env!("CARGO_BIN_EXE_agent-collaboration")).canonicalize()?
    {
        return Err(
            "Persisted messaging context does not match the restarted proof runtime".into(),
        );
    }
    Ok(())
}

fn proof_root() -> ProofResult<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("CODEX_AUTOMATION_PROOF_ROOT")
            .ok_or("Set CODEX_AUTOMATION_PROOF_ROOT to the resumed proof directory")?,
    )
    .canonicalize()?)
}

fn copy_current_skill(workspace: &Path) -> ProofResult<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../agent-skills/agent-collaboration")
        .canonicalize()?;
    let destination = workspace.join("agent-collaboration-skill");
    copy_directory(&source, &destination)?;
    Ok(destination.join("SKILL.md"))
}

fn copy_directory(source: &Path, destination: &Path) -> ProofResult<()> {
    std::fs::create_dir(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &destination_path)?;
        } else {
            std::fs::copy(entry.path(), destination_path)?;
        }
    }
    Ok(())
}
