//! Codex route classification.

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
    /// WebSocket upgrade on `/v1/responses`.
    ResponsesWebSocket,
    /// `GET /v1/models`.
    Models,
    /// `POST /v1/memories/trace_summarize`.
    MemoriesTraceSummarize,
    /// `POST /v1/responses/compact`.
    ResponsesCompact,
}

impl RouteKind {
    /// Returns the shared quota route band for this route.
    #[must_use]
    pub const fn route_band(self) -> RouteBand {
        match self {
            Self::Responses | Self::ResponsesWebSocket => RouteBand::Responses,
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
        _ => RouteClass::Rejected {
            reason: "unsupported_path",
        },
    }
}
