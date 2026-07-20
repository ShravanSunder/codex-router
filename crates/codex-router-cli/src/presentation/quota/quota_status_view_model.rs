use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_router_core::ids::AccountId;

pub(crate) type QuotaStatusViewModelLoader =
    Arc<dyn Fn() -> QuotaStatusViewModelLoadFuture + Send + Sync>;
pub(crate) type QuotaStatusViewModelLoadFuture =
    Pin<Box<dyn Future<Output = Option<QuotaStatusViewModel>> + Send>>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaStatusViewModel {
    pub(crate) width: usize,
    pub(crate) route_line: String,
    pub(crate) why_line: String,
    pub(crate) serving_clients: Option<u32>,
    pub(crate) rows: Vec<QuotaStatusAccountViewModel>,
    pub(crate) selected: Option<QuotaSelectedAccountViewModel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaStatusAccountViewModel {
    pub(crate) account_id: AccountId,
    pub(crate) account_tag: String,
    pub(crate) active_credential_generation: Option<u64>,
    pub(crate) enabled: bool,
    pub(crate) selected: bool,
    pub(crate) account: String,
    pub(crate) status: String,
    pub(crate) active_clients: String,
    pub(crate) reset_credits: String,
    pub(crate) reason: String,
    pub(crate) weekly_window: String,
    pub(crate) short_window: String,
    pub(crate) burn_meter: String,
    pub(crate) sample_metadata: SampleMetadata,
    pub(crate) reset_pace: ResetPaceViewModel,
    pub(crate) weekly_pace: String,
    pub(crate) weekly_quota_floor_percent: u16,
    pub(crate) details: QuotaSelectedAccountViewModel,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SampleConfidence {
    #[default]
    Unknown,
    Fresh,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SampleMetadata {
    pub(crate) confidence: SampleConfidence,
    pub(crate) age_label: String,
    pub(crate) age_seconds: Option<u64>,
    pub(crate) semantic_label: &'static str,
}

impl Default for SampleMetadata {
    fn default() -> Self {
        Self {
            confidence: SampleConfidence::Unknown,
            age_label: "unknown".to_owned(),
            age_seconds: None,
            semantic_label: "sample unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ResetPaceState {
    UnderBurning,
    #[default]
    Healthy,
    OverBurning,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ResetPaceMeterSegments {
    pub(crate) filled: usize,
    pub(crate) empty: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResetPaceViewModel {
    pub(crate) state: ResetPaceState,
    pub(crate) multiple_label: String,
    pub(crate) impact_label: Option<String>,
    pub(crate) semantic_label: &'static str,
    pub(crate) meter_left_segments: ResetPaceMeterSegments,
    pub(crate) meter_right_segments: ResetPaceMeterSegments,
    pub(crate) center_marker: char,
    pub(crate) unavailable_reason: Option<String>,
}

impl Default for ResetPaceViewModel {
    fn default() -> Self {
        Self {
            state: ResetPaceState::Unavailable,
            multiple_label: "burn unavailable".to_owned(),
            impact_label: None,
            semantic_label: "burn unavailable",
            meter_left_segments: ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            meter_right_segments: ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            center_marker: '│',
            unavailable_reason: Some("reset pace unavailable".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaSelectedAccountViewModel {
    pub(crate) account: String,
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) short_window: String,
    pub(crate) weekly_window: String,
    pub(crate) burn_meter: String,
    pub(crate) burn_pace: String,
    pub(crate) sample_metadata: SampleMetadata,
    pub(crate) reset_pace: ResetPaceViewModel,
    pub(crate) short_reset_pace: ResetPaceViewModel,
    pub(crate) total_rate: String,
    pub(crate) connection_rate: String,
    pub(crate) active_clients: String,
    pub(crate) guards: String,
    pub(crate) reset: String,
    pub(crate) note: String,
}
