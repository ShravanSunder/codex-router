#![allow(clippy::unwrap_used, clippy::expect_used)]

use codex_acp_adapter::{
    AcpConnectionContext, AcpRouterChannels, AcpSchemaCatalog, AcpSessionRegistry,
    CodexAdmissionSource, UnmaterializedBindingStore, bounded_acp_output, lazy_codex_session_route,
};
use collaboration_service::{NativeGenerationGate, UnmaterializedThreadHolder};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

#[path = "support/actor_native_fixture.rs"]
mod actor_native_fixture;
use actor_native_fixture::*;

#[tokio::test]
async fn abrupt_registry_drop_preserves_native_actor_on_host_tracker() {
    let fixture = NativeActorFixture::start(false);
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let gate = gate(&fixture);
    let registry = registry(
        &fixture,
        holder.clone(),
        gate.acquire().unwrap().retirement(),
    )
    .await;
    tokio::time::timeout(BOUND, fixture.started)
        .await
        .unwrap()
        .unwrap();
    drop(registry);
    assert!(!gate.acquire().unwrap().retirement().is_cancelled());
    assert!(
        !holder.host_tasks().is_empty(),
        "native actor tracked from creation"
    );
    fixture.finish.send(()).unwrap();
    tokio::time::timeout(BOUND, holder.drain_host_tasks())
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.backend)
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn actual_generation_retirement_ends_host_owned_actor() {
    let fixture = NativeActorFixture::start(true);
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let gate = gate(&fixture);
    let (output, mut frames) = bounded_acp_output(CancellationToken::new());
    let mut registry =
        AcpSessionRegistry::new(output, gate.acquire().unwrap().retirement(), holder.clone());
    registry
        .insert(fixture.binding().await)
        .unwrap_or_else(|_| panic!("insert"));
    registry
        .begin_prompt(
            &mut AcpSchemaCatalog::load().unwrap(),
            json!("retired-prompt"),
            prompt(),
        )
        .unwrap();
    tokio::time::timeout(BOUND, fixture.started)
        .await
        .unwrap()
        .unwrap();
    let update = tokio::time::timeout(BOUND, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        update
            .pointer("/params/update/content/text")
            .and_then(Value::as_str),
        Some("actor entered native event loop")
    );
    gate.retire().unwrap();
    tokio::time::timeout(BOUND, registry.complete_next())
        .await
        .unwrap()
        .unwrap();
    let terminal = tokio::time::timeout(BOUND, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal["id"], "retired-prompt");
    assert_eq!(terminal["error"]["code"], -32603);
    assert_eq!(
        terminal
            .pointer("/error/data/detail")
            .and_then(Value::as_str),
        Some("Native backend connection lost")
    );
    tokio::time::timeout(BOUND, holder.drain_host_tasks())
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.backend)
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn lazy_route_local_failure_reports_unavailability_and_preserves_native_actor() {
    let fixture = NativeActorFixture::start(false);
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let gate = gate(&fixture);
    let admission = Arc::new(Admission {
        gate: gate.clone(),
        holder: holder.clone(),
        calls: AtomicUsize::new(0),
        fail_catalog: true,
    });
    let closed = CancellationToken::new();
    let (input, receiver) = bounded_acp_output(closed.clone());
    let (output, mut frames) = bounded_acp_output(closed.clone());
    let route = tokio::spawn(lazy_codex_session_route(admission.clone()).run(
        AcpRouterChannels {
            input: receiver,
            output,
            closed: closed.clone(),
        },
        AcpConnectionContext {
            actor: None,
            client_profile: None,
            client_supports_elicitation_form: false,
        },
    ));
    input
        .send(json!({"jsonrpc":"2.0","id":"load","method":"session/load","params":load()}))
        .await
        .unwrap();
    let loaded = tokio::time::timeout(BOUND, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*loaded, json!({"jsonrpc":"2.0","id":"load","result":{}}));
    input
        .send(json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt","params":prompt()}))
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.started)
        .await
        .unwrap()
        .unwrap();
    input
        .send(json!({"jsonrpc":"2.0","id":"list","method":"session/list","params":{}}))
        .await
        .unwrap();
    let failed = tokio::time::timeout(BOUND, frames.recv())
        .await
        .expect("local failure must close observer promptly")
        .unwrap();
    assert_eq!(
        *failed,
        json!({"jsonrpc":"2.0","id":"list","error":{"code":-32000,"message":"Codex route unavailable"}})
    );
    let failed = tokio::time::timeout(BOUND, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *failed,
        json!({"jsonrpc":"2.0","id":"prompt","error":{"code":-32000,"message":"Codex route unavailable"}})
    );
    assert!(!gate.acquire().unwrap().retirement().is_cancelled());
    assert_eq!(admission.calls.load(Ordering::SeqCst), 1);
    fixture.finish.send(()).unwrap();
    tokio::time::timeout(BOUND, holder.drain_host_tasks())
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.backend)
        .await
        .unwrap()
        .unwrap();
    closed.cancel();
    tokio::time::timeout(BOUND, route)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn aborting_lazy_route_detaches_observer_and_preserves_native_actor() {
    let fixture = NativeActorFixture::start(false);
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let gate = gate(&fixture);
    let admission = Arc::new(Admission {
        gate: gate.clone(),
        holder: holder.clone(),
        calls: AtomicUsize::new(0),
        fail_catalog: true,
    });
    let closed = CancellationToken::new();
    let (input, receiver) = bounded_acp_output(closed.clone());
    let (output, mut frames) = bounded_acp_output(closed.clone());
    let route = tokio::spawn(lazy_codex_session_route(admission).run(
        AcpRouterChannels {
            input: receiver,
            output,
            closed: closed.clone(),
        },
        AcpConnectionContext {
            actor: None,
            client_profile: None,
            client_supports_elicitation_form: false,
        },
    ));
    input
        .send(json!({"jsonrpc":"2.0","id":"load","method":"session/load","params":load()}))
        .await
        .unwrap();
    assert_eq!(
        *tokio::time::timeout(BOUND, frames.recv())
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
    assert!(
        !holder.host_tasks().is_empty(),
        "actor belongs to Host before route abort"
    );
    route.abort();
    assert!(
        tokio::time::timeout(BOUND, route)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    closed.cancel();
    drop(input);
    drop(frames);
    assert!(!gate.acquire().unwrap().retirement().is_cancelled());
    assert!(
        !holder.host_tasks().is_empty(),
        "route abort retains Host actor"
    );
    fixture.finish.send(()).unwrap();
    tokio::time::timeout(BOUND, holder.drain_host_tasks())
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.backend)
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[derive(Clone, Copy)]
enum ObserverDropPoint {
    BeforeCompletion,
    QueuedCompletion,
    InstalledReady,
}
#[allow(clippy::panic)]
async fn recover_unmaterialized_at(point: ObserverDropPoint) {
    let (binding, backend) = unmaterialized_binding().await;
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let (output, mut frames) = bounded_acp_output(CancellationToken::new());
    let mut registry = AcpSessionRegistry::new(output, CancellationToken::new(), holder.clone());
    registry
        .insert(binding)
        .unwrap_or_else(|_| panic!("insert unmaterialized binding"));
    registry
        .begin_prompt(
            &mut AcpSchemaCatalog::load().unwrap(),
            json!("early-cancel"),
            prompt(),
        )
        .unwrap();
    registry.cancel(THREAD_ID).unwrap();
    match point {
        ObserverDropPoint::BeforeCompletion => {}
        ObserverDropPoint::QueuedCompletion => {
            tokio::time::timeout(BOUND, holder.drain_host_tasks())
                .await
                .unwrap();
        }
        ObserverDropPoint::InstalledReady => {
            tokio::time::timeout(BOUND, registry.complete_next())
                .await
                .unwrap()
                .unwrap();
            let terminal = tokio::time::timeout(BOUND, frames.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                terminal.pointer("/result/stopReason"),
                Some(&json!("cancelled"))
            );
            assert_eq!(
                terminal
                    .pointer("/result/_meta/codex-router~1nativeInterruption/state")
                    .and_then(Value::as_str),
                Some("notDispatched")
            );
        }
    }
    drop(registry);
    tokio::time::timeout(BOUND, holder.drain_host_tasks())
        .await
        .unwrap();
    assert!(
        holder.contains(THREAD_ID),
        "observer drop retains unmaterialized native connection"
    );
    let codex_acp_adapter::HeldBindingCheckout::Ready(binding) = holder.checkout(THREAD_ID) else {
        panic!("held binding must be available once");
    };
    assert!(binding.is_unmaterialized());
    assert!(matches!(
        holder.checkout(THREAD_ID),
        codex_acp_adapter::HeldBindingCheckout::Busy
    ));
    holder.restore(*binding);
    let codex_acp_adapter::HeldBindingCheckout::Ready(binding) = holder.checkout(THREAD_ID) else {
        panic!("restore retains binding");
    };
    holder.finish(THREAD_ID);
    drop(binding);
    assert!(matches!(
        holder.checkout(THREAD_ID),
        codex_acp_adapter::HeldBindingCheckout::Missing
    ));
    tokio::time::timeout(BOUND, backend).await.unwrap().unwrap();
}
#[tokio::test]
async fn aborted_observer_recovers_unmaterialized_completion_once() {
    recover_unmaterialized_at(ObserverDropPoint::BeforeCompletion).await;
}
#[tokio::test]
async fn dropped_observer_recovers_queued_unmaterialized_completion_once() {
    recover_unmaterialized_at(ObserverDropPoint::QueuedCompletion).await;
}
#[tokio::test]
async fn registry_drop_recovers_installed_ready_unmaterialized_binding_once() {
    recover_unmaterialized_at(ObserverDropPoint::InstalledReady).await;
}

#[tokio::test]
async fn aborting_outer_acp_connection_preserves_host_native_actor() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let fixture = NativeActorFixture::start(false);
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let gate = gate(&fixture);
    let admission = Admission {
        gate: gate.clone(),
        holder: holder.clone(),
        calls: AtomicUsize::new(0),
        fail_catalog: false,
    };
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let serving = tokio::spawn(codex_acp_adapter::serve_acp_connection(
        server,
        admission.acquire().unwrap(),
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    for (id, method, params) in [
        ("init", "initialize", json!({"protocolVersion":1})),
        ("load", "session/load", load()),
    ] {
        write
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(BOUND, read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id);
        assert!(reply.get("result").is_some(), "{reply}");
    }
    write
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt","params":prompt()})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.started)
        .await
        .unwrap()
        .unwrap();
    assert!(!holder.host_tasks().is_empty());
    serving.abort();
    assert!(serving.await.unwrap_err().is_cancelled());
    drop(write);
    drop(read);
    assert!(!gate.acquire().unwrap().retirement().is_cancelled());
    fixture.finish.send(()).unwrap();
    tokio::time::timeout(BOUND, holder.drain_host_tasks())
        .await
        .unwrap();
    tokio::time::timeout(BOUND, fixture.backend)
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn lazy_admission_rejects_unavailable_and_reacquires_after_actual_retirement() {
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let gate = NativeGenerationGate::default();
    let admission = Arc::new(Admission {
        gate: gate.clone(),
        holder,
        calls: AtomicUsize::new(0),
        fail_catalog: false,
    });
    let closed = CancellationToken::new();
    let (input, receiver) = bounded_acp_output(closed.clone());
    let (output, mut frames) = bounded_acp_output(closed.clone());
    let route = tokio::spawn(lazy_codex_session_route(admission.clone()).run(
        AcpRouterChannels {
            input: receiver,
            output,
            closed: closed.clone(),
        },
        AcpConnectionContext {
            actor: None,
            client_profile: None,
            client_supports_elicitation_form: false,
        },
    ));
    assert_eq!(admission.calls.load(Ordering::SeqCst), 0);
    input
        .send(json!({"jsonrpc":"2.0","id":"unavailable","method":"session/list","params":{}}))
        .await
        .unwrap();
    let reply = tokio::time::timeout(BOUND, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *reply,
        json!({"jsonrpc":"2.0","id":"unavailable","error":{"code":-32000,"message":"Codex backend unavailable: native backend unavailable"}})
    );
    gate.activate(
        generation(),
        "/tmp/lazy-actor-lifetime/backend.sock".into(),
        Some(schemas()),
    )
    .unwrap();
    for id in ["first", "same-admission"] {
        input
            .send(json!({"jsonrpc":"2.0","id":id,"method":"session/list","params":{}}))
            .await
            .unwrap();
        assert_eq!(
            *tokio::time::timeout(BOUND, frames.recv())
                .await
                .unwrap()
                .unwrap(),
            json!({"jsonrpc":"2.0","id":id,"result":{"sessions":[]}})
        );
    }
    assert_eq!(
        admission.calls.load(Ordering::SeqCst),
        2,
        "one rejected attempt then one admission"
    );
    gate.retire().unwrap();
    input
        .send(json!({"jsonrpc":"2.0","id":"retired","method":"session/list","params":{}}))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(BOUND, frames.recv())
            .await
            .unwrap()
            .unwrap()["error"]["code"],
        -32000
    );
    let next = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":2}),
    )
    .unwrap();
    gate.activate(
        next,
        "/tmp/lazy-actor-lifetime/backend.sock".into(),
        Some(schemas()),
    )
    .unwrap();
    input
        .send(json!({"jsonrpc":"2.0","id":"next-generation","method":"session/list","params":{}}))
        .await
        .unwrap();
    assert_eq!(
        *tokio::time::timeout(BOUND, frames.recv())
            .await
            .unwrap()
            .unwrap(),
        json!({"jsonrpc":"2.0","id":"next-generation","result":{"sessions":[]}})
    );
    input
        .send(json!({"jsonrpc":"2.0","id":"next-still-active","method":"session/list","params":{}}))
        .await
        .unwrap();
    assert_eq!(
        *tokio::time::timeout(BOUND, frames.recv())
            .await
            .unwrap()
            .unwrap(),
        json!({"jsonrpc":"2.0","id":"next-still-active","result":{"sessions":[]}})
    );
    assert_eq!(
        admission.calls.load(Ordering::SeqCst),
        4,
        "late previous-generation close cannot invalidate the current admission"
    );
    assert!(!gate.acquire().unwrap().retirement().is_cancelled());
    closed.cancel();
    tokio::time::timeout(BOUND, route)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
