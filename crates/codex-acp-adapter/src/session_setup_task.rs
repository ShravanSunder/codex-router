//! Native setup runs outside frontend routing; the registry publishes the returned binding.
use crate::{AcpSchemaCatalog, AcpSessionBinding, SessionSetupError, SessionSetupInputs};
use codex_native_integration::{NativePayloadSchemas, NativeProtocolConnection};
use communication_protocol::CodexGeneration;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub(crate) struct SetupTaskInputs {
    pub known_session: Option<AcpSessionBinding>,
    pub backend_path: PathBuf,
    pub schemas: Arc<NativePayloadSchemas>,
    pub generation: CodexGeneration,
    pub params: Value,
    pub create_new: bool,
    pub cancellation_barrier: Option<crate::CancellationBarrier>,
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
    let outcome = async {
        let connection = NativeProtocolConnection::connect(&inputs.backend_path)
            .await
            .map_err(|_| SessionSetupError::Unavailable)?;
        let setup = SessionSetupInputs {
            connection,
            schemas: inputs.schemas,
            generation: inputs.generation,
            params: inputs.params,
        };
        if inputs.create_new {
            let session = AcpSessionBinding::create(&mut catalog, setup).await?;
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
        Err(error) => SetupTaskOutput {
            binding: None,
            outcome: Err(error),
        },
    }
}
