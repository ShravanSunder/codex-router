#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::session_connection_registry::actor_lifetime_fixture::*;
use crate::{AcpStoredSessions, HeldBindingCheckout, UnmaterializedBindingStore};
use std::{future::Future, pin::Pin};

#[derive(Default)]
struct BindingHolder {
    tasks: tokio_util::task::TaskTracker,
}
impl UnmaterializedBindingStore for BindingHolder {
    fn hold(&self, _: crate::AcpSessionBinding) {
        panic!("materialized actor must not be held");
    }
    fn checkout(&self, _: &str) -> HeldBindingCheckout {
        HeldBindingCheckout::Missing
    }
    fn restore(&self, _: crate::AcpSessionBinding) {
        panic!("unexpected restore");
    }
    fn finish(&self, _: &str) {}
    fn host_tasks(&self) -> tokio_util::task::TaskTracker {
        self.tasks.clone()
    }
}
struct EmptyCatalog;
impl AcpStoredSessions for EmptyCatalog {
    fn list(&self, _: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(async { Ok(json!({"sessions":[]})) })
    }
}

async fn assert_failed_input_before_closed_event(actual_retirement: bool) {
    let fixture = NativeActorFixture::start(actual_retirement);
    let holder = Arc::new(BindingHolder::default());
    let retirement = CancellationToken::new();
    let closed = CancellationToken::new();
    let (input, input_receiver) = bounded_acp_output(closed.clone());
    let (inner_output, mut inner_frames) = bounded_acp_output(closed.clone());
    let native_route = route_codex_sessions(
        AcpRouterChannels {
            input: input_receiver,
            output: inner_output,
            closed: closed.clone(),
        },
        AcpConnectionInputs {
            backend_path: fixture.path.clone(),
            generation: generation(),
            schemas: schemas(),
            retired: retirement.clone(),
            holder: holder.clone(),
            stored_sessions: Arc::new(EmptyCatalog),
            recorder: Arc::new(AcceptingConversationRecorder),
            approval_broker: Arc::new(crate::RejectingApprovalBroker),
        },
    );
    let task = tokio::spawn(async move {
        let _result = native_route.await;
    });
    input
        .send(json!({"jsonrpc":"2.0","id":"load","method":"session/load","params":load()}))
        .await
        .unwrap();
    assert_eq!(
        *tokio::time::timeout(BOUND, inner_frames.recv())
            .await
            .unwrap()
            .unwrap(),
        json!({"jsonrpc":"2.0","id":"load","result":{}})
    );
    input
        .send(json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt","params":prompt()}))
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.started)
        .await
        .unwrap()
        .unwrap();
    if actual_retirement {
        let update = tokio::time::timeout(BOUND, inner_frames.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            update.pointer("/params/update/content/text"),
            Some(&json!("actor entered native event loop"))
        );
    }
    let mut active = Some(ActiveGeneration {
        epoch: 1,
        input,
        closed,
        retirement: retirement.clone(),
        task,
        session_ids: BTreeSet::from([THREAD_ID.to_owned()]),
        pending: BTreeMap::from([(json!("prompt").to_string(), json!("prompt"))]),
    });
    // Join the real inner route's abortion before forwarding the next input.
    // Its receiver is certainly gone, and no Closed event is handled first.
    let current = active.as_mut().unwrap();
    current.task.abort();
    assert!(
        tokio::time::timeout(BOUND, &mut current.task)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert!(!retirement.is_cancelled());
    if actual_retirement {
        retirement.cancel();
    }
    let (outer_output, mut outer_frames) = bounded_acp_output(CancellationToken::new());
    let mut closed_sessions = BTreeMap::new();
    let result = forward_active_input(
        &mut active,
        &mut closed_sessions,
        &outer_output,
        &json!({"jsonrpc":"2.0","id":"late-input","method":"session/list","params":{}}),
    )
    .await;
    assert!(
        result.is_ok(),
        "local observer failure must preserve outer route: {result:?}"
    );
    assert!(active.is_none());
    let message = if actual_retirement {
        "Codex generation retired"
    } else {
        "Codex route unavailable"
    };
    for id in ["late-input", "prompt"] {
        assert_eq!(
            *tokio::time::timeout(BOUND, outer_frames.recv())
                .await
                .unwrap()
                .unwrap(),
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}})
        );
    }
    assert!(
        outer_frames.try_recv().is_err(),
        "each pending request settled once"
    );
    assert_eq!(
        closed_sessions
            .get(THREAD_ID)
            .map(|reason| reason.session_message()),
        Some(if actual_retirement {
            "Codex session generation retired"
        } else {
            "Codex session route unavailable"
        })
    );
    assert_eq!(retirement.is_cancelled(), actual_retirement);
    if !actual_retirement {
        fixture.finish.send(()).unwrap();
    }
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
async fn failed_input_before_closed_event_settles_pending_locally_and_keeps_native_actor() {
    assert_failed_input_before_closed_event(false).await;
}
#[tokio::test]
async fn failed_input_before_closed_event_preserves_actual_retirement_disposition() {
    assert_failed_input_before_closed_event(true).await;
}
