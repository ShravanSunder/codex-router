//! Background Thread Listen delivery through the ordinary native message dispatcher.
use crate::{EndpointDirectory, NativeControlBackend};
use collaboration_protocol::{
    MessageContent, MessageDelivery, NativeSendParams, SessionRef, UuidIdentity,
};
use message_board::{BatchSink, BatchSinkFailure, ListenDeliveryRecord, ThreadListenBatchSet};
use serde_json::{Value, json};

#[derive(Clone)]
pub(crate) struct SessionDeliverySink {
    pub(crate) service_id: UuidIdentity,
    pub(crate) endpoints: EndpointDirectory,
    pub(crate) backend: Option<NativeControlBackend>,
    pub(crate) target: SessionRef,
}

impl BatchSink for SessionDeliverySink {
    fn deliver<'a>(
        &'a self,
        record: ListenDeliveryRecord,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BatchSinkFailure>> + Send + 'a>,
    > {
        Box::pin(async move { self.dispatch(record).await })
    }
}

impl SessionDeliverySink {
    async fn dispatch(&self, record: ListenDeliveryRecord) -> Result<(), BatchSinkFailure> {
        let generation = self
            .backend
            .as_ref()
            .and_then(|backend| backend.gate.acquire().ok())
            .map(|admission| admission.generation().clone())
            .ok_or(BatchSinkFailure::Unavailable)?;
        let text = render_record(&record).map_err(|_| BatchSinkFailure::Unavailable)?;
        // A delivery is Router's own record; the target session did not send it.
        let message = MessageContent::Router {
            text: text.try_into().map_err(|_| BatchSinkFailure::Unavailable)?,
        };
        let params = NativeSendParams {
            target: self.target.clone(),
            generation,
            message,
            delivery: MessageDelivery::Auto,
            client_user_message_id: None,
        };
        let endpoints = self
            .endpoints
            .subscribe()
            .and_then(|subscription| subscription.snapshot())
            .map_err(|_| BatchSinkFailure::Unavailable)?
            .endpoints;
        let response = crate::native_message_dispatch::dispatch_message(
            crate::native_control_dispatch::NativeControlRequest {
                method: "codex/messageSend",
                params: serde_json::to_value(params).map_err(|_| BatchSinkFailure::Unavailable)?,
                id: json!(message_board::ListenId::generate().as_str()),
                service_id: &self.service_id,
                backend: self.backend.as_ref(),
                endpoints: &endpoints,
                stored_observation: None,
                access_routes: None,
            },
        )
        .await;
        if response.get("result").is_some() {
            Ok(())
        } else if response.pointer("/error/data/kind").and_then(Value::as_str)
            == Some("nativeRejected")
        {
            Err(BatchSinkFailure::Rejected {
                evidence: response
                    .pointer("/error/data")
                    .cloned()
                    .unwrap_or(Value::Null),
            })
        } else {
            Err(BatchSinkFailure::Unavailable)
        }
    }
}

fn render_record(record: &ListenDeliveryRecord) -> Result<String, serde_json::Error> {
    match record {
        ListenDeliveryRecord::Batch(batch_set) => {
            let summary = batch_summary(batch_set);
            Ok(format!("{summary}\n{}", serde_json::to_string(batch_set)?))
        }
        ListenDeliveryRecord::Heartbeat(heartbeat) => serde_json::to_string(heartbeat),
        ListenDeliveryRecord::Finalization(finalization) => serde_json::to_string(finalization),
    }
}

fn batch_summary(batch_set: &ThreadListenBatchSet) -> String {
    let count = batch_set
        .batches
        .iter()
        .map(|batch| batch.messages.len())
        .sum::<usize>();
    let first = batch_set
        .batches
        .iter()
        .flat_map(|batch| batch.messages.iter())
        .map(|message| message.activity_sequence.get())
        .min()
        .unwrap_or(0);
    let last = batch_set
        .batches
        .iter()
        .map(|batch| batch.delivered_through.get())
        .max()
        .unwrap_or(0);
    let threads = batch_set
        .batches
        .iter()
        .map(|batch| batch.root_message_id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "Thread activity: {threads}; count {count}; sequences {first}-{last}; catchUp: {}.",
        batch_set.catch_up
    )
}
