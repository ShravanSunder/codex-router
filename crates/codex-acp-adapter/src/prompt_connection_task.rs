//! One prompt actor keeps frontend cancellation responsive while native work is pending.
use crate::{AcpSchemaCatalog, AcpSessionBinding, PendingAcpPrompt, PromptEvent};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub enum PromptCommand {
    Cancel,
}
pub struct PromptTaskInputs {
    pub session: AcpSessionBinding,
    pub request_id: Value,
    pub params: Value,
    pub commands: mpsc::Receiver<PromptCommand>,
    pub output: crate::AcpOutputSender,
    pub retired: CancellationToken,
}
/// The registry installs this binding before publishing the terminal response.
#[derive(Default)]
pub struct PromptTaskCompletion {
    pub binding: Option<AcpSessionBinding>,
    pub terminal: Option<Value>,
    pub cancellation_barrier: Option<crate::CancellationBarrier>,
}
pub async fn run_prompt_task(mut inputs: PromptTaskInputs) -> PromptTaskCompletion {
    if inputs.retired.is_cancelled() {
        return PromptTaskCompletion::default();
    }
    if let Ok(command) = inputs.commands.try_recv() {
        let terminal = if matches!(command, PromptCommand::Cancel) {
            cancelled_response(&inputs.request_id, "notDispatched")
        } else {
            failure(
                &inputs.request_id,
                "Unexpected permission response before prompt dispatch",
            )
        };
        return PromptTaskCompletion {
            cancellation_barrier: None,
            binding: Some(inputs.session),
            terminal: Some(terminal),
        };
    }
    let mut catalog = match AcpSchemaCatalog::load() {
        Ok(catalog) => catalog,
        Err(_error) => {
            return PromptTaskCompletion {
                cancellation_barrier: None,
                binding: Some(inputs.session),
                terminal: Some(failure(&inputs.request_id, "Native schema unavailable")),
            };
        }
    };
    let mut commands_open = true;
    let mut frontend_detached = false;
    let pending = {
        let start = PendingAcpPrompt::start(
            inputs.session,
            &mut catalog,
            inputs.request_id.clone(),
            &inputs.params,
        );
        tokio::pin!(start);
        tokio::select! {
            biased;
            _=inputs.retired.cancelled()=>return PromptTaskCompletion::default(),
            command=inputs.commands.recv(),if commands_open=>{
                let terminal=match command {
                    Some(PromptCommand::Cancel)=>Some(cancelled_response(&inputs.request_id,"unknown")),
                    None=>{
                        commands_open=false;
                        frontend_detached=true;
                        inputs.commands.close();
                        None
                    },
                };
                if commands_open || terminal.is_some() {
                    return PromptTaskCompletion {cancellation_barrier:Some(crate::CancellationBarrier::UnknownTurn),binding:None,terminal};
                }
                start.await
            },
            result=&mut start=>result,
        }
    };
    let mut pending = match pending {
        Ok(pending) => pending,
        Err(error) => {
            let message = error.to_string();
            return PromptTaskCompletion {
                cancellation_barrier: frontend_detached
                    .then_some(crate::CancellationBarrier::UnknownTurn),
                binding: None,
                terminal: (!frontend_detached).then(|| failure(&inputs.request_id, &message)),
            };
        }
    };
    loop {
        tokio::select! {
            biased;
            _=inputs.retired.cancelled()=>return PromptTaskCompletion {
                cancellation_barrier: None,binding:None,terminal:Some(failure(&inputs.request_id,"Native backend connection lost"))},
            command=inputs.commands.recv(),if commands_open=>match command {
                Some(PromptCommand::Cancel)=>{
                    let terminal=pending.cancel().await;
                    let cancellation_barrier=pending.blocks_next_prompt().then(|| pending.cancellation_barrier());
                    return PromptTaskCompletion {cancellation_barrier,binding:pending.into_session().ok(),terminal};
                },
                None=>{
                    commands_open=false;
                    frontend_detached=true;
                    inputs.commands.close();
                },
            },
            event=pending.next_event(&mut catalog)=>match event {
                Ok(Some(PromptEvent::Update(frame)))=>{
                    if !frontend_detached && inputs.output.send(frame).await.is_err() {
                        frontend_detached=true;
                        commands_open=false;
                        inputs.commands.close();
                    }
                },
                Ok(Some(PromptEvent::Terminal(frame)))=>return PromptTaskCompletion {
                cancellation_barrier: pending.blocks_next_prompt().then(|| pending.cancellation_barrier()),
                binding:pending.into_session().ok(),terminal:(!frontend_detached).then_some(frame)},
                Ok(Some(PromptEvent::NativeCallback(_)))=>{
                    if !frontend_detached {let _cancel=pending.cancel().await;}
                    return PromptTaskCompletion {
                cancellation_barrier: frontend_detached.then(|| pending.cancellation_barrier()),
                binding:None,terminal:(!frontend_detached).then(|| failure(&inputs.request_id,"Unsupported native interaction or invalid update"))};
                },
                Err(error)=>{
                    let message=error.to_string();
                    if !frontend_detached {let _cancel=pending.cancel().await;}
                    return PromptTaskCompletion {
                cancellation_barrier: frontend_detached.then(|| pending.cancellation_barrier()),
                binding:None,terminal:(!frontend_detached).then(|| failure(&inputs.request_id,&message))};
                },
                Ok(Some(PromptEvent::NativeNotification(_))|None)=>{},
            }
        }
    }
}
fn cancelled_response(id: &Value, state: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"cancelled","_meta":{"codex-router/nativeInterruption":{"state":state}}}})
}
fn failure(id: &Value, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"Native prompt rejected","data":{"detail":message}}})
}
