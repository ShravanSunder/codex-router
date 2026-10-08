#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::actor_lifetime_fixture::*;
use super::*;
use serde_json::json;

#[derive(Default)]
struct BindingHolder {
    tasks: tokio_util::task::TaskTracker,
    binding: std::sync::Mutex<(usize, Option<AcpSessionBinding>)>,
}
impl UnmaterializedBindingStore for BindingHolder {
    fn hold(&self, binding: AcpSessionBinding) {
        assert!(
            binding.is_unmaterialized(),
            "materialized sibling must not be held"
        );
        let mut held = self.binding.lock().unwrap();
        held.0 += 1;
        assert!(
            held.1.replace(binding).is_none(),
            "completion held only once"
        );
    }
    fn checkout(&self, _: &str) -> HeldBindingCheckout {
        HeldBindingCheckout::Missing
    }
    fn restore(&self, _: AcpSessionBinding) {
        panic!("unexpected restore");
    }
    fn finish(&self, _: &str) {}
    fn host_tasks(&self) -> tokio_util::task::TaskTracker {
        self.tasks.clone()
    }
}

#[tokio::test]
async fn completion_waiter_failure_does_not_retire_live_native_sibling() {
    let fixture = NativeActorFixture::start(false);
    let holder = Arc::new(BindingHolder::default());
    let retired = CancellationToken::new();
    let (output, mut frames) = crate::bounded_acp_output(CancellationToken::new());
    let mut registry = AcpSessionRegistry::new(output, retired.clone(), holder.clone());
    registry
        .insert(fixture.binding().await)
        .unwrap_or_else(|_| panic!("insert"));
    registry
        .begin_prompt(
            &mut AcpSchemaCatalog::load().unwrap(),
            json!("live-sibling"),
            prompt(),
        )
        .unwrap();
    tokio::time::timeout(BOUND, fixture.started)
        .await
        .unwrap()
        .unwrap();
    registry
        .prompts
        .spawn(async { panic!("injected completion observer failure") });
    assert!(matches!(
        tokio::time::timeout(BOUND, registry.complete_next())
            .await
            .unwrap(),
        Err(SessionRegistryError::NotLoaded)
    ));
    assert!(
        !retired.is_cancelled(),
        "local waiter has no native retirement authority"
    );
    fixture.finish.send(()).unwrap();
    tokio::time::timeout(BOUND, registry.complete_next())
        .await
        .unwrap()
        .unwrap();
    let update = tokio::time::timeout(BOUND, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        update.pointer("/params/sessionId").and_then(Value::as_str),
        Some(THREAD_ID)
    );
    let terminal = tokio::time::timeout(BOUND, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal["id"], "live-sibling");
    assert_eq!(terminal["result"]["stopReason"], "end_turn");
    assert!(matches!(
        registry.sessions.get(THREAD_ID),
        Some(SessionSlot::Ready(_))
    ));
    registry.shutdown().await;
    assert!(
        !retired.is_cancelled(),
        "graceful observer shutdown has no retirement authority"
    );
    holder.tasks.close();
    tokio::time::timeout(BOUND, holder.tasks.wait())
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.backend)
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn rejected_completion_delivery_recovers_real_actor_unmaterialized_binding_once() {
    let (session, backend) = unmaterialized_binding().await;
    let holder = Arc::new(BindingHolder::default());
    let (commands, receiver) = mpsc::channel(1);
    commands.try_send(PromptCommand::Cancel).unwrap();
    let (output, _frames) = crate::bounded_acp_output(CancellationToken::new());
    let completion_receiver = spawn_host_prompt(
        PromptTaskInputs {
            session,
            request_id: json!("unsent-completion"),
            params: prompt(),
            commands: receiver,
            output,
            retired: CancellationToken::new(),
        },
        holder.clone(),
    );
    // Current-thread Tokio cannot poll the actor before this synchronous drop.
    // Its real early-cancel completion necessarily takes sender-rejection recovery.
    drop(completion_receiver);
    holder.tasks.close();
    tokio::time::timeout(BOUND, holder.tasks.wait())
        .await
        .unwrap();
    let (count, binding) = {
        let mut held = holder.binding.lock().unwrap();
        (held.0, held.1.take())
    };
    assert_eq!(count, 1);
    let binding = binding.expect("recovered actual binding");
    assert_eq!(binding.session_id(), THREAD_ID);
    assert!(binding.is_unmaterialized());
    drop(binding);
    tokio::time::timeout(BOUND, backend).await.unwrap().unwrap();
}
