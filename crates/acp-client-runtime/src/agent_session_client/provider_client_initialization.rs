//! Public initialization entry points for one ACP provider connection.

use super::*;

impl<P: InteractionPort> AgentSessionClient<P> {
    pub async fn initialize(
        launch: ExternalProviderLaunch,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            INITIALIZE_TIMEOUT,
            Vec::new(),
            interaction_port,
            event_sink,
        )
        .await
    }

    pub async fn initialize_with_mcp_http(
        launch: ExternalProviderLaunch,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            INITIALIZE_TIMEOUT,
            vec![McpServer::Http(McpServerHttp::new(server_name, server_url))],
            interaction_port,
            event_sink,
        )
        .await
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub async fn initialize_with_timeout(
        launch: ExternalProviderLaunch,
        initialize_timeout: Duration,
        interaction_port: Arc<P>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, ExternalProviderRuntimeError> {
        Self::initialize_with_timeout_and_mcp_servers(
            launch,
            initialize_timeout,
            Vec::new(),
            interaction_port,
            event_sink,
        )
        .await
    }
}
