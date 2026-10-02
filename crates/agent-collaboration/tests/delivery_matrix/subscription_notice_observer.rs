//! Recipient-observed subscription line and stored activity range expansion.
use super::{ProofContext, ProofResult, SUBSCRIPTION_NOTICE_LABEL};
use collaboration_client::protocol::{
    PushKind, PushRecordShowParams, PushRecordShowResult, SessionRef,
};
use serde_json::json;

pub(crate) async fn verify_subscription_notice(
    proof: &mut ProofContext,
    target: &SessionRef,
    notice: &str,
    body_marker: &str,
) -> ProofResult<PushRecordShowResult> {
    if !notice.starts_with(SUBSCRIPTION_NOTICE_LABEL)
        || notice.lines().count() != 1
        || notice.contains(body_marker)
        || notice.contains("Self-declared sender:")
    {
        return Err("Subscription input was not a body-free neutral line".into());
    }
    let reference = notice
        .rsplit(" · ")
        .next()
        .ok_or("subscription link missing")?;
    let link = collaboration_client::protocol::RouterLink::parse(reference)?;
    let shown = proof
        .client
        .router_show(PushRecordShowParams {
            caller: target.clone(),
            reference: link.to_string(),
        })
        .await?;
    if shown.record.kind != PushKind::SubscriptionActivity
        || shown.record.target != *target
        || !shown
            .activity_ranges
            .iter()
            .flat_map(|range| &range.messages)
            .any(|message| message.text.as_str() == body_marker)
    {
        return Err("Subscription link did not expand the target's posted activity".into());
    }
    proof.record("subscriptionNotice", json!({"target":target,"pushId":shown.record.push_id,"line":notice,"rangeCount":shown.activity_ranges.len()}))?;
    Ok(shown)
}
