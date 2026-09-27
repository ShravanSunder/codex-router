//! Turn Cursor's blocking ask_question extension into a canonical Question.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use serde::Deserialize;
use serde_json::{Value, json};
use session_event_model::{
    ChoiceOption, QuestionAnswerValue, QuestionField, QuestionFields, QuestionRequest,
    QuestionResponse,
};
use tokio_util::sync::CancellationToken;

use super::ActiveApprovalContext;
use crate::{InteractionPort, provider_connection_activity::ProviderConnectionActivity};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorAskQuestion {
    tool_call_id: String,
    title: Option<String>,
    questions: Vec<CursorQuestion>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorQuestion {
    id: String,
    prompt: String,
    options: Vec<CursorChoice>,
    #[serde(default)]
    allow_multiple: bool,
}

#[derive(Deserialize)]
struct CursorChoice {
    id: String,
    label: String,
}

pub(super) struct ProviderCursorQuestionHandler<P: InteractionPort> {
    tool_registry: Arc<ProviderConnectionActivity>,
    approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
    interaction_port: Arc<P>,
}

impl<P: InteractionPort> ProviderCursorQuestionHandler<P> {
    pub(super) fn new(
        tool_registry: Arc<ProviderConnectionActivity>,
        approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
        interaction_port: Arc<P>,
    ) -> Self {
        Self {
            tool_registry,
            approval_contexts,
            interaction_port,
        }
    }

    fn request_from_cursor(
        session_id: &str,
        response_id: &impl serde::Serialize,
        cursor: CursorAskQuestion,
    ) -> Result<QuestionRequest, Error> {
        let fields = cursor
            .questions
            .into_iter()
            .map(|question| {
                let options = question
                    .options
                    .into_iter()
                    .map(|option| ChoiceOption {
                        option_id: option.id,
                        label: option.label,
                    })
                    .collect();
                if question.allow_multiple {
                    QuestionField::MultiChoice {
                        field_id: question.id,
                        label: question.prompt,
                        description: None,
                        required: true,
                        options,
                        min: None,
                        max: None,
                    }
                } else {
                    QuestionField::SingleChoice {
                        field_id: question.id,
                        label: question.prompt,
                        description: None,
                        required: true,
                        options,
                    }
                }
            })
            .collect();
        let fields = QuestionFields::new(fields).map_err(|_| Error::invalid_params())?;
        let request_id = serde_json::to_string(&(session_id, response_id))
            .map_err(|_| Error::internal_error())?;
        let _tool_call_id = cursor.tool_call_id;
        Ok(QuestionRequest {
            request_id,
            prompt: cursor
                .title
                .unwrap_or_else(|| "Answer questions".to_owned()),
            fields,
        })
    }
}

fn cursor_answer(response: QuestionResponse, field_order: &[String]) -> Value {
    match response {
        QuestionResponse::Answered { content } if content.is_empty() => {
            json!({"outcome":{"outcome":"skipped"}})
        }
        QuestionResponse::Answered { content } => {
            let answers = answers_in_field_order(content, field_order);
            json!({"outcome":{"outcome":"answered","answers":answers}})
        }
        QuestionResponse::Declined => json!({"outcome":{"outcome":"skipped"}}),
        QuestionResponse::Cancelled => json!({"outcome":{"outcome":"cancelled"}}),
    }
}

fn answers_in_field_order(
    mut content: BTreeMap<String, QuestionAnswerValue>,
    field_order: &[String],
) -> Vec<Value> {
    field_order
        .iter()
        .filter_map(|field_id| {
            let QuestionAnswerValue::SelectedOptions {
                selected_option_ids,
            } = content.remove(field_id)?
            else {
                return None;
            };
            Some(json!({"questionId":field_id,"selectedOptionIds":selected_option_ids}))
        })
        .collect()
}

impl<P: InteractionPort> HandleDispatchFrom<Agent> for ProviderCursorQuestionHandler<P> {
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
        if request.method() != "cursor/ask_question" {
            return Ok(Handled::No {
                message: Dispatch::Request(request, responder),
                retry: false,
            });
        }
        let cursor = match serde_json::from_value::<CursorAskQuestion>(request.params().clone()) {
            Ok(cursor) => cursor,
            Err(_) => {
                responder.respond_with_error(Error::invalid_params())?;
                return Ok(Handled::Yes);
            }
        };
        let Some(session_id) = self.tool_registry.resolve_session(&cursor.tool_call_id) else {
            tracing::warn!("unresolved Cursor question request");
            responder.respond(json!({"outcome":{"outcome":"cancelled"}}))?;
            return Ok(Handled::Yes);
        };
        let context = self
            .approval_contexts
            .lock()
            .ok()
            .and_then(|contexts| contexts.get(&session_id).cloned());
        let Some(context) = context else {
            tracing::warn!("Cursor question has no active Turn context");
            responder.respond(json!({"outcome":{"outcome":"cancelled"}}))?;
            return Ok(Handled::Yes);
        };
        let canonical = match Self::request_from_cursor(&session_id, responder.id(), cursor) {
            Ok(canonical) => canonical,
            Err(error) => {
                responder.respond_with_error(error)?;
                return Ok(Handled::Yes);
            }
        };
        let field_order = canonical
            .fields
            .iter()
            .map(|field| match field {
                QuestionField::Text { field_id, .. }
                | QuestionField::Number { field_id, .. }
                | QuestionField::Boolean { field_id, .. }
                | QuestionField::SingleChoice { field_id, .. }
                | QuestionField::MultiChoice { field_id, .. } => field_id.clone(),
            })
            .collect::<Vec<_>>();
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
            responder.respond(cursor_answer(response, &field_order))
        })?;
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderCursorQuestionHandler"
    }
}

#[cfg(test)]
mod tests {
    use super::cursor_answer;
    use session_event_model::QuestionResponse;

    #[test]
    fn cursor_decline_cancel_and_empty_answer_remain_distinct() {
        assert_eq!(
            cursor_answer(QuestionResponse::Declined, &[]),
            serde_json::json!({"outcome":{"outcome":"skipped"}})
        );
        assert_eq!(
            cursor_answer(QuestionResponse::Cancelled, &[]),
            serde_json::json!({"outcome":{"outcome":"cancelled"}})
        );
        assert_eq!(
            cursor_answer(
                QuestionResponse::Answered {
                    content: Default::default()
                },
                &[]
            ),
            serde_json::json!({"outcome":{"outcome":"skipped"}})
        );
    }
}
