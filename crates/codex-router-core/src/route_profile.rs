//! Typed policy properties for one provider route.

use crate::provider::Provider;

/// Identity field that keeps a server-side continuation on its original account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HardPinKey {
    /// OpenAI previous-response continuation identity.
    PreviousResponseId,
}

/// How a request continues an earlier conversation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinuationModel {
    /// Router owns the previous response and may require the same account.
    ServerSide { hard_pin: HardPinKey },
    /// The client carries the conversation state in each request.
    ClientCarried,
}

/// Point at which a route may switch its selected account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitchPoint {
    /// An active WebSocket turn completes before the next account is selected.
    TurnBoundary,
    /// Each HTTP request selects independently, subject to pins and response ownership.
    NextRequest,
}

/// Event that renews the route's session pin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinRenewal {
    /// Current Codex selection and forwarded activity triggers renew the pin.
    OnActivity,
    /// The provider edge publishes or renews the pin after a successful response.
    OnSuccess,
}

/// Route's permitted account-attempt strategy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptPolicy {
    /// Replay a complete request across enabled accounts after recognized exhaustion.
    ReplayAcrossEnabled,
    /// Permit at most one recovery attempt after the first account.
    AtMostTwo,
    /// Reconnect the client after a WebSocket account reaches its quota floor.
    Reconnect,
}

/// Quota window observed for a route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowKind {
    /// OpenAI or Claude five-hour quota window.
    FiveHour,
    /// Provider weekly quota window.
    Weekly,
}

impl WindowKind {
    /// Returns the stable storage name for this quota window.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FiveHour => "five_hour",
            Self::Weekly => "weekly",
        }
    }

    /// Parses a stable storage name for a quota window.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "five_hour" => Some(Self::FiveHour),
            "weekly" => Some(Self::Weekly),
            _ => None,
        }
    }
}

/// Selection rule applied to a route's quota window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowRule {
    /// Preserve the existing OpenAI selection behavior across its quota windows.
    LegacyOpenAi,
    /// Move an account to reserve after its used percentage reaches this threshold.
    NearFullReserve { percent: u8 },
    /// Move an account to reserve this many basis points above its weekly floor.
    WeeklyFloor { early_switch_bps: u16 },
}

/// One quota window used by route selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowPolicy {
    /// Window identity.
    pub kind: WindowKind,
    /// Selection behavior applied to this window.
    pub rule: WindowRule,
}

/// Typed selection and attempt behavior for one provider route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteProfile {
    /// Stable route-profile name.
    pub name: &'static str,
    /// Provider whose accounts may serve this route.
    pub provider: Provider,
    /// Conversation continuation behavior.
    pub continuation: ContinuationModel,
    /// Point where account switching may occur.
    pub switch_point: SwitchPoint,
    /// Pin renewal event for this route.
    pub pin_renewal: PinRenewal,
    /// Attempt strategy for this route.
    pub attempt_policy: AttemptPolicy,
    /// Quota windows considered by this route.
    pub windows: &'static [WindowPolicy],
}

const OPENAI_WINDOWS: [WindowPolicy; 1] = [WindowPolicy {
    kind: WindowKind::Weekly,
    rule: WindowRule::LegacyOpenAi,
}];

/// Default percentage of five-hour quota use at which a Claude account becomes reserve.
pub const DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT: u8 = 95;

/// Returns Claude's two selection-window inputs for one configured near-full threshold.
#[must_use]
pub const fn claude_window_policies_for_percent(percent: u8) -> [WindowPolicy; 2] {
    [
        WindowPolicy {
            kind: WindowKind::FiveHour,
            rule: WindowRule::NearFullReserve { percent },
        },
        WindowPolicy {
            kind: WindowKind::Weekly,
            rule: WindowRule::WeeklyFloor {
                early_switch_bps: 300,
            },
        },
    ]
}

pub const CLAUDE_WINDOW_POLICIES: [WindowPolicy; 2] =
    claude_window_policies_for_percent(DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT);

/// Current OpenAI Responses WebSocket routing behavior.
pub const RESPONSES_WEBSOCKET: RouteProfile = RouteProfile {
    name: "responses-websocket",
    provider: Provider::Openai,
    continuation: ContinuationModel::ServerSide {
        hard_pin: HardPinKey::PreviousResponseId,
    },
    switch_point: SwitchPoint::TurnBoundary,
    pin_renewal: PinRenewal::OnActivity,
    attempt_policy: AttemptPolicy::Reconnect,
    windows: &OPENAI_WINDOWS,
};

/// Current OpenAI Responses HTTP routing behavior.
pub const RESPONSES_HTTP: RouteProfile = RouteProfile {
    name: "responses-http",
    provider: Provider::Openai,
    continuation: ContinuationModel::ServerSide {
        hard_pin: HardPinKey::PreviousResponseId,
    },
    switch_point: SwitchPoint::NextRequest,
    pin_renewal: PinRenewal::OnActivity,
    attempt_policy: AttemptPolicy::ReplayAcrossEnabled,
    windows: &OPENAI_WINDOWS,
};

/// Claude Messages routing behavior.
pub const CLAUDE_MESSAGES: RouteProfile = RouteProfile {
    name: "claude-messages",
    provider: Provider::Claude,
    continuation: ContinuationModel::ClientCarried,
    switch_point: SwitchPoint::NextRequest,
    pin_renewal: PinRenewal::OnSuccess,
    attempt_policy: AttemptPolicy::AtMostTwo,
    windows: &CLAUDE_WINDOW_POLICIES,
};

#[cfg(test)]
mod tests {
    use super::AttemptPolicy;
    use super::CLAUDE_MESSAGES;
    use super::ContinuationModel;
    use super::HardPinKey;
    use super::PinRenewal;
    use super::RESPONSES_HTTP;
    use super::RESPONSES_WEBSOCKET;
    use super::SwitchPoint;
    use super::WindowKind;
    use super::WindowPolicy;
    use super::WindowRule;
    use crate::provider::Provider;

    #[test]
    fn openai_websocket_profile_encodes_current_route_behavior() {
        assert_eq!(RESPONSES_WEBSOCKET.name, "responses-websocket");
        assert_eq!(RESPONSES_WEBSOCKET.provider, Provider::Openai);
        assert_eq!(
            RESPONSES_WEBSOCKET.continuation,
            ContinuationModel::ServerSide {
                hard_pin: HardPinKey::PreviousResponseId,
            }
        );
        assert_eq!(RESPONSES_WEBSOCKET.switch_point, SwitchPoint::TurnBoundary);
        assert_eq!(RESPONSES_WEBSOCKET.pin_renewal, PinRenewal::OnActivity);
        assert_eq!(RESPONSES_WEBSOCKET.attempt_policy, AttemptPolicy::Reconnect);
        let Some(window) = RESPONSES_WEBSOCKET.windows.first() else {
            panic!("OpenAI WebSocket profile should include its quota window");
        };
        assert_eq!(window.kind, WindowKind::Weekly);
        assert_eq!(window.rule, WindowRule::LegacyOpenAi);
    }

    #[test]
    fn openai_http_profile_encodes_current_route_behavior() {
        assert_eq!(RESPONSES_HTTP.name, "responses-http");
        assert_eq!(RESPONSES_HTTP.provider, Provider::Openai);
        assert_eq!(
            RESPONSES_HTTP.continuation,
            ContinuationModel::ServerSide {
                hard_pin: HardPinKey::PreviousResponseId,
            }
        );
        assert_eq!(RESPONSES_HTTP.switch_point, SwitchPoint::NextRequest);
        assert_eq!(RESPONSES_HTTP.pin_renewal, PinRenewal::OnActivity);
        assert_eq!(
            RESPONSES_HTTP.attempt_policy,
            AttemptPolicy::ReplayAcrossEnabled
        );
        let Some(window) = RESPONSES_HTTP.windows.first() else {
            panic!("OpenAI HTTP profile should include its quota window");
        };
        assert_eq!(window.kind, WindowKind::Weekly);
        assert_eq!(window.rule, WindowRule::LegacyOpenAi);
    }

    #[test]
    fn claude_messages_profile_encodes_quota_window_policy() {
        assert_eq!(CLAUDE_MESSAGES.provider, Provider::Claude);
        assert_eq!(
            CLAUDE_MESSAGES.continuation,
            ContinuationModel::ClientCarried
        );
        assert_eq!(CLAUDE_MESSAGES.switch_point, SwitchPoint::NextRequest);
        assert_eq!(CLAUDE_MESSAGES.pin_renewal, PinRenewal::OnSuccess);
        assert_eq!(CLAUDE_MESSAGES.attempt_policy, AttemptPolicy::AtMostTwo);
        assert_eq!(
            CLAUDE_MESSAGES.windows,
            &[
                WindowPolicy {
                    kind: WindowKind::FiveHour,
                    rule: WindowRule::NearFullReserve { percent: 95 },
                },
                WindowPolicy {
                    kind: WindowKind::Weekly,
                    rule: WindowRule::WeeklyFloor {
                        early_switch_bps: 300,
                    },
                },
            ]
        );
    }

    #[test]
    fn window_kinds_have_stable_storage_names_and_reject_unknown_values() {
        assert_eq!(WindowKind::FiveHour.as_str(), "five_hour");
        assert_eq!(WindowKind::parse("five_hour"), Some(WindowKind::FiveHour));
        assert_eq!(WindowKind::Weekly.as_str(), "weekly");
        assert_eq!(WindowKind::parse("weekly"), Some(WindowKind::Weekly));
        assert_eq!(WindowKind::parse("future_window"), None);
    }
}
