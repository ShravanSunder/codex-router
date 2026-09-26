//! Native setup runs outside frontend routing; the registry publishes the returned binding.
use crate::{
    AcpSchemaCatalog, AcpSessionBinding, ConversationOperationRecorder, SessionSetupError,
    SessionSetupInputs,
};
use codex_native_integration::{NativePayloadSchemas, NativeProtocolConnection};
use collaboration_protocol::{CodexGeneration, OperationId, SessionId};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub(crate) struct SetupTaskInputs {
    pub known_session: Option<AcpSessionBinding>,
    pub adopt_unmaterialized: bool,
    pub backend_path: PathBuf,
    pub schemas: Arc<NativePayloadSchemas>,
    pub generation: CodexGeneration,
    pub params: Value,
    pub create_new: bool,
    pub operation_id: Option<OperationId>,
    pub recorder: Arc<dyn ConversationOperationRecorder>,
    pub cancellation_barrier: Option<crate::CancellationBarrier>,
    pub approval_broker: Arc<dyn crate::ApprovalBroker>,
}
pub(crate) struct SetupTaskOutput {
    pub binding: Option<AcpSessionBinding>,
    pub outcome: Result<(Vec<Value>, Value), SessionSetupError>,
}
pub(crate) async fn run_session_setup(inputs: SetupTaskInputs) -> SetupTaskOutput {
    let mut catalog = match AcpSchemaCatalog::load() {
        Ok(catalog) => catalog,
        Err(_) => {
            return SetupTaskOutput {
                binding: inputs.known_session,
                outcome: Err(SessionSetupError::SchemaUnavailable),
            };
        }
    };
    if let Some(mut session) = inputs.known_session {
        if inputs.adopt_unmaterialized {
            let outcome = session
                .adopt_unmaterialized(&mut catalog, &inputs.generation, &inputs.params)
                .map(|()| (Vec::new(), json!({})));
            return SetupTaskOutput {
                binding: Some(session),
                outcome,
            };
        }
        let outcome = match session
            .resume_with_receipt(&mut catalog, &inputs.generation, &inputs.params)
            .await
        {
            Ok(response)
                if inputs
                    .cancellation_barrier
                    .as_ref()
                    .is_some_and(|barrier| !barrier.resolved_by_resume(&response)) =>
            {
                Err(SessionSetupError::CancellationUnresolved)
            }
            Ok(response) => crate::project_history(&mut catalog, session.session_id(), &response)
                .map(|history| (history, json!({})))
                .map_err(|_| SessionSetupError::OutcomeUnknown),
            Err(error) => Err(error),
        };
        let detached = matches!(
            &outcome,
            Err(SessionSetupError::OutcomeUnknown | SessionSetupError::Unavailable)
        );
        return SetupTaskOutput {
            binding: if detached { None } else { Some(session) },
            outcome,
        };
    }
    let operation_id = inputs.operation_id.clone();
    if inputs.create_new {
        let Some(operation_id) = operation_id.as_ref() else {
            return SetupTaskOutput {
                binding: None,
                outcome: Err(SessionSetupError::InvalidParameters),
            };
        };
        if inputs
            .recorder
            .admit_create(operation_id, &inputs.generation)
            .await
            .is_err()
        {
            return SetupTaskOutput {
                binding: None,
                outcome: Err(SessionSetupError::Unavailable),
            };
        }
    }
    let outcome = async {
        let connection = NativeProtocolConnection::connect(&inputs.backend_path)
            .await
            .map_err(|_| SessionSetupError::Unavailable)?;
        let setup = SessionSetupInputs {
            connection,
            schemas: inputs.schemas,
            generation: inputs.generation,
            params: inputs.params,
            approval_broker: inputs.approval_broker,
            operation_id: operation_id.clone(),
            recorder: Arc::clone(&inputs.recorder),
        };
        if inputs.create_new {
            let session = AcpSessionBinding::create(&mut catalog, setup).await?;
            let session_id = SessionId::try_from(session.session_id().to_owned())
                .map_err(|_| SessionSetupError::OutcomeUnknown)?;
            if let Some(operation_id) = operation_id.as_ref() {
                inputs
                    .recorder
                    .record_created(operation_id, &session_id)
                    .await
                    .map_err(|_| SessionSetupError::OutcomeUnknown)?;
            }
            let result = session.new_session_result();
            Ok((session, Vec::new(), result))
        } else {
            let (session, history) = AcpSessionBinding::load_existing_guarded(
                &mut catalog,
                setup,
                inputs.cancellation_barrier.as_ref(),
            )
            .await?;
            Ok((session, history, json!({})))
        }
    }
    .await;
    match outcome {
        Ok((session, history, result)) => SetupTaskOutput {
            binding: Some(session),
            outcome: Ok((history, result)),
        },
        Err(error) => {
            let known_not_submitted = matches!(
                error,
                SessionSetupError::InvalidParameters
                    | SessionSetupError::ConfigurationMismatch
                    | SessionSetupError::ModelMismatch { .. }
                    | SessionSetupError::AccessMismatch { .. }
                    | SessionSetupError::NativeRejected
                    | SessionSetupError::SchemaUnavailable
            );
            let error = if let Some(operation_id) = operation_id.as_ref() {
                if inputs
                    .recorder
                    .record_failure(operation_id, known_not_submitted)
                    .await
                    .is_err()
                {
                    SessionSetupError::OutcomeUnknown
                } else {
                    error
                }
            } else {
                error
            };
            SetupTaskOutput {
                binding: None,
                outcome: Err(error),
            }
        }
    }
}
