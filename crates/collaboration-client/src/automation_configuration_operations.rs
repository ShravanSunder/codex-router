//! Configure future attempt budgets and inspect service readiness through typed collaboration API calls.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
    AutomationConfiguration, AutomationConfigureRequest, AutomationStatus, ConfigurationFailure,
};
use serde::{Serialize, de::DeserializeOwned};
#[derive(Debug, thiserror::Error)]
pub enum ConfigurationClientError {
    #[error("configuration request rejected: {0:?}")]
    Rejected(Box<ConfigurationFailure>),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl CollaborationClient {
    pub async fn configure_automation(
        &self,
        request: AutomationConfigureRequest,
    ) -> Result<AutomationConfiguration, ConfigurationClientError> {
        self.configuration_call("automation_configure", request)
            .await
    }
    pub async fn automation_status(&self) -> Result<AutomationStatus, ConfigurationClientError> {
        self.configuration_call("automation_status", serde_json::json!({}))
            .await
    }
    async fn configuration_call<TRequest: Serialize, TResponse: DeserializeOwned>(
        &self,
        method: &str,
        request: TRequest,
    ) -> Result<TResponse, ConfigurationClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid configuration request"))?;
        let result = match self.connection.call(method, params).await {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                return Err(ConfigurationClientError::Rejected(
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid configuration error"))?,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid configuration result").into())
    }
}
