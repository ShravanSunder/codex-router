//! Check a delivered native push against its public receipt and stored input.
use super::native_push_correlation::{
    ExpectedNativePushCorrelation, validate_native_push_correlation,
};
use super::public_push_receipt::wait_for_public_receipt;
use super::{ProofContext, ProofResult};
use collaboration_protocol::{
    DeliveryClientReceipt, NativeSendAcceptance, PushRecord, PushRecordShowParams,
};
use serde_json::{Value, json};

pub(super) async fn record_push_input_correlation(
    proof: &mut ProofContext,
    producer: &str,
    record: &PushRecord,
    user_message: &Value,
    deadline: tokio::time::Instant,
) -> ProofResult<bool> {
    // The initial show predates native delivery; observe its settled receipt only
    // after thread/read has established that this exact input arrived.
    let settled = wait_for_public_receipt(
        async || {
            let current = proof
                .client
                .router_show(PushRecordShowParams {
                    caller: record.target.clone(),
                    reference: record.push_id.as_str().to_owned(),
                })
                .await?;
            if current.record.push_id != record.push_id || current.record.target != record.target {
                return Err(format!("{producer} settled push identity changed").into());
            }
            Ok(current.record)
        },
        deadline,
    )
    .await?;
    let record = &settled;
    let receipt = record
        .last_outcome
        .as_ref()
        .and_then(|outcome| outcome.client.as_ref());
    let Some(DeliveryClientReceipt::CodexAppServer(receipt)) = receipt else {
        return Err(format!("{producer} omitted native delivery acceptance").into());
    };
    if receipt.target != record.target
        || receipt.generation != proof.generation
        || String::from(receipt.client_user_message_id.clone()) != record.push_id.as_str()
    {
        return Err(
            format!("{producer} native receipt target/generation/correlation mismatch").into(),
        );
    }
    let turn_id = match &receipt.acceptance {
        NativeSendAcceptance::NativeInputAccepted { turn_id, .. }
        | NativeSendAcceptance::SteerAccepted { turn_id, .. } => String::from(turn_id.clone()),
        NativeSendAcceptance::QueueAccepted { .. } => {
            return Err("S6 requires a native accepted turn".into());
        }
    };
    let observed_id = user_message
        .get("id")
        .and_then(Value::as_str)
        .ok_or("native userMessage omitted its generated identity")?;
    if user_message.get("clientId").and_then(Value::as_str) != Some(record.push_id.as_str()) {
        return Err(format!("{producer} native userMessage clientId did not match PushId").into());
    }
    let turns = tokio::time::timeout_at(deadline, proof.turns(&record.target)).await??;
    validate_native_push_correlation(
        &turns,
        ExpectedNativePushCorrelation {
            push_id: record.push_id.as_str(),
            accepted_turn_id: &turn_id,
            native_item_id: observed_id,
        },
    )?;
    proof.record(
        "pushInputCorrelation",
        json!({
            "producer":producer,"pushId":record.push_id,"observedUserMessageId":observed_id,
            "observedClientId":record.push_id,"acceptedTurnId":turn_id,
            "target":record.target,"generation":receipt.generation,"status":"asserted"
        }),
    )?;
    Ok(true)
}
