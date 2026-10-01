//! Provider route classification.

use codex_router_core::routes::RouteBand;

/// HTTP method used by route classifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    /// GET.
    Get,
    /// POST.
    Post,
    /// Other method.
    Other,
}

/// Supported proxy route kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteKind {
    /// `POST /v1/responses`.
    Responses,
    /// `POST /anthropic/v1/messages`.
    ClaudeMessages,
    /// WebSocket upgrade on `/v1/responses`.
    ResponsesWebSocket,
    /// `GET /v1/models`.
    Models,
    /// `POST /v1/memories/trace_summarize`.
    MemoriesTraceSummarize,
    /// `POST /v1/responses/compact`.
    ResponsesCompact,
    /// `POST /v1/images/generations`.
    ImageGenerations,
    /// `POST /v1/images/edits`.
    ImageEdits,
}

impl RouteKind {
    /// Returns the shared quota route band for this route.
    #[must_use]
    pub const fn route_band(self) -> RouteBand {
        match self {
            Self::Responses
            | Self::ResponsesWebSocket
            | Self::ImageGenerations
            | Self::ImageEdits => RouteBand::Responses,
            Self::ClaudeMessages => RouteBand::ClaudeMessages,
            Self::Models => RouteBand::Models,
            Self::MemoriesTraceSummarize => RouteBand::MemoriesTraceSummarize,
            Self::ResponsesCompact => RouteBand::ResponsesCompact,
        }
    }

    /// Returns whether the route may carry previous-response affinity.
    #[must_use]
    pub const fn previous_response_affinity_capable(self) -> bool {
        matches!(self, Self::Responses | Self::ResponsesWebSocket)
    }
}

/// Route classification result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteClass {
    /// Route is supported.
    Supported(RouteKind),
    /// Route is rejected before selection.
    Rejected {
        /// Static rejection reason for audit.
        reason: &'static str,
    },
}

/// Classifies a Codex request route.
#[must_use]
pub fn classify_route(method: Method, path: &str, websocket_upgrade: bool) -> RouteClass {
    match (method, path, websocket_upgrade) {
        (Method::Post, "/v1/responses", false) => RouteClass::Supported(RouteKind::Responses),
        (Method::Post, "/anthropic/v1/messages", false) => {
            RouteClass::Supported(RouteKind::ClaudeMessages)
        }
        (Method::Post, "/v1/responses", true) => {
            RouteClass::Supported(RouteKind::ResponsesWebSocket)
        }
        (Method::Get, "/v1/models", false) => RouteClass::Supported(RouteKind::Models),
        (Method::Post, "/v1/memories/trace_summarize", false) => {
            RouteClass::Supported(RouteKind::MemoriesTraceSummarize)
        }
        (Method::Post, "/v1/responses/compact", false) => {
            RouteClass::Supported(RouteKind::ResponsesCompact)
        }
        (Method::Post, "/v1/images/generations", false) => {
            RouteClass::Supported(RouteKind::ImageGenerations)
        }
        (Method::Post, "/v1/images/edits", false) => RouteClass::Supported(RouteKind::ImageEdits),
        _ => RouteClass::Rejected {
            reason: "unsupported_path",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::Method;
    use super::RouteClass;
    use super::RouteKind;
    use super::classify_route;
    use codex_router_core::routes::RouteBand;

    #[test]
    fn claude_messages_is_the_only_supported_anthropic_route() {
        assert_eq!(
            classify_route(Method::Post, "/anthropic/v1/messages", false),
            RouteClass::Supported(RouteKind::ClaudeMessages)
        );

        for (method, path, websocket_upgrade) in [
            (Method::Post, "/anthropic/v1/messages/count_tokens", false),
            (Method::Post, "/anthropic/v1/models", false),
            (Method::Get, "/anthropic/v1/messages", false),
            (Method::Post, "/anthropic/v1/messages", true),
        ] {
            assert!(matches!(
                classify_route(method, path, websocket_upgrade),
                RouteClass::Rejected { .. }
            ));
        }
    }

    #[test]
    fn claude_messages_uses_its_own_route_band_without_codex_affinity() {
        let route_kind = RouteKind::ClaudeMessages;

        assert_eq!(route_kind.route_band(), RouteBand::ClaudeMessages);
        assert!(!route_kind.previous_response_affinity_capable());
    }
}
