//! Parse subscription scopes and partial policies before opening the Control client.
use super::board_preparation::{
    self, ActorInput, CommandContext, PreparedBoardCommand, placeholder_identity,
};
use super::board_subscription_arguments::{
    SubscriptionModeKind, ThreadSubscribeArguments, ThreadSubscriptionScopeArguments,
    ThreadSubscriptionsArguments, ThreadUnsubscribeArguments, WhenIdleKind,
};
use super::board_value_parsing::{command_context, parse_uuid_v7};
use collaboration_client::board::{
    BatchTiming, MAX_SUBSCRIPTION_CAP_SECONDS, MAX_SUBSCRIPTION_QUIET_SECONDS,
    SubscriptionLifetime, SubscriptionMode, SubscriptionPolicyPatch, SubscriptionScope,
    SubscriptionTimingPatch, WhenIdle,
};
use collaboration_client::protocol::{
    ThreadSubscribeRequest, ThreadSubscriptionsRequest, ThreadUnsubscribeRequest,
};

pub(super) struct PendingSubscriptionActor<TRequest> {
    pub request: TRequest,
    pub actor: ActorInput,
}

pub(super) fn prepare_subscribe(
    arguments: ThreadSubscribeArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    require_json(arguments.common.json, "Thread Subscribe")?;
    let actor = board_preparation::parse_actor_input(&arguments.actor)?;
    let scope = subscription_scope(arguments.scope)?;
    let quiet_seconds = arguments
        .quiet
        .as_deref()
        .map(|value| parse_subscription_duration(value, "--quiet", true))
        .transpose()?;
    let cap_seconds = arguments
        .cap
        .as_deref()
        .map(|value| parse_subscription_duration(value, "--cap", true))
        .transpose()?;
    if quiet_seconds.is_some_and(|seconds| seconds > MAX_SUBSCRIPTION_QUIET_SECONDS) {
        return Err("--quiet must be at most 30 minutes; for example --quiet 2m".into());
    }
    if cap_seconds.is_some_and(|seconds| seconds > MAX_SUBSCRIPTION_CAP_SECONDS) {
        return Err("--cap must be at most 60 minutes; for example --cap 10m".into());
    }
    if let (Some(quiet_seconds), Some(cap_seconds)) = (quiet_seconds, cap_seconds) {
        BatchTiming::new(quiet_seconds, cap_seconds).map_err(|_| {
            "--cap must be at least --quiet and at most 60 minutes; for example --quiet 2m --cap 10m"
                .to_owned()
        })?;
    }
    let lifetime_seconds = arguments
        .for_duration
        .as_deref()
        .map(|value| parse_subscription_duration(value, "--for", false))
        .transpose()?;
    if let Some(seconds) = lifetime_seconds {
        SubscriptionLifetime::new(seconds).map_err(|_| {
            "--for must be between 10 minutes and 7 days; for example --for 24h".to_owned()
        })?;
    }
    let request = ThreadSubscribeRequest {
        actor: placeholder_actor(&actor)?,
        scope,
        policy: SubscriptionPolicyPatch {
            mode: arguments.mode.map(subscription_mode),
            when_idle: arguments.when_idle.map(when_idle),
            timing: SubscriptionTimingPatch {
                quiet_seconds,
                cap_seconds,
            },
            lifetime_seconds,
        },
    };
    Ok((
        PreparedBoardCommand::ThreadSubscribe(PendingSubscriptionActor { request, actor }),
        command_context(arguments.common),
    ))
}

pub(super) fn prepare_unsubscribe(
    arguments: ThreadUnsubscribeArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    require_json(arguments.common.json, "Thread Unsubscribe")?;
    let actor = board_preparation::parse_actor_input(&arguments.actor)?;
    let request = ThreadUnsubscribeRequest {
        actor: placeholder_actor(&actor)?,
        scope: subscription_scope(arguments.scope)?,
    };
    Ok((
        PreparedBoardCommand::ThreadUnsubscribe(PendingSubscriptionActor { request, actor }),
        command_context(arguments.common),
    ))
}

pub(super) fn prepare_subscriptions(
    arguments: ThreadSubscriptionsArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    require_json(arguments.common.json, "Thread Subscriptions")?;
    let actor = board_preparation::parse_actor_input(&arguments.actor)?;
    let request = ThreadSubscriptionsRequest {
        actor: placeholder_actor(&actor)?,
    };
    Ok((
        PreparedBoardCommand::ThreadSubscriptions(PendingSubscriptionActor { request, actor }),
        command_context(arguments.common),
    ))
}

pub(super) fn subscription_mode(value: SubscriptionModeKind) -> SubscriptionMode {
    match value {
        SubscriptionModeKind::Deliver => SubscriptionMode::Deliver,
        SubscriptionModeKind::Poll => SubscriptionMode::Poll,
        SubscriptionModeKind::Off => SubscriptionMode::Off,
    }
}

pub(super) fn when_idle(value: WhenIdleKind) -> WhenIdle {
    match value {
        WhenIdleKind::Hold => WhenIdle::Hold,
        WhenIdleKind::Wake => WhenIdle::Wake,
        WhenIdleKind::Drop => WhenIdle::Drop,
    }
}

fn subscription_scope(
    arguments: ThreadSubscriptionScopeArguments,
) -> Result<SubscriptionScope, String> {
    match (arguments.root_message_id, arguments.topic_id) {
        (Some(root_message_id), None) => Ok(SubscriptionScope::thread(parse_uuid_v7(
            root_message_id,
            "--root-message-id",
        )?)),
        (None, Some(topic_id)) => Ok(SubscriptionScope::topic(parse_uuid_v7(
            topic_id,
            "--topic-id",
        )?)),
        _ => Err("Choose exactly one subscription scope: --root-message-id or --topic-id".into()),
    }
}

fn placeholder_actor(actor: &ActorInput) -> Result<collaboration_client::board::Identity, String> {
    match actor {
        ActorInput::Explicit(identity) => Ok(identity.clone()),
        ActorInput::Self_ => placeholder_identity(),
    }
}

fn require_json(enabled: bool, command: &str) -> Result<(), String> {
    if enabled {
        Ok(())
    } else {
        Err(format!("{command} requires --json"))
    }
}

fn parse_subscription_duration(value: &str, flag: &str, allow_zero: bool) -> Result<u64, String> {
    let example = match flag {
        "--quiet" => "--quiet 2m",
        "--cap" => "--cap 10m",
        "--for" => "--for 24h",
        _ => "--for 24h",
    };
    let error =
        || format!("{flag} requires an integer followed by s, m, h, or d; for example {example}");
    let (number, multiplier) = if let Some(number) = value.strip_suffix('s') {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 60 * 60)
    } else if let Some(number) = value.strip_suffix('d') {
        (number, 24 * 60 * 60)
    } else {
        return Err(error());
    };
    let seconds = number
        .parse::<u64>()
        .map_err(|_| error())?
        .checked_mul(multiplier)
        .ok_or_else(error)?;
    if !allow_zero && seconds == 0 {
        return Err(format!(
            "{flag} must be greater than zero; for example {example}"
        ));
    }
    Ok(seconds)
}

#[cfg(test)]
mod tests {
    use super::parse_subscription_duration;

    #[test]
    fn subscription_duration_errors_show_flag_specific_examples() {
        let invalid_duration_cases: [(&str, &str, bool, &str); 3] = [
            ("--quiet", "not-a-duration", true, "--quiet 2m"),
            ("--cap", "not-a-duration", true, "--cap 10m"),
            ("--for", "not-a-duration", false, "--for 24h"),
        ];

        for (flag, value, allow_zero, example) in invalid_duration_cases {
            let Err(error) = parse_subscription_duration(value, flag, allow_zero) else {
                panic!("expected an error for {flag} {value}");
            };
            assert!(error.contains(&format!("for example {example}")), "{error}");
        }

        let Err(positive_duration_error) = parse_subscription_duration("0s", "--for", false) else {
            panic!("expected --for to reject zero duration");
        };
        assert!(positive_duration_error.contains("--for must be greater than zero"));
        assert!(positive_duration_error.contains("for example --for 24h"));
    }
}
