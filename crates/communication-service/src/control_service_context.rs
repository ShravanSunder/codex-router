//! Shared service identity and composed dependencies, separate from connection admission.
use crate::EndpointDirectory;
use communication_protocol::{EndpointDescription, UuidIdentity};

#[derive(Clone)]
pub struct ServiceIdentity {
    pub(crate) board: Option<std::sync::Arc<tokio::sync::Mutex<project_board_storage::BoardStore>>>,
    pub(crate) service_id: UuidIdentity,
    pub(crate) configuration: crate::AutomationConfigurationHandle,
    pub(crate) configuration_backend:
        Option<std::sync::Arc<dyn crate::AutomationConfigurationBackend>>,
    pub(crate) service_epoch: UuidIdentity,
    pub(crate) schema_digest: communication_protocol::SchemaDigest,
    pub(crate) directory: EndpointDirectory,
    pub(crate) wake_wait_permits: std::sync::Arc<tokio::sync::Semaphore>,
    pub(crate) journal: Option<std::sync::Arc<lifecycle_observation::LifecycleStore>>,
    pub(crate) native_backend: Option<crate::NativeControlBackend>,
    pub(crate) automation:
        Option<std::sync::Arc<tokio::sync::Mutex<automation_storage::AutomationStore>>>,
}
impl ServiceIdentity {
    pub fn with_board_store(
        mut self,
        store: std::sync::Arc<tokio::sync::Mutex<project_board_storage::BoardStore>>,
    ) -> Self {
        self.board = Some(store);
        self
    }
    pub fn automation_retention_worker(&self) -> Option<crate::AutomationRetentionWorker> {
        self.automation
            .as_ref()
            .map(|store| crate::AutomationRetentionWorker::new(std::sync::Arc::clone(store)))
    }
    pub fn with_automation_configuration(
        mut self,
        handle: crate::AutomationConfigurationHandle,
        backend: std::sync::Arc<dyn crate::AutomationConfigurationBackend>,
    ) -> Self {
        self.configuration = handle;
        self.configuration_backend = Some(backend);
        self
    }

    pub fn schedule_timing_worker(&self) -> Option<crate::ScheduleTimingWorker> {
        self.automation.as_ref().map(|store| {
            crate::ScheduleTimingWorker::new(
                std::sync::Arc::clone(store),
                self.native_backend.clone(),
                self.configuration.clone(),
            )
        })
    }

    pub fn wake_timing_worker(&self) -> Option<crate::WakeTimingWorker> {
        self.automation.as_ref().map(|store| {
            crate::WakeTimingWorker::new(
                std::sync::Arc::clone(store),
                crate::wakeup_native_sender::WakeNativeSender {
                    service_id: self.service_id.clone(),
                    endpoints: self.directory.clone(),
                    backend: self.native_backend.clone(),
                    configuration: self.configuration.clone(),
                },
            )
        })
    }

    pub fn with_automation_store(
        mut self,
        store: std::sync::Arc<tokio::sync::Mutex<automation_storage::AutomationStore>>,
    ) -> Self {
        self.automation = Some(store);
        self
    }

    pub fn with_native_backend(
        mut self,
        backend: crate::NativeControlBackend,
    ) -> Result<Self, String> {
        if backend.endpoint.service_id != self.service_id {
            return Err("native backend belongs to another service".into());
        }
        self.native_backend = Some(backend);
        Ok(self)
    }
    /// Registers a bounded inventory without inferring endpoint liveness.
    pub fn with_endpoints(self, endpoints: Vec<EndpointDescription>) -> Result<Self, String> {
        let mut identities = std::collections::HashSet::new();
        if endpoints.len() > 64 {
            return Err("too many endpoints".into());
        }
        for endpoint in &endpoints {
            if endpoint.endpoint.service_id != self.service_id {
                return Err("endpoint belongs to another service".into());
            }
            if !identities.insert(endpoint.endpoint.clone()) {
                return Err("duplicate endpoint identity".into());
            }
            if !(1..=2).contains(&endpoint.channels.len()) {
                return Err("invalid channel count".into());
            }
        }
        for endpoint in endpoints {
            self.directory
                .publish(endpoint)
                .map_err(|error| error.to_string())?;
        }
        Ok(self)
    }

    #[must_use]
    pub fn endpoint_directory(&self) -> EndpointDirectory {
        self.directory.clone()
    }

    pub fn with_journal(
        mut self,
        journal: std::sync::Arc<lifecycle_observation::LifecycleStore>,
    ) -> Self {
        self.journal = Some(journal);
        self
    }

    pub fn new(service_id: &str, service_epoch: &str, schema_digest: &str) -> Result<Self, String> {
        let digest = communication_protocol::SchemaDigest::try_from(schema_digest.to_owned())
            .map_err(str::to_owned)?;
        Ok(Self {
            configuration: crate::AutomationConfigurationHandle::default(),
            configuration_backend: None,
            service_id: UuidIdentity::try_from(service_id.to_owned())
                .map_err(|error| error.to_string())?,
            service_epoch: UuidIdentity::try_from(service_epoch.to_owned())
                .map_err(|error| error.to_string())?,
            schema_digest: digest,
            journal: None,
            native_backend: None,
            wake_wait_permits: std::sync::Arc::new(tokio::sync::Semaphore::new(16)),
            automation: None,
            board: None,
            directory: EndpointDirectory::new(
                UuidIdentity::try_from(service_id.to_owned()).map_err(|error| error.to_string())?,
            ),
        })
    }
}
