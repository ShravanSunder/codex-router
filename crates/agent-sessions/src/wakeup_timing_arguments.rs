//! Readable timer flags resolve to the same bounded wire types used by SDK callers.
use clap::Args;
use communication_protocol::{ExpiryRequest, PositiveSeconds, TimingRequest};
#[derive(Args)]
#[group(multiple = true)]
pub(crate) struct WakeTimingArguments {
    /// Fire once at a UTC RFC3339 instant.
    #[arg(long,required_unless_present_any=["after","every","cron"],conflicts_with_all=["after","every","cron"])]
    at: Option<String>,
    /// Fire once after a duration: integer followed by s, m, h or d (for example 10m).
    #[arg(long,conflicts_with_all=["every","cron"])]
    after: Option<String>,
    /// Repeat relative to creation, for example 10m. Pausing does not shift this anchor.
    #[arg(long, conflicts_with = "cron")]
    every: Option<String>,
    /// Five fields: minute hour day month weekday. Requires --timezone.
    #[arg(long, requires = "timezone")]
    cron: Option<String>,
    #[arg(long, requires = "cron")]
    timezone: Option<String>,
    /// Expire after this duration; does not extend across pause/resume.
    #[arg(long = "for", conflicts_with = "until")]
    lifetime: Option<String>,
    /// Exclusive UTC RFC3339 expiry instant.
    #[arg(long)]
    until: Option<String>,
}
impl WakeTimingArguments {
    pub(crate) fn prepare(self) -> Result<(TimingRequest, ExpiryRequest), String> {
        let timing = if let Some(at) = self.at {
            TimingRequest::At {
                at: at
                    .try_into()
                    .map_err(|_| "--at requires a valid UTC RFC3339 timestamp")?,
            }
        } else if let Some(after) = self.after {
            TimingRequest::After {
                seconds: duration(&after)?,
            }
        } else if let Some(every) = self.every {
            TimingRequest::Interval {
                seconds: duration(&every)?,
            }
        } else {
            TimingRequest::Cron {
                expression: self.cron.ok_or("Choose --at, --after, --every or --cron")?,
                timezone: self
                    .timezone
                    .ok_or("--cron requires an explicit --timezone")?,
            }
        };
        let expiry = if let Some(lifetime) = self.lifetime {
            ExpiryRequest::After {
                seconds: duration(&lifetime)?,
            }
        } else if let Some(until) = self.until {
            ExpiryRequest::At {
                at: until
                    .try_into()
                    .map_err(|_| "--until requires a valid UTC RFC3339 timestamp")?,
            }
        } else {
            ExpiryRequest::None
        };
        Ok((timing, expiry))
    }
}
fn duration(text: &str) -> Result<PositiveSeconds, String> {
    let error = || {
        "Use an integer duration with s, m, h or d, between 1 second and 365 days (for example 10m)"
            .to_owned()
    };
    let (number, multiplier) = if let Some(value) = text.strip_suffix('s') {
        (value, 1)
    } else if let Some(value) = text.strip_suffix('m') {
        (value, 60)
    } else if let Some(value) = text.strip_suffix('h') {
        (value, 3600)
    } else if let Some(value) = text.strip_suffix('d') {
        (value, 86400)
    } else {
        return Err(error());
    };
    let value = number
        .parse::<u32>()
        .map_err(|_| error())?
        .checked_mul(multiplier)
        .ok_or_else(error)?;
    value.try_into().map_err(|_| error())
}
