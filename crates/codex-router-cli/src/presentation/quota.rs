//! Quota status terminal presentation.

mod quota_browse_rendering;
mod quota_reset_detail_content;
mod quota_reset_detail_rendering;
mod quota_reset_keyboard_interaction;
mod quota_reset_presentation_model;
mod quota_status_component;
mod quota_status_entrypoint;
mod quota_status_view_model;
mod responsive_quota_layout;

#[cfg(test)]
mod quota_browse_presentation_test;
#[cfg(test)]
mod quota_reset_presentation_test;

pub(crate) use quota_status_entrypoint::run_quota_status_view;
pub(crate) use quota_status_entrypoint::write_quota_status_view;
pub(crate) use quota_status_view_model::QuotaSelectedAccountViewModel;
pub(crate) use quota_status_view_model::QuotaStatusAccountViewModel;
pub(crate) use quota_status_view_model::QuotaStatusViewModel;
pub(crate) use quota_status_view_model::QuotaStatusViewModelLoader;
pub(crate) use quota_status_view_model::ResetPaceMeterSegments;
pub(crate) use quota_status_view_model::ResetPaceState;
pub(crate) use quota_status_view_model::ResetPaceViewModel;
pub(crate) use quota_status_view_model::SampleConfidence;
pub(crate) use quota_status_view_model::SampleMetadata;
