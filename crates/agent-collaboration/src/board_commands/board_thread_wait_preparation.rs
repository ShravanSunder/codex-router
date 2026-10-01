//! Parse the subscription wait filter and deadline before opening Control.
use super::board_arguments::ThreadWaitArguments;
use super::board_preparation::{
    self, ActorInput, CommandContext, PreparedBoardCommand, placeholder_identity,
};
use super::board_thread_subscription_preparation::parse_subscription_duration;
use super::board_value_parsing::{command_context, parse_uuid_v7};
use collaboration_client::board::{MessageId, TopicId};
use collaboration_client::protocol::{
    MAX_SUBSCRIPTION_WAIT_SECONDS, ThreadSubscriptionWaitFilter, ThreadSubscriptionWaitRequest,
};
use std::time::Duration;

pub(super) struct PendingSubscriptionWait {
    pub request: ThreadSubscriptionWaitRequest,
    pub actor: ActorInput,
    pub timeout: Duration,
}

pub(super) fn prepare(
    arguments: ThreadWaitArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    if !arguments.common.json {
        return Err("Thread Wait requires --json".into());
    }
    let actor = board_preparation::parse_actor_input(&arguments.actor)?;
    let root_message_ids = arguments
        .root_message_id
        .into_iter()
        .map(|value| parse_uuid_v7::<MessageId>(value, "--root-message-id"))
        .collect::<Result<Vec<_>, _>>()?;
    let filter = match (root_message_ids.is_empty(), arguments.topic_id) {
        (false, None) => {
            let mut unique_roots = Vec::with_capacity(root_message_ids.len());
            for root_message_id in root_message_ids {
                if unique_roots.contains(&root_message_id) {
                    return Err("--root-message-id values must be unique".into());
                }
                unique_roots.push(root_message_id);
            }
            ThreadSubscriptionWaitFilter::Roots {
                root_message_ids: unique_roots,
            }
        }
        (true, Some(topic)) => ThreadSubscriptionWaitFilter::Topic {
            topic_id: parse_uuid_v7::<TopicId>(topic, "--topic-id")?,
        },
        (true, None) => ThreadSubscriptionWaitFilter::All {},
        (false, Some(_)) => {
            return Err("Choose repeated --root-message-id values or one --topic-id".into());
        }
    };
    let max_wait_seconds = parse_subscription_duration(&arguments.max_wait, "--max-wait", true)?;
    if max_wait_seconds > MAX_SUBSCRIPTION_WAIT_SECONDS {
        return Err("--max-wait must be at most 1500 seconds".into());
    }
    let request = ThreadSubscriptionWaitRequest {
        actor: match &actor {
            ActorInput::Explicit(identity) => identity.clone(),
            ActorInput::Self_ => placeholder_identity()?,
        },
        filter,
        max_wait_seconds,
    };
    request.validate().map_err(|error| error.to_string())?;
    let timeout = Duration::from_secs(max_wait_seconds).saturating_add(Duration::from_secs(5));
    Ok((
        PreparedBoardCommand::ThreadSubscriptionWait(PendingSubscriptionWait {
            request,
            actor,
            timeout,
        }),
        command_context(arguments.common),
    ))
}
