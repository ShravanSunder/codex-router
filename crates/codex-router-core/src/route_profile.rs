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

#[cfg(test)]
mod tests {
    use super::AttemptPolicy;
    use super::ContinuationModel;
    use super::HardPinKey;
    use super::PinRenewal;
    use super::RESPONSES_HTTP;
    use super::RESPONSES_WEBSOCKET;
    use super::SwitchPoint;
    use super::WindowKind;
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
}
