use super::test_support::ScriptedSessionBackend;
use super::*;
use codex_native_integration::{CodexProjectTrustLookup, ProjectTrustAnswer};
use std::{sync::Mutex, thread::ThreadId};

struct RecordingTrustLookup {
    observed_thread: Mutex<Option<ThreadId>>,
}

impl CodexProjectTrustLookup for RecordingTrustLookup {
    fn project_trust(&self, cwd: &Path) -> ProjectTrustAnswer {
        *self.observed_thread.lock().expect("test lock") = Some(std::thread::current().id());
        ProjectTrustAnswer::Untrusted {
            trust_target: cwd.to_string_lossy().into_owned(),
            explicitly_untrusted: false,
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn config_read_runs_filesystem_trust_lookup_off_the_executor()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let backend = ScriptedSessionBackend::new()?;
    let lookup = Arc::new(RecordingTrustLookup {
        observed_thread: Mutex::new(None),
    });
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            ScriptedSessionBackend::actor()?,
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
        .with_project_trust(Arc::clone(&lookup) as Arc<dyn CodexProjectTrustLookup>),
    );
    let executor_thread = std::thread::current().id();
    let reply = handle_app_server_rpc(
        context,
        json!({
            "id":1,"method":"config/read","params":{
                "includeLayers":true,"cwd":directory.path()
            }
        }),
    )
    .await?;
    assert_eq!(reply.response["id"], 1);
    let observed = lookup
        .observed_thread
        .lock()
        .expect("test lock")
        .ok_or("lookup not called")?;
    assert_ne!(observed, executor_thread);
    Ok(())
}
