//! ACP connection routing over bounded carriers and one shared native backend.
use crate::session_setup_task::{SetupTaskInputs, SetupTaskOutput, run_session_setup};
use crate::{
    AcpNegotiation, AcpRouterChannels, AcpSchemaCatalog, AcpSessionRegistry,
    ConversationOperationRecorder, HeldBindingCheckout, UnmaterializedBindingStore,
    acp_connection_channels, run_acp_transport,
};
use codex_native_integration::NativePayloadSchemas;
use collaboration_protocol::{CodexGeneration, OperationId};
use serde_json::{Value, json};
use std::{collections::BTreeSet, future::Future, io, path::PathBuf, pin::Pin, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

/// Implemented by the shared stored-catalog owner; ACP never scans a second history store.
pub trait AcpStoredSessions: Send + Sync {
    fn list(&self, params: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>>;
}
pub struct AcpConnectionInputs {
    pub backend_path: PathBuf,
    pub generation: CodexGeneration,
    pub schemas: Arc<NativePayloadSchemas>,
    pub stored_sessions: Arc<dyn AcpStoredSessions>,
    pub retired: CancellationToken,
    pub approval_broker: Arc<dyn crate::ApprovalBroker>,
    pub holder: Arc<dyn UnmaterializedBindingStore>,
    pub recorder: Arc<dyn ConversationOperationRecorder>,
}
pub async fn serve_acp_connection<TStream: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: TStream,
    inputs: AcpConnectionInputs,
) -> io::Result<()> {
    let (wire, router) = acp_connection_channels();
    let closed = router.closed.clone();
    let transport = tokio::spawn(run_acp_transport(stream, wire));
    let result = route_connection(router, inputs).await;
    closed.cancel();
    let transport_result = transport
        .await
        .map_err(|_| io::Error::other("ACP transport task failed"))?;
    result.and(transport_result)
}
async fn route_connection(
    mut router: AcpRouterChannels,
    inputs: AcpConnectionInputs,
) -> io::Result<()> {
    let mut schema = AcpSchemaCatalog::load().map_err(io::Error::other)?;
    let mut negotiation = AcpNegotiation::default();
    let actor_retirement = inputs.retired.child_token();
    let mut sessions = AcpSessionRegistry::new(
        router.output.clone(),
        actor_retirement,
        Arc::clone(&inputs.holder),
    );
    let mut used_ids = BTreeSet::new();
    let mut catalog_requests: tokio::task::JoinSet<(Value, io::Result<Value>)> =
        tokio::task::JoinSet::new();
    let mut setup_requests: tokio::task::JoinSet<(Value, Option<String>, SetupTaskOutput)> =
        tokio::task::JoinSet::new();
    let result=async {
        loop {
            let frame=tokio::select! {
                _=router.closed.cancelled()=>break Ok(()),
                _=inputs.retired.cancelled()=>break Ok(()),
                completed=catalog_requests.join_next(),if !catalog_requests.is_empty()=>{
                    let Some(Ok((id, result)))=completed else {break Err(io::Error::other("ACP catalog task failed"));};
                    let response=match result {
                        Ok(result) if schema.validate("ListSessionsResponse",&result).unwrap_or(false)=>json!({"jsonrpc":"2.0","id":id,"result":result}),
                        Err(failure) if failure.kind()==io::ErrorKind::InvalidInput=>error(id,-32602,"Invalid stored session cursor or scope"),
                        _=>error(id,-32603,"Stored session discovery unavailable"),
                    };
                    router.output.send(response).await?;
                    continue;
                },
                completed=setup_requests.join_next(),if !setup_requests.is_empty()=>{
                    let Some(Ok((id, requested_session, completed)))=completed else {break Err(io::Error::other("ACP setup task failed"));};
                    if let Some(session_id)=requested_session.as_ref() {
                        sessions.finish_failed_load(session_id);
                        if completed.outcome.is_ok() { sessions.clear_cancellation_barrier(session_id); }
                    }
                    if let Some(binding)=completed.binding && let Err((_,binding))=sessions.insert(binding) {
                        inputs.holder.restore(*binding);
                        router.output.send(error(id,-32603,"Session capacity unavailable")).await?;
                        continue;
                    }
                    match completed.outcome {
                        Ok((history,result))=>{
                            if requested_session.is_none() {
                                let Some(session_id)=result.get("sessionId").and_then(Value::as_str) else {
                                    router.output.send(error(id,-32603,"Created session identity unavailable")).await?;
                                    continue;
                                };
                                match inputs.holder.checkout(session_id) {
                                    HeldBindingCheckout::Ready(binding) => {
                                        if let Err((_,binding)) = sessions.insert(*binding) {
                                            inputs.holder.restore(*binding);
                                            router.output.send(error(id,-32603,"Session capacity unavailable")).await?;
                                            continue;
                                        }
                                        inputs.holder.finish(session_id);
                                    }
                                    HeldBindingCheckout::Busy | HeldBindingCheckout::Missing => {
                                        router.output.send(error(id,-32603,"Created session binding unavailable")).await?;
                                        continue;
                                    }
                                }
                            }
                            for update in history {router.output.send(update).await?;}
                            router.output.send(json!({"jsonrpc":"2.0","id":id,"result":result})).await?;
                        },
                        Err(failure)=>router.output.send(setup_error(id,&failure)).await?,
                    }
                    continue;
                },
                result=sessions.complete_next(),if sessions.has_pending()=>{result.map_err(io::Error::other)?;continue;},
                frame=router.input.recv()=>match frame {Some(frame)=>frame,None=>break Ok(())},
            };
            if frame.get("jsonrpc")!=Some(&json!("2.0")) {router.output.send(error(Value::Null,-32600,"Invalid ACP envelope")).await?;continue;}
            // Router sends the client no requests, so a response frame is
            // unsolicited; JSON-RPC forbids answering it. A client-sent
            // session/request_permission carries a method and falls through to
            // the unsupported-method reply below.
            if frame.get("method").is_none() {continue;}
            let method=frame.get("method").and_then(Value::as_str).unwrap_or("");
            let params=frame.get("params").cloned().unwrap_or_else(||json!({}));
            let Some(id)=frame.get("id").cloned() else {
                if negotiation.is_initialized()&&method=="session/cancel"&&schema.validate("CancelNotification",&params).unwrap_or(false)
                    && let Some(session)=params.get("sessionId").and_then(Value::as_str) {let _cancel=sessions.cancel(session);}
                continue;
            };
            if !(id.is_null()||id.is_string()||id.as_i64().is_some()) {router.output.send(error(Value::Null,-32600,"Invalid ACP request ID")).await?;continue;}
            if used_ids.len()>=65536 {break Err(io::Error::other("ACP request lifetime limit"));}
            if !used_ids.insert(id.to_string()) {router.output.send(error(id,-32600,"ACP request ID reused")).await?;continue;}
            if method=="initialize" {
                let response=negotiation.initialize(&mut schema,&frame).unwrap_or_else(|_|error(id,-32602,"ACP initialization rejected"));
                router.output.send(response).await?;continue;
            }
            if !negotiation.is_initialized() {router.output.send(error(id,-32600,"ACP initialization required")).await?;continue;}
            match method {
                "session/new"|"session/load"=>{
                    let create_new=method=="session/new";
                    let definition=if create_new {"NewSessionRequest"} else {"LoadSessionRequest"};
                    if !schema.validate(definition,&params).unwrap_or(false) {router.output.send(error(id,-32602,"Invalid session setup parameters")).await?;continue;}
                    let operation_id = if create_new {
                        match params.pointer("/_meta/codexRouter/operationId").and_then(Value::as_str)
                            .and_then(|value| OperationId::try_from(value.to_owned()).ok()) {
                            Some(value) => Some(value),
                            None => {router.output.send(error(id,-32602,"Create requires a UUIDv7 operation ID")).await?;continue;}
                        }
                    } else {None};
                    if setup_requests.len()>=64 || (create_new && !sessions.has_setup_capacity(setup_requests.len())) {
                        router.output.send(error(id,-32603,"Session setup capacity unavailable")).await?;continue;
                    }
                    let requested_session=if create_new {None} else {params.get("sessionId").and_then(Value::as_str).map(str::to_owned)};
                    let mut known_session=if let Some(session_id)=requested_session.as_ref() {
                        match sessions.reserve_load(session_id,setup_requests.len()) {
                            Ok(binding)=>binding,
                            Err(_)=>{router.output.send(error(id,-32600,"Session busy or capacity unavailable")).await?;continue;},
                        }
                    } else {None};
                    let mut adopted_held = false;
                    if known_session.is_none() && let Some(session_id) = requested_session.as_ref() {
                        match inputs.holder.checkout(session_id) {
                            HeldBindingCheckout::Ready(binding) => {
                                known_session = Some(*binding);
                                adopted_held = true;
                            }
                            HeldBindingCheckout::Busy => {
                                sessions.finish_failed_load(session_id);
                                router.output.send(error(id,-32600,"Held session is busy")).await?;
                                continue;
                            }
                            HeldBindingCheckout::Missing => {}
                        }
                    }
                    let cancellation_barrier=requested_session.as_ref().and_then(|session| sessions.cancellation_barrier(session));
                    let setup=SetupTaskInputs { cancellation_barrier, known_session, adopt_unmaterialized:adopted_held, backend_path:inputs.backend_path.clone(), schemas:Arc::clone(&inputs.schemas), generation:inputs.generation.clone(), params, create_new, operation_id, recorder:Arc::clone(&inputs.recorder), approval_broker:Arc::clone(&inputs.approval_broker) };
                    let holder=Arc::clone(&inputs.holder);
                    if create_new {
                        let (result_sender,result_receiver)=tokio::sync::oneshot::channel();
                        holder.create_tasks().spawn(async move {
                            let mut outcome=run_session_setup(setup).await;
                            if let Some(binding)=outcome.binding.take() {
                                holder.hold(binding);
                            }
                            let _sent=result_sender.send(outcome);
                        });
                        setup_requests.spawn(async move {
                            let outcome=result_receiver.await.unwrap_or(SetupTaskOutput {
                                binding:None,
                                outcome:Err(crate::SessionSetupError::OutcomeUnknown),
                            });
                            drop(frame);
                            (id,None,outcome)
                        });
                    } else {
                        setup_requests.spawn(async move {
                            let mut checkout = match (adopted_held, requested_session.as_ref()) {
                                (true, Some(session_id)) => Some(CheckedOutBinding::new(holder, session_id.clone())),
                                _ => None,
                            };
                            let mut outcome=run_session_setup(setup).await;
                            if let Some(checkout) = checkout.as_mut() {
                                if outcome.outcome.is_err() && let Some(binding)=outcome.binding.take() {
                                    checkout.restore(binding);
                                } else {
                                    checkout.finish();
                                }
                            }
                            drop(frame);
                            (id,requested_session,outcome)
                        });
                    }
                },
                "session/prompt"=>{
                    if sessions.begin_prompt(&mut schema,id.clone(),params).is_err() {router.output.send(error(id,-32600,"Session prompt unavailable or already pending")).await?;}
                },
                "session/list"=>{
                    if !schema.validate("ListSessionsRequest",&params).unwrap_or(false) {router.output.send(error(id,-32602,"Invalid session list parameters")).await?;continue;}
                    if catalog_requests.len()>=64 {router.output.send(error(id,-32603,"Stored session request capacity unavailable")).await?;continue;}
                    let provider=Arc::clone(&inputs.stored_sessions);
                    catalog_requests.spawn(async move {
                        // Retain the inbound frame's byte permit through asynchronous work.
                        let result=provider.list(params).await;
                        drop(frame);
                        (id,result)
                    });
                },
                _=>router.output.send(error(id,-32601,"ACP method not supported")).await?,
            }
        }
    }.await;
    setup_requests.abort_all();
    while setup_requests.join_next().await.is_some() {}
    catalog_requests.abort_all();
    while catalog_requests.join_next().await.is_some() {}
    sessions.shutdown().await;
    result
}
struct CheckedOutBinding {
    holder: Arc<dyn UnmaterializedBindingStore>,
    session_id: String,
    active: bool,
}
impl CheckedOutBinding {
    fn new(holder: Arc<dyn UnmaterializedBindingStore>, session_id: String) -> Self {
        Self {
            holder,
            session_id,
            active: true,
        }
    }
    fn restore(&mut self, binding: crate::AcpSessionBinding) {
        self.holder.restore(binding);
        self.active = false;
    }
    fn finish(&mut self) {
        self.holder.finish(&self.session_id);
        self.active = false;
    }
}
impl Drop for CheckedOutBinding {
    fn drop(&mut self) {
        if self.active {
            self.holder.finish(&self.session_id);
        }
    }
}
fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn setup_error(id: Value, failure: &crate::SessionSetupError) -> Value {
    let code = if matches!(
        failure,
        crate::SessionSetupError::InvalidParameters
            | crate::SessionSetupError::ConfigurationMismatch
    ) {
        -32602
    } else {
        -32603
    };
    match failure {
        crate::SessionSetupError::ModelMismatch {
            requested,
            effective,
        } => json!({
            "jsonrpc":"2.0","id":id,
            "error":{"code":code,"message":"Native session setup rejected","data":{
                "kind":"modelMismatch","requested":requested,"effective":effective
            }}
        }),
        crate::SessionSetupError::AccessMismatch {
            requested,
            effective,
        } => json!({
            "jsonrpc":"2.0","id":id,
            "error":{"code":code,"message":"Native session setup rejected","data":{
                "kind":"accessMismatch","requested":requested,"effective":effective
            }}
        }),
        _ => error(id, code, "Native session setup rejected"),
    }
}
