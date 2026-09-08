//! Occurrence calculation keeps relative intervals distinct from calendar cron.
use chrono::{DateTime, Duration, Utc};
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum TimingError {
    #[error("timing duration must be between 1 and 31536000 seconds")]
    InvalidDuration,
    #[error("cron expression must contain exactly five valid fields")]
    InvalidCron,
    #[error("cron timezone must be a valid explicit IANA timezone")]
    InvalidTimezone,
    #[error("timing calculation exceeds the supported datetime range")]
    OutOfRange,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TimingRule {
    At {
        at: DateTime<Utc>,
    },
    After {
        seconds: u32,
    },
    Interval {
        seconds: u32,
    },
    Cron {
        expression: String,
        timezone: String,
    },
}

impl TimingRule {
    /// Calculates the first due instant, or the next instant strictly after a handled boundary.
    /// Expiry and pause eligibility belong to the caller's lifecycle transaction.
    pub fn next_due(
        &self,
        anchor: DateTime<Utc>,
        after: Option<DateTime<Utc>>,
    ) -> Result<Option<DateTime<Utc>>, TimingError> {
        match self {
            Self::At { at } => {
                Ok(Some(*at).filter(|due| after.is_none_or(|boundary| *due > boundary)))
            }
            Self::After { seconds } => {
                let seconds = validate_seconds(*seconds)?;
                let due = anchor
                    .checked_add_signed(Duration::seconds(seconds))
                    .ok_or(TimingError::OutOfRange)?;
                Ok(Some(due).filter(|at| after.is_none_or(|boundary| *at > boundary)))
            }
            Self::Interval { seconds } => {
                let seconds = validate_seconds(*seconds)?;
                let elapsed = after.map_or(0, |boundary| {
                    boundary.signed_duration_since(anchor).num_seconds().max(0)
                });
                let steps = (elapsed / seconds)
                    .checked_add(1)
                    .ok_or(TimingError::OutOfRange)?;
                let offset = steps.checked_mul(seconds).ok_or(TimingError::OutOfRange)?;
                anchor
                    .checked_add_signed(Duration::seconds(offset))
                    .map(Some)
                    .ok_or(TimingError::OutOfRange)
            }
            Self::Cron {
                expression,
                timezone,
            } => {
                if expression.split_whitespace().count() != 5 {
                    return Err(TimingError::InvalidCron);
                }
                let timezone: chrono_tz::Tz =
                    timezone.parse().map_err(|_| TimingError::InvalidTimezone)?;
                let cron = CronParser::builder()
                    .seconds(Seconds::Disallowed)
                    .year(Year::Disallowed)
                    .build()
                    .parse(expression)
                    .map_err(|_| TimingError::InvalidCron)?;
                let start = after
                    .map_or(anchor, |boundary| boundary.max(anchor))
                    .with_timezone(&timezone);
                cron.find_next_occurrence(&start, false)
                    .map(|due| Some(due.with_timezone(&Utc)))
                    .map_err(|_| TimingError::OutOfRange)
            }
        }
    }
}

fn validate_seconds(seconds: u32) -> Result<i64, TimingError> {
    if !(1..=31_536_000).contains(&seconds) {
        return Err(TimingError::InvalidDuration);
    }
    Ok(i64::from(seconds))
}
