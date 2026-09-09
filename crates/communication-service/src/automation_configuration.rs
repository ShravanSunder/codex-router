//! Host supplies persistence; service workers share only the current admitted configuration.
use communication_protocol::{
    AutomationConfiguration, AutomationConfigureRequest, ConfigurationFailure,
};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::RwLock;
#[derive(Clone)]
pub struct AutomationConfigurationHandle {
    state: Arc<RwLock<ConfigurationState>>,
}
struct ConfigurationState {
    value: AutomationConfiguration,
    ready: bool,
}
impl Default for AutomationConfigurationHandle {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(ConfigurationState {
                value: AutomationConfiguration::default(),
                ready: true,
            })),
        }
    }
}
pub struct ConfigurationAdmissionLease {
    state: tokio::sync::OwnedRwLockReadGuard<ConfigurationState>,
}
impl ConfigurationAdmissionLease {
    pub fn configuration(&self) -> Option<AutomationConfiguration> {
        self.state.ready.then_some(self.state.value)
    }
}
impl AutomationConfigurationHandle {
    pub async fn admission_lease(&self) -> ConfigurationAdmissionLease {
        ConfigurationAdmissionLease {
            state: Arc::clone(&self.state).read_owned().await,
        }
    }
    pub async fn current(&self) -> Option<AutomationConfiguration> {
        let state = self.state.read().await;
        state.ready.then_some(state.value)
    }
    pub async fn suspend(&self) {
        self.state.write().await.ready = false;
    }
    pub async fn publish(&self, value: AutomationConfiguration) {
        let mut state = self.state.write().await;
        state.value = value;
        state.ready = true;
    }
}
pub trait AutomationConfigurationBackend: Send + Sync {
    fn configure(
        &self,
        request: AutomationConfigureRequest,
    ) -> Pin<
        Box<dyn Future<Output = Result<AutomationConfiguration, ConfigurationFailure>> + Send + '_>,
    >;
}
