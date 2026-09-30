//! Host-internal typed prompt, steer, and queue admission.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProviderQueueAdmissionError {
    #[error("provider session not found")]
    SessionNotFound,
    #[error("provider session is unavailable")]
    Unavailable,
    #[error("provider session is live elsewhere")]
    LiveElsewhere,
    #[error("session settings are unresolved")]
    SettingsUnresolved,
    #[error("provider prompt content is empty")]
    EmptyContent,
    #[error("provider does not accept {content_type} content")]
    UnsupportedContent { content_type: &'static str },
    #[error("provider queue is full")]
    Busy,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderSteerContentsError {
    #[error(transparent)]
    Admission(#[from] ProviderQueueAdmissionError),
    #[error(transparent)]
    Runtime(#[from] crate::ExternalProviderRuntimeError),
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderPromptContentsError {
    #[error(transparent)]
    Admission(#[from] ProviderQueueAdmissionError),
    #[error("provider prompt operation failed")]
    Operation(Box<collaboration_protocol::ConversationOperationFailure>),
}

impl ProviderAcpDeliveryRoute {
    pub async fn prompt_contents(
        &self,
        target: SessionRef,
        actor: ProviderIdentity,
        input_id: session_event_model::InputId,
        contents: Vec<session_event_model::PromptContent>,
    ) -> Result<collaboration_protocol::ConversationOperationSubmission, ProviderPromptContentsError>
    {
        if contents.is_empty() {
            return Err(ProviderQueueAdmissionError::EmptyContent.into());
        }
        if !self.serves(&target) {
            return Err(ProviderQueueAdmissionError::SessionNotFound.into());
        }
        let session_lock = self
            .session_lock(&target)
            .map_err(|_| ProviderQueueAdmissionError::Unavailable)?;
        let _guard = session_lock.lock().await;
        match self.claim.claim(&target).await {
            RouteClaim::Holds | RouteClaim::CanLoad => {}
            RouteClaim::LiveElsewhere { .. } => {
                return Err(ProviderQueueAdmissionError::LiveElsewhere.into());
            }
            RouteClaim::NotMine => return Err(ProviderQueueAdmissionError::SessionNotFound.into()),
            RouteClaim::Unavailable { .. } => {
                return Err(ProviderQueueAdmissionError::Unavailable.into());
            }
        }
        let record = self
            .store
            .lock()
            .await
            .session_record(&target)
            .await
            .map_err(|_| ProviderQueueAdmissionError::Unavailable)?
            .ok_or(ProviderQueueAdmissionError::SessionNotFound)?;
        match ensure_provider_session_loaded(
            &self.supervisor,
            &self.store,
            self.ownership.as_ref(),
            &target,
            LoadPolicy::MayLoad,
        )
        .await
        {
            ProviderSessionLoadOutcome::Ready | ProviderSessionLoadOutcome::AlreadyLoaded => {}
            ProviderSessionLoadOutcome::NotLoaded => {
                return Err(ProviderQueueAdmissionError::Unavailable.into());
            }
            ProviderSessionLoadOutcome::MissingRecord => {
                return Err(ProviderQueueAdmissionError::SessionNotFound.into());
            }
            ProviderSessionLoadOutcome::LiveElsewhere => {
                return Err(ProviderQueueAdmissionError::LiveElsewhere.into());
            }
            _ => return Err(ProviderQueueAdmissionError::Unavailable.into()),
        }
        let runtime = self
            .supervisor
            .runtime_for(&target.endpoint)
            .ok_or(ProviderQueueAdmissionError::Unavailable)?;
        if runtime
            .settings_unresolved(&String::from(target.session_id.clone()))
            .await
        {
            return Err(ProviderQueueAdmissionError::SettingsUnresolved.into());
        }
        let capabilities = runtime
            .capability_report(&String::from(target.session_id.clone()))
            .await;
        if let Some(content_type) = unsupported_content_type(&contents, &capabilities) {
            return Err(ProviderQueueAdmissionError::UnsupportedContent { content_type }.into());
        }
        self.supervisor
            .prompt_contents(ProviderPromptContentsRequest {
                operation_id: OperationId::generate(),
                input_id,
                target,
                requested_by: actor,
                approver: record.approver,
                contents,
            })
            .await
            .map_err(|failure| ProviderPromptContentsError::Operation(Box::new(failure)))
    }

    pub async fn steer_contents(
        &self,
        target: SessionRef,
        actor: ProviderIdentity,
        input_id: session_event_model::InputId,
        contents: Vec<session_event_model::PromptContent>,
    ) -> Result<ProviderSteeringOutcome, ProviderSteerContentsError> {
        if contents.is_empty() {
            return Err(ProviderQueueAdmissionError::EmptyContent.into());
        }
        if !self.serves(&target) {
            return Err(ProviderQueueAdmissionError::SessionNotFound.into());
        }
        let session_lock = self
            .session_lock(&target)
            .map_err(|_| ProviderQueueAdmissionError::Unavailable)?;
        let _guard = session_lock.lock().await;
        match self.claim.claim(&target).await {
            RouteClaim::Holds | RouteClaim::CanLoad => {}
            RouteClaim::LiveElsewhere { .. } => {
                return Err(ProviderQueueAdmissionError::LiveElsewhere.into());
            }
            RouteClaim::NotMine => return Err(ProviderQueueAdmissionError::SessionNotFound.into()),
            RouteClaim::Unavailable { .. } => {
                return Err(ProviderQueueAdmissionError::Unavailable.into());
            }
        }
        let _record = self
            .store
            .lock()
            .await
            .session_record(&target)
            .await
            .map_err(|_| ProviderQueueAdmissionError::Unavailable)?
            .ok_or(ProviderQueueAdmissionError::SessionNotFound)?;
        let runtime = self
            .supervisor
            .runtime_for(&target.endpoint)
            .ok_or(ProviderQueueAdmissionError::Unavailable)?;
        if runtime
            .settings_unresolved(&String::from(target.session_id.clone()))
            .await
        {
            return Err(ProviderQueueAdmissionError::SettingsUnresolved.into());
        }
        let capabilities = runtime
            .capability_report(&String::from(target.session_id.clone()))
            .await;
        if let Some(content_type) = unsupported_content_type(&contents, &capabilities) {
            return Err(ProviderQueueAdmissionError::UnsupportedContent { content_type }.into());
        }
        let outcome = runtime
            .steer_session_contents_with_input(
                String::from(target.session_id.clone()),
                input_id.clone(),
                contents,
            )
            .await?;
        tracing::info!(
            ?target,
            ?actor,
            ?input_id,
            ?outcome,
            "provider content steer settled"
        );
        Ok(outcome)
    }

    pub async fn queue_contents(
        &self,
        target: SessionRef,
        actor: ProviderIdentity,
        contents: Vec<session_event_model::PromptContent>,
    ) -> Result<crate::ProviderQueuedInput, ProviderQueueAdmissionError> {
        if contents.is_empty() {
            return Err(ProviderQueueAdmissionError::EmptyContent);
        }
        if !self.serves(&target) {
            return Err(ProviderQueueAdmissionError::SessionNotFound);
        }
        let session_lock = self
            .session_lock(&target)
            .map_err(|_| ProviderQueueAdmissionError::Unavailable)?;
        let _guard = session_lock.lock().await;
        match self.claim.claim(&target).await {
            RouteClaim::Holds | RouteClaim::CanLoad => {}
            RouteClaim::LiveElsewhere { .. } => {
                return Err(ProviderQueueAdmissionError::LiveElsewhere);
            }
            RouteClaim::NotMine => return Err(ProviderQueueAdmissionError::SessionNotFound),
            RouteClaim::Unavailable { .. } => return Err(ProviderQueueAdmissionError::Unavailable),
        }
        let record = self
            .store
            .lock()
            .await
            .session_record(&target)
            .await
            .map_err(|_| ProviderQueueAdmissionError::Unavailable)?
            .ok_or(ProviderQueueAdmissionError::SessionNotFound)?;
        let runtime = self
            .supervisor
            .runtime_for(&target.endpoint)
            .ok_or(ProviderQueueAdmissionError::Unavailable)?;
        if runtime
            .settings_unresolved(&String::from(target.session_id.clone()))
            .await
        {
            return Err(ProviderQueueAdmissionError::SettingsUnresolved);
        }
        let capabilities = runtime
            .capability_report(&String::from(target.session_id.clone()))
            .await;
        if let Some(content_type) = unsupported_content_type(&contents, &capabilities) {
            return Err(ProviderQueueAdmissionError::UnsupportedContent { content_type });
        }
        let binding = self
            .supervisor
            .binding(&target.endpoint)
            .ok_or(ProviderQueueAdmissionError::Unavailable)?;
        let permit = self
            .queue
            .reserve(&target)
            .map_err(|_| ProviderQueueAdmissionError::Busy)?;
        let operation_id = OperationId::generate();
        let input_id = session_event_model::InputId::generate();
        let queued = self
            .supervisor
            .queued_operation_registry()
            .record_queued_contents(
                operation_id.clone(),
                target.clone(),
                binding,
                input_id.clone(),
                &contents,
            );
        permit.send(ProviderQueuedPrompt::Contents {
            request: ProviderPromptContentsRequest {
                operation_id,
                input_id,
                target,
                requested_by: actor,
                approver: record.approver,
                contents,
            },
            load_policy: LoadPolicy::MayLoad,
        });
        Ok(queued)
    }
}

fn unsupported_content_type(
    contents: &[session_event_model::PromptContent],
    capabilities: &acp_client_runtime::ProviderCapabilityReport,
) -> Option<&'static str> {
    contents.iter().find_map(|content| match content {
        session_event_model::PromptContent::Text { .. }
        | session_event_model::PromptContent::ResourceLink { .. } => None,
        session_event_model::PromptContent::Image { .. } if !capabilities.accepts_image => {
            Some("image")
        }
        session_event_model::PromptContent::Audio { .. } if !capabilities.accepts_audio => {
            Some("audio")
        }
        session_event_model::PromptContent::EmbeddedResource { .. }
            if !capabilities.accepts_embedded_resource =>
        {
            Some("embeddedResource")
        }
        _ => None,
    })
}
