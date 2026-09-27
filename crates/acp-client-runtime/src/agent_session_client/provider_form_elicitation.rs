//! ACP form elicitation through the canonical Session Question port.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use agent_client_protocol::schema::v1::{
    CreateElicitationRequest, ElicitationMode, ElicitationPropertySchema, ElicitationScope,
    MultiSelectItems,
};
use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use serde_json::{Value, json};
use session_event_model::{
    ChoiceOption, QuestionAnswerValue, QuestionField, QuestionFields, QuestionRequest,
    QuestionResponse,
};
use tokio_util::sync::CancellationToken;

use super::ActiveApprovalContext;
use crate::InteractionPort;

pub(super) struct ProviderFormElicitationHandler<P: InteractionPort> {
    approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
    interaction_port: Arc<P>,
}

impl<P: InteractionPort> ProviderFormElicitationHandler<P> {
    pub(super) fn new(
        approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
        interaction_port: Arc<P>,
    ) -> Self {
        Self {
            approval_contexts,
            interaction_port,
        }
    }

    fn question_from_form(
        session_id: &str,
        response_id: &impl serde::Serialize,
        request: CreateElicitationRequest,
    ) -> Result<QuestionRequest, Error> {
        let ElicitationMode::Form(form) = request.mode else {
            return Err(Error::invalid_params());
        };
        let required = form
            .requested_schema
            .required
            .unwrap_or_default()
            .into_iter()
            .collect::<HashSet<_>>();
        let fields = form
            .requested_schema
            .properties
            .into_iter()
            .map(|(field_id, schema)| {
                let is_required = required.contains(&field_id);
                match schema {
                    ElicitationPropertySchema::String(schema) => {
                        let label = schema.title.unwrap_or_else(|| field_id.clone());
                        let description = schema.description;
                        let titled = schema.one_of.map(|options| {
                            options
                                .into_iter()
                                .map(|option| ChoiceOption {
                                    option_id: option.value,
                                    label: option.title,
                                })
                                .collect::<Vec<_>>()
                        });
                        let plain = schema.enum_values.map(|values| {
                            values
                                .into_iter()
                                .map(|value| ChoiceOption {
                                    option_id: value.clone(),
                                    label: value,
                                })
                                .collect::<Vec<_>>()
                        });
                        if let Some(options) = titled.or(plain) {
                            Ok(QuestionField::SingleChoice {
                                field_id,
                                label,
                                description,
                                required: is_required,
                                options,
                            })
                        } else {
                            Ok(QuestionField::Text {
                                field_id,
                                label,
                                description,
                                required: is_required,
                            })
                        }
                    }
                    ElicitationPropertySchema::Number(schema) => Ok(QuestionField::Number {
                        label: schema.title.unwrap_or_else(|| field_id.clone()),
                        description: schema.description,
                        field_id,
                        required: is_required,
                    }),
                    ElicitationPropertySchema::Integer(schema) => Ok(QuestionField::Number {
                        label: schema.title.unwrap_or_else(|| field_id.clone()),
                        description: schema.description,
                        field_id,
                        required: is_required,
                    }),
                    ElicitationPropertySchema::Boolean(schema) => Ok(QuestionField::Boolean {
                        label: schema.title.unwrap_or_else(|| field_id.clone()),
                        description: schema.description,
                        field_id,
                        required: is_required,
                    }),
                    ElicitationPropertySchema::Array(schema) => {
                        let options = match schema.items {
                            MultiSelectItems::String(items) => items
                                .values
                                .into_iter()
                                .map(|value| ChoiceOption {
                                    option_id: value.clone(),
                                    label: value,
                                })
                                .collect(),
                            MultiSelectItems::Titled(items) => items
                                .options
                                .into_iter()
                                .map(|option| ChoiceOption {
                                    option_id: option.value,
                                    label: option.title,
                                })
                                .collect(),
                            _ => return Err(Error::invalid_params()),
                        };
                        Ok(QuestionField::MultiChoice {
                            label: schema.title.unwrap_or_else(|| field_id.clone()),
                            description: schema.description,
                            field_id,
                            required: is_required,
                            options,
                            min: schema
                                .min_items
                                .and_then(|value| usize::try_from(value).ok()),
                            max: schema
                                .max_items
                                .and_then(|value| usize::try_from(value).ok()),
                        })
                    }
                    _ => Err(Error::invalid_params()),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let fields = QuestionFields::new(fields).map_err(|_| Error::invalid_params())?;
        let request_id = serde_json::to_string(&(session_id, response_id))
            .map_err(|_| Error::internal_error())?;
        Ok(QuestionRequest {
            request_id,
            prompt: request.message,
            fields,
        })
    }
}

fn elicitation_answer(response: QuestionResponse, fields: &QuestionFields) -> Value {
    match response {
        QuestionResponse::Answered { content } if content.is_empty() => json!({"action":"decline"}),
        QuestionResponse::Answered { content } => {
            let mut values = serde_json::Map::new();
            for field in fields.iter() {
                let (field_id, value) = match field {
                    QuestionField::Text { field_id, .. } => (
                        field_id,
                        content.get(field_id).and_then(|value| match value {
                            QuestionAnswerValue::Text(text) => Some(json!(text)),
                            _ => None,
                        }),
                    ),
                    QuestionField::Number { field_id, .. } => (
                        field_id,
                        content.get(field_id).and_then(|value| match value {
                            QuestionAnswerValue::Number(number) => Some(json!(number)),
                            _ => None,
                        }),
                    ),
                    QuestionField::Boolean { field_id, .. } => (
                        field_id,
                        content.get(field_id).and_then(|value| match value {
                            QuestionAnswerValue::Boolean(boolean) => Some(json!(boolean)),
                            _ => None,
                        }),
                    ),
                    QuestionField::SingleChoice { field_id, .. } => (
                        field_id,
                        content.get(field_id).and_then(|value| match value {
                            QuestionAnswerValue::SelectedOptions {
                                selected_option_ids,
                            } if selected_option_ids.len() == 1 => selected_option_ids
                                .first()
                                .map(|option_id| json!(option_id)),
                            _ => None,
                        }),
                    ),
                    QuestionField::MultiChoice { field_id, .. } => (
                        field_id,
                        content.get(field_id).and_then(|value| match value {
                            QuestionAnswerValue::SelectedOptions {
                                selected_option_ids,
                            } => Some(json!(selected_option_ids)),
                            _ => None,
                        }),
                    ),
                };
                if let Some(value) = value {
                    values.insert(field_id.clone(), value);
                }
            }
            if values.len() != content.len() {
                json!({"action":"cancel"})
            } else {
                json!({"action":"accept","content":values})
            }
        }
        QuestionResponse::Declined => json!({"action":"decline"}),
        QuestionResponse::Cancelled => json!({"action":"cancel"}),
    }
}

impl<P: InteractionPort> HandleDispatchFrom<Agent> for ProviderFormElicitationHandler<P> {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, Error> {
        let Dispatch::Request(request, responder) = message else {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        };
        if request.method() != "elicitation/create" {
            return Ok(Handled::No {
                message: Dispatch::Request(request, responder),
                retry: false,
            });
        }
        let request =
            match serde_json::from_value::<CreateElicitationRequest>(request.params().clone()) {
                Ok(request) => request,
                Err(_) => {
                    responder.respond_with_error(Error::invalid_params())?;
                    return Ok(Handled::Yes);
                }
            };
        if !matches!(request.mode, ElicitationMode::Form(_)) {
            responder.respond_with_error(Error::invalid_params())?;
            return Ok(Handled::Yes);
        }
        let ElicitationScope::Session(scope) = request.scope() else {
            responder.respond_with_error(Error::invalid_params())?;
            return Ok(Handled::Yes);
        };
        let session_id = scope.session_id.0.to_string();
        let context = self
            .approval_contexts
            .lock()
            .ok()
            .and_then(|contexts| contexts.get(&session_id).cloned());
        let Some(context) = context else {
            tracing::warn!("ACP elicitation has no active Turn context");
            responder.respond(json!({"action":"cancel"}))?;
            return Ok(Handled::Yes);
        };
        let canonical = match Self::question_from_form(&session_id, responder.id(), request) {
            Ok(question) => question,
            Err(error) => {
                responder.respond_with_error(error)?;
                return Ok(Handled::Yes);
            }
        };
        let fields = canonical.fields.clone();
        let port = Arc::clone(&self.interaction_port);
        let request_cancellation = responder.cancellation();
        connection.spawn(async move {
            let agent_cancellation = CancellationToken::new();
            let answer = port.request_question(
                context.approval,
                canonical,
                context.cancelling,
                agent_cancellation.clone(),
            );
            tokio::pin!(answer);
            let response = tokio::select! {
                biased;
                () = request_cancellation.cancelled() => {
                    agent_cancellation.cancel();
                    answer.await
                }
                response = &mut answer => response,
            };
            responder.respond(elicitation_answer(response, &fields))
        })?;
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderFormElicitationHandler"
    }
}
