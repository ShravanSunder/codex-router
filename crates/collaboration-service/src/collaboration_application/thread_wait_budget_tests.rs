//! A thread-wait notice's roots are cut to the bound `thread_wait_root_notice_limit` computes,
//! so the largest possible notice still fits the response limit: one maximal root fits, and
//! the bound leaves no room for a second when the budget has room for exactly one.
use super::{
    API_RESULT_BUDGET, RESULT_LIMIT_BYTES, ResultByteBudget, ThreadWaitBudgetError,
    thread_wait_root_notice_limit,
};
use chrono::{DateTime, Utc};
use collaboration_protocol::{
    MAX_PUSH_LINE_BYTES, MessageText, PushId, SubscriptionWaitBatch, ThreadSubscriptionWaitResult,
};
use message_board::{ActivitySequence, MessageId, PendingRootNotice, TopicId};
use std::error::Error;

fn maximal_root() -> Result<PendingRootNotice, Box<dyn Error>> {
    let maximum_activity_sequence = u64::try_from(i64::MAX)?;
    Ok(PendingRootNotice::new(
        MessageId::generate(),
        TopicId::generate(),
        ActivitySequence::try_from(maximum_activity_sequence)?,
        ActivitySequence::try_from(maximum_activity_sequence)?,
        u64::MAX,
    )?)
}

/// The largest notice a wait can hand out, carrying `roots`.
fn largest_notice(
    roots: Vec<PendingRootNotice>,
) -> Result<ThreadSubscriptionWaitResult, Box<dyn Error>> {
    Ok(ThreadSubscriptionWaitResult {
        batch: Some(SubscriptionWaitBatch::Notice {
            push_id: PushId::try_from("01890f2e-7b4c-7cc0-98c4-000000000002".to_owned())?,
            line: MessageText::try_from("\\".repeat(MAX_PUSH_LINE_BYTES))?,
            held: true,
            held_since: Some(
                DateTime::<Utc>::from_timestamp(253_402_300_799, 999_999_999)
                    .ok_or("valid maximum RFC 3339 timestamp")?,
            ),
            draining: true,
            roots,
        }),
    })
}

fn root() -> PendingRootNotice {
    maximal_root().expect("maximal root notice")
}

fn notice(roots: Vec<PendingRootNotice>) -> ThreadSubscriptionWaitResult {
    largest_notice(roots).expect("largest notice")
}

#[test]
fn thread_wait_root_notice_limit_fits_one_maximal_root_but_not_two() {
    // Arrange: an envelope that leaves the roots exactly one maximal root plus one byte.
    let one_root_bytes = serde_json::to_vec(&[root()]).expect("root encodes").len();
    let unenveloped_limit =
        thread_wait_root_notice_limit(ResultByteBudget::new(RESULT_LIMIT_BYTES, 0))
            .expect("the unenveloped budget has room");
    let envelope_bytes = unenveloped_limit - one_root_bytes - 1;
    let budget = ResultByteBudget::new(RESULT_LIMIT_BYTES, envelope_bytes);

    // Act
    let limit = thread_wait_root_notice_limit(budget).expect("room for one root");
    let one_root = notice(vec![root()]);
    let two_roots = notice(vec![root(), root()]);
    let no_room = thread_wait_root_notice_limit(ResultByteBudget::new(
        RESULT_LIMIT_BYTES,
        envelope_bytes + 2,
    ));

    // Assert
    assert_eq!(limit, one_root_bytes + 1);
    assert!(
        budget.admits(&one_root),
        "the largest one-root notice must fit"
    );
    assert!(!budget.admits(&two_roots), "two maximal roots must not fit");
    assert_eq!(no_room, Err(ThreadWaitBudgetError::NoRoomForOneRoot));
}

#[test]
fn the_api_bound_admits_every_notice_whose_roots_stay_within_it() {
    // Arrange: as many maximal roots as the API bound allows. Every maximal root encodes to
    // the same length, so `count` roots encode to `count * (root + comma) + 1` bytes (two
    // brackets, one comma fewer than roots) and the largest count is computed, not searched.
    let limit = thread_wait_root_notice_limit(API_RESULT_BUDGET).expect("API budget has room");
    let root_bytes = serde_json::to_vec(&root()).expect("root encodes").len();
    let largest_count = (limit - 1) / (root_bytes + 1);
    let mut roots: Vec<_> = (0..=largest_count).map(|_| root()).collect();
    let one_too_many_bytes = serde_json::to_vec(&roots).expect("roots encode").len();
    roots.pop();
    let largest_bytes = serde_json::to_vec(&roots).expect("roots encode").len();

    // Act
    let admitted = API_RESULT_BUDGET.admits(&notice(roots));

    // Assert
    assert!(largest_count > 0, "the API bound holds at least one root");
    assert!(
        largest_bytes <= limit,
        "{largest_count} roots take {largest_bytes} bytes, over the {limit}-byte bound"
    );
    assert!(
        one_too_many_bytes > limit,
        "one more root still fits, so the count is not the largest"
    );
    assert!(admitted, "the largest notice within the bound must fit");
}
