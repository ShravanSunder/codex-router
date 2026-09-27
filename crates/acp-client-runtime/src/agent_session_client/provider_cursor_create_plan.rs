//! Publish a Cursor plan before asking its Approver whether to proceed.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use serde::Deserialize;
use serde_json::json;
use session_event_model::{
    ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, ApprovalSubject, OfferedOption,
    OfferedOptionId, OfferedOptions, OptionsOrigin, SessionEvent,
};
use tokio_util::sync::CancellationToken;

use super::{
    ActiveApprovalContext,
    provider_cursor_plan_items::{CursorPlanDefinition, CursorPlanItems, CursorTodo},
};
use crate::{
    ApprovalPortOutcome, InteractionPort, SessionEventSink,
    provider_connection_activity::ProviderConnectionActivity,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorCreatePlan {
    tool_call_id: String,
    name: Option<String>,
    overview: Option<String>,
    plan: String,
    #[serde(default)]
    todos: Vec<CursorTodo>,
}

pub(super) struct ProviderCursorCreatePlanHandler<P: InteractionPort> {
    tool_registry: Arc<ProviderConnectionActivity>,
    plan_items: Arc<CursorPlanItems>,
    event_sink: Arc<dyn SessionEventSink>,
    approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
    interaction_port: Arc<P>,
}

impl<P: InteractionPort> ProviderCursorCreatePlanHandler<P> {
    pub(super) fn new(
        tool_registry: Arc<ProviderConnectionActivity>,
        plan_items: Arc<CursorPlanItems>,
        event_sink: Arc<dyn SessionEventSink>,
        approval_contexts: Arc<Mutex<HashMap<String, ActiveApprovalContext<P>>>>,
        interaction_port: Arc<P>,
    ) -> Self {
        Self {
            tool_registry,
            plan_items,
            event_sink,
            approval_contexts,
            interaction_port,
        }
    }

    fn approval_request(
        session_id: &str,
        response_id: &impl serde::Serialize,
        plan: &CursorCreatePlan,
        plan_item_id: String,
    ) -> Result<ApprovalRequest, Error> {
        let request_id = serde_json::to_string(&(session_id, response_id))
            .map_err(|_| Error::internal_error())?;
        let options = OfferedOptions::new(vec![
            OfferedOption {
                option_id: OfferedOptionId::new("plan.accept")
                    .map_err(|_| Error::internal_error())?,
                label: "Accept plan".to_owned(),
                choice: ApprovalChoice::new(ApprovalEffect::Allow, ApprovalScope::Once),
            },
            OfferedOption {
                option_id: OfferedOptionId::new("plan.reject")
                    .map_err(|_| Error::internal_error())?,
                label: "Reject plan".to_owned(),
                choice: ApprovalChoice::new(ApprovalEffect::Decline, ApprovalScope::Once),
            },
        ])
        .map_err(|_| Error::internal_error())?;
        Ok(ApprovalRequest {
            request_id,
            title: plan
                .name
                .clone()
                .unwrap_or_else(|| "Approve plan".to_owned()),
            description: plan.overview.clone(),
            subject: Some(ApprovalSubject::Plan {
                tool_call_id: plan.tool_call_id.clone(),
                plan_item_id,
            }),
            options_origin: OptionsOrigin::RouterSynthesized,
            options,
        })
    }
}

fn cursor_plan_decision(outcome: ApprovalPortOutcome) -> serde_json::Value {
    match outcome {
        ApprovalPortOutcome::Selected { option_id, .. } if option_id == "plan.accept" => {
            json!({"outcome":"accepted"})
        }
        ApprovalPortOutcome::Selected { option_id, note } if option_id == "plan.reject" => {
            match note {
                Some(reason) => json!({"outcome":"rejected","reason":reason}),
                None => json!({"outcome":"rejected"}),
            }
        }
        ApprovalPortOutcome::Selected { .. }
        | ApprovalPortOutcome::Cancelled
        | ApprovalPortOutcome::Unavailable => json!({"outcome":"cancelled"}),
    }
}

impl<P: InteractionPort> HandleDispatchFrom<Agent> for ProviderCursorCreatePlanHandler<P> {
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
        if request.method() != "cursor/create_plan" {
            return Ok(Handled::No {
                message: Dispatch::Request(request, responder),
                retry: false,
            });
        }
        let plan = match serde_json::from_value::<CursorCreatePlan>(request.params().clone()) {
            Ok(plan) => plan,
            Err(_) => {
                responder.respond_with_error(Error::invalid_params())?;
                return Ok(Handled::Yes);
            }
        };
        let Some(session_id) = self.tool_registry.resolve_session(&plan.tool_call_id) else {
            tracing::warn!("unresolved Cursor create_plan request");
            responder.respond(json!({"outcome":"cancelled"}))?;
            return Ok(Handled::Yes);
        };
        let context = self
            .approval_contexts
            .lock()
            .ok()
            .and_then(|contexts| contexts.get(&session_id).cloned());
        let Some(context) = context else {
            tracing::warn!("Cursor create_plan has no active Turn context");
            responder.respond(json!({"outcome":"cancelled"}))?;
            return Ok(Handled::Yes);
        };
        let (existed, item) = self.plan_items.set_plan(
            &session_id,
            CursorPlanDefinition {
                tool_call_id: plan.tool_call_id.clone(),
                name: plan.name.clone(),
                overview: plan.overview.clone(),
                markdown: plan.plan.clone(),
                todos: plan.todos.clone(),
            },
        );
        let approval =
            Self::approval_request(&session_id, responder.id(), &plan, item.item_id.clone())?;
        let event = if existed {
            SessionEvent::ItemUpdated { item }
        } else {
            SessionEvent::ItemStarted { item }
        };
        if self.event_sink.publish(&session_id, event).is_err() {
            tracing::error!("provider Session event sink overflow on Cursor plan");
            responder.respond(json!({"outcome":"cancelled"}))?;
            return Ok(Handled::Yes);
        }
        let port = Arc::clone(&self.interaction_port);
        let request_cancellation = responder.cancellation();
        connection.spawn(async move {
            let agent_cancellation = CancellationToken::new();
            let decision = port.request_approval(
                context.approval,
                approval,
                context.cancelling,
                agent_cancellation.clone(),
            );
            tokio::pin!(decision);
            let outcome = tokio::select! {
                biased;
                () = request_cancellation.cancelled() => {
                    agent_cancellation.cancel();
                    decision.await
                }
                outcome = &mut decision => outcome,
            };
            responder.respond(cursor_plan_decision(outcome))
        })?;
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderCursorCreatePlanHandler"
    }
}
