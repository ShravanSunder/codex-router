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
    let start = PendingAcpPrompt::start(
        inputs.session,
        &mut catalog,
        inputs.request_id.clone(),
        &inputs.params,
    );
    let pending = tokio::select! {
        biased;
        _=inputs.retired.cancelled()=>return PromptTaskCompletion::default(),
        command=inputs.commands.recv()=>{
            let terminal=match command {
                Some(PromptCommand::Cancel)=>Some(cancelled_response(&inputs.request_id,"unknown")),
                None=>None,
            };
            return PromptTaskCompletion {cancellation_barrier:Some(crate::CancellationBarrier::UnknownTurn),binding:None,terminal};
        },
        result=start=>result,
    };
    let mut pending = match pending {
        Ok(pending) => pending,
        Err(error) => {
            let message = error.to_string();
            return PromptTaskCompletion {
                cancellation_barrier: None,
                binding: None,
                terminal: Some(failure(&inputs.request_id, &message)),
            };
        }
    };
    loop {
        tokio::select! {
            biased;
            _=inputs.retired.cancelled()=>return PromptTaskCompletion {
                cancellation_barrier: None,binding:None,terminal:Some(failure(&inputs.request_id,"Native backend connection lost"))},
            command=inputs.commands.recv()=>match command {
                Some(PromptCommand::Cancel)=>{
                    let terminal=pending.cancel().await;
                    let cancellation_barrier=pending.blocks_next_prompt().then(|| pending.cancellation_barrier());
                    return PromptTaskCompletion {cancellation_barrier,binding:pending.into_session().ok(),terminal};
                },
                None=>{let _cancel=pending.cancel().await;return PromptTaskCompletion::default();},
            },
            event=pending.next_event(&mut catalog)=>match event {
                Ok(Some(PromptEvent::Update(frame)))=>{
                    if inputs.output.send(frame).await.is_err() {let _cancel=pending.cancel().await;return PromptTaskCompletion::default();}
                },
                Ok(Some(PromptEvent::Terminal(frame)))=>return PromptTaskCompletion {
                cancellation_barrier: None,binding:pending.into_session().ok(),terminal:Some(frame)},
                Ok(Some(PromptEvent::NativeCallback(_)))=>{
                    let _cancel=pending.cancel().await;
                    return PromptTaskCompletion {
                cancellation_barrier: None,binding:None,terminal:Some(failure(&inputs.request_id,"Unsupported native interaction or invalid update"))};
                },
                Err(error)=>{
                    let message=error.to_string();
                    let _cancel=pending.cancel().await;
                    return PromptTaskCompletion {
                cancellation_barrier: None,binding:None,terminal:Some(failure(&inputs.request_id,&message))};
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
