use super::*;

const SHORT_QUOTA_WAIT_MIN_JITTER_SECONDS: u64 = 60;

const SHORT_QUOTA_WAIT_MAX_JITTER_SECONDS: u64 = 120;

#[cfg(debug_assertions)]
const TEST_SHORT_QUOTA_WAIT_JITTER_ENV: &str = "CODEX_ROUTER_TEST_SHORT_QUOTA_WAIT_JITTER_SECONDS";

pub(super) fn short_quota_wait_delay_seconds(
    accounts: &[BurnDownAccountInput],
    now_unix_seconds: u64,
) -> Option<u64> {
    short_quota_wait_delay_seconds_with_jitter(
        accounts,
        now_unix_seconds,
        short_quota_wait_jitter_seconds(),
    )
}

pub(super) fn short_quota_wait_jitter_seconds() -> u64 {
    #[cfg(debug_assertions)]
    if let Some(jitter_seconds) = bounded_positive_test_jitter(
        std::env::var(TEST_SHORT_QUOTA_WAIT_JITTER_ENV)
            .ok()
            .as_deref(),
    ) {
        return jitter_seconds;
    }
    let jitter_range = SHORT_QUOTA_WAIT_MAX_JITTER_SECONDS
        .saturating_sub(SHORT_QUOTA_WAIT_MIN_JITTER_SECONDS)
        .saturating_add(1);
    let subsecond_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| u64::from(duration.subsec_nanos()));
    SHORT_QUOTA_WAIT_MIN_JITTER_SECONDS.saturating_add(subsecond_nanos % jitter_range)
}

#[cfg(debug_assertions)]
pub(super) fn bounded_positive_test_jitter(value: Option<&str>) -> Option<u64> {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=SHORT_QUOTA_WAIT_MAX_JITTER_SECONDS).contains(value))
}

pub(super) fn short_quota_wait_delay_seconds_with_jitter(
    accounts: &[BurnDownAccountInput],
    now_unix_seconds: u64,
    jitter_seconds: u64,
) -> Option<u64> {
    let mut earliest_short_reset_unix_seconds: Option<u64> = None;
    let mut routable_account_count = 0_u64;

    for account in accounts {
        if !account.routing_enabled() {
            continue;
        }
        routable_account_count = routable_account_count.saturating_add(1);

        let Some(short_window) = account
            .windows()
            .iter()
            .find(|window| window.window_seconds() == V1_SHORT_WINDOW_SECONDS)
        else {
            continue;
        };
        let Some(weekly_window) = account
            .windows()
            .iter()
            .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        else {
            continue;
        };

        if short_window.status() != QuotaWindowStatus::Ineligible
            || short_window.remaining_headroom() != 0
            || weekly_window.status() != QuotaWindowStatus::Eligible
            || weekly_window.remaining_headroom() == 0
        {
            continue;
        }

        let Some(short_reset_unix_seconds) = short_window.reset_unix_seconds() else {
            continue;
        };
        if short_reset_unix_seconds <= now_unix_seconds {
            continue;
        }
        earliest_short_reset_unix_seconds = Some(
            earliest_short_reset_unix_seconds.map_or(short_reset_unix_seconds, |earliest| {
                earliest.min(short_reset_unix_seconds)
            }),
        );
    }

    if routable_account_count == 0 {
        return None;
    }

    earliest_short_reset_unix_seconds.map(|short_reset_unix_seconds| {
        short_reset_unix_seconds
            .saturating_sub(now_unix_seconds)
            .saturating_add(jitter_seconds)
    })
}

pub(super) fn exhausted_account_short_quota_wait_delay_seconds(
    account: &BurnDownAccountInput,
    now_unix_seconds: u64,
    jitter_seconds: u64,
) -> Option<u64> {
    if !account.routing_enabled() {
        return None;
    }
    let short_window = account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == V1_SHORT_WINDOW_SECONDS)?;
    let weekly_window = account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)?;
    if weekly_window.status() != QuotaWindowStatus::Eligible
        || weekly_window.remaining_headroom() == 0
    {
        return None;
    }
    let short_reset_unix_seconds = short_window.reset_unix_seconds()?;
    if short_reset_unix_seconds <= now_unix_seconds {
        return None;
    }
    Some(
        short_reset_unix_seconds
            .saturating_sub(now_unix_seconds)
            .saturating_add(jitter_seconds),
    )
}
