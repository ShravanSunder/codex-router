#![allow(clippy::unwrap_used, clippy::expect_used)]
use codex_acp_adapter as adapter;
#[path = "../../../codex-acp-adapter/tests/support/actor_lifetime_fixture.rs"]
mod native_fixture;
pub use native_fixture::*;

use codex_acp_adapter::{
    AcpConnectionInputs, AcpSchemaCatalog, AcpSessionRegistry, AcpStoredSessions,
    CodexAdmissionSource, bounded_acp_output,
};
use collaboration_service::{NativeGenerationGate, UnmaterializedThreadHolder};
use serde_json::{Value, json};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
pub fn gate(fixture: &NativeActorFixture) -> NativeGenerationGate {
    let gate = NativeGenerationGate::default();
    gate.activate(generation(), fixture.path.clone(), Some(schemas()))
        .unwrap();
    gate
}
pub async fn registry(
    fixture: &NativeActorFixture,
    holder: Arc<UnmaterializedThreadHolder>,
    retired: CancellationToken,
) -> AcpSessionRegistry {
    let (output, _frames) = bounded_acp_output(CancellationToken::new());
    let mut registry = AcpSessionRegistry::new(output, retired, holder);
    registry
        .insert(fixture.binding().await)
        .map_err(|(error, _binding)| error)
        .expect("insert binding");
    registry
        .begin_prompt(
            &mut AcpSchemaCatalog::load().unwrap(),
            json!("prompt-lifetime"),
            prompt(),
        )
        .unwrap();
    registry
}

struct LocalCatalogFailure {
    fail: bool,
}
impl AcpStoredSessions for LocalCatalogFailure {
    // Deliberately fail the local JoinSet task, exercising its real error path.
    #[allow(clippy::panic)]
    fn list(&self, _: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        let fail = self.fail;
        Box::pin(async move {
            if fail {
                panic!("injected local catalog waiter failure");
            }
            Ok(json!({"sessions":[]}))
        })
    }
}
pub struct Admission {
    pub gate: NativeGenerationGate,
    pub holder: Arc<UnmaterializedThreadHolder>,
    pub calls: AtomicUsize,
    pub fail_catalog: bool,
}
impl CodexAdmissionSource for Admission {
    fn acquire(&self) -> io::Result<AcpConnectionInputs> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let admission = self.gate.acquire()?;
        Ok(AcpConnectionInputs {
            backend_path: admission.backend_path().to_path_buf(),
            generation: admission.generation().clone(),
            schemas: schemas(),
            holder: self.holder.clone(),
            retired: admission.retirement(),
            stored_sessions: Arc::new(LocalCatalogFailure {
                fail: self.fail_catalog,
            }),
            recorder: Arc::new(AcceptingConversationRecorder),
            approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        })
    }
}
