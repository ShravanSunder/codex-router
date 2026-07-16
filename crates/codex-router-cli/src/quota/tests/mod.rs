pub(super) use codex_router_core::ids::AccountId;
use codex_router_selection::burn_down::RoutingReason;

use super::*;

const NOW: u64 = 1_700_000_000;

mod command;
mod refresh_provider;
mod selection_projection;
mod status_formatting;
mod status_metrics;
mod status_pace;
mod support;

use support::*;
