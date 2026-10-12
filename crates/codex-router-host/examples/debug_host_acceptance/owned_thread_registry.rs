//! Acceptance targets are fresh starts or children of this run's owned native parents.
use codex_native_integration::NativeProtocolConnection;
use serde_json::{Value, json};
use std::{collections::BTreeSet, error::Error, path::Path};

pub(super) const PROOF_MODEL: &str = "gpt-6-luna";
const PROOF_EFFORT: &str = "medium";

pub struct OwnedTurnReceipt {
    thread_id: String,
    turn_id: String,
}

#[derive(Default)]
pub struct OwnedThreadRegistry {
    threads: BTreeSet<String>,
}
impl OwnedThreadRegistry {
    pub async fn fork_owned_thread(
        &mut self,
        client: &mut NativeProtocolConnection,
        parent: &str,
        cwd: &Path,
    ) -> Result<String, Box<dyn Error>> {
        self.require_owned(parent)?;
        // No policy/config/model override: the native fork inherits its owned parent.
        let response = client
            .request(
                "thread/fork",
                json!({"threadId":parent,"cwd":cwd,"deferGoalContinuation":true}),
            )
            .await?;
        let child = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("native fork returned no child identity; effect is unknown")?
            .to_owned();
        if child == parent || !self.threads.insert(child.clone()) {
            return Err("native fork did not return a distinct owned child".into());
        }
        if response.get("modelProvider").and_then(Value::as_str) != Some("codex-router-debug")
            || response.get("model").and_then(Value::as_str) != Some(PROOF_MODEL)
            || response.get("reasoningEffort").and_then(Value::as_str) != Some(PROOF_EFFORT)
            || response.pointer("/sandbox/type").and_then(Value::as_str) != Some("readOnly")
            || response.get("approvalPolicy").and_then(Value::as_str) != Some("never")
            || response.get("approvalsReviewer").and_then(Value::as_str) != Some("user")
            || response.get("cwd").and_then(Value::as_str) != cwd.to_str()
        {
            return Err("owned fork did not preserve source cwd/model/read-only policy".into());
        }
        Ok(child)
    }
    pub async fn create(
        &mut self,
        client: &mut NativeProtocolConnection,
        cwd: &Path,
    ) -> Result<String, Box<dyn Error>> {
        self.create_with_config(client, cwd, None, "never").await
    }
    pub async fn create_with_user_review(
        &mut self,
        client: &mut NativeProtocolConnection,
        cwd: &Path,
    ) -> Result<String, Box<dyn Error>> {
        self.create_with_config(client, cwd, None, "untrusted")
            .await
    }
    pub async fn create_with_socket_access(
        &mut self,
        client: &mut NativeProtocolConnection,
        cwd: &Path,
        service_socket: &Path,
    ) -> Result<String, Box<dyn Error>> {
        let socket = std::fs::canonicalize(service_socket)?;
        let profile = "debug-agent-collaboration";
        let configuration = json!({
            "permissions":{profile:{"extends":":read-only","network":{
                "enabled":true,"domains":{},
                "unix_sockets":{socket.to_str().ok_or("socket path is not UTF-8")?:"allow"},
                "allow_local_binding":false,"allow_upstream_proxy":false,
                "dangerously_allow_all_unix_sockets":false
            }}},
            "features.network_proxy":{
                "enabled":true,"credential_broker":false,
                "proxy_url":"http://127.0.0.1:0","enable_socks5":false,
                "domains":{},
                "unix_sockets":{socket.to_str().ok_or("socket path is not UTF-8")?:"allow"},
                "allow_local_binding":false,"allow_upstream_proxy":false,
                "dangerously_allow_all_unix_sockets":false
            }
        });
        self.create_with_config(client, cwd, Some(configuration), "never")
            .await
    }
    async fn create_with_config(
        &mut self,
        client: &mut NativeProtocolConnection,
        cwd: &Path,
        configuration: Option<Value>,
        approval_policy: &str,
    ) -> Result<String, Box<dyn Error>> {
        let mut parameters = json!({"cwd":cwd,"model":PROOF_MODEL,"experimentalRawEvents":false,"approvalPolicy":approval_policy,"approvalsReviewer":"user"});
        let fields = parameters
            .as_object_mut()
            .ok_or("invalid creation parameters")?;
        if configuration.is_some() {
            fields.insert("permissions".to_owned(), json!("debug-agent-collaboration"));
        } else {
            fields.insert("sandbox".to_owned(), json!("read-only"));
        }
        let mut configuration = super::proof_environment_settings::thread_overrides(configuration)?;
        configuration
            .as_object_mut()
            .ok_or("proof configuration must be an object")?
            .insert("model_reasoning_effort".to_owned(), json!(PROOF_EFFORT));
        fields.insert("config".to_owned(), configuration);
        let response = match client.request("thread/start", parameters).await {
            Ok(response) => response,
            Err(error) => {
                if let Some(rejection) = client.take_last_rejection() {
                    use std::io::Write;
                    use std::os::unix::fs::OpenOptionsExt;
                    let path = std::env::temp_dir().join(format!(
                        "debug-thread-rejection-{}.json",
                        std::process::id()
                    ));
                    let mut file = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .mode(0o600)
                        .open(&path)?;
                    file.write_all(&serde_json::to_vec(&rejection)?)?;
                    eprintln!("Private thread rejection captured for this proof process");
                }
                return Err(error.into());
            }
        };
        let id = response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("native creation returned no thread identity")?
            .to_owned();
        if response.get("modelProvider").and_then(Value::as_str) != Some("codex-router-debug") {
            return Err("created thread did not retain debug provider".into());
        }
        if response.get("model").and_then(Value::as_str) != Some(PROOF_MODEL) {
            return Err("created proof thread did not retain Luna; no turn submitted".into());
        }
        if response.get("reasoningEffort").and_then(Value::as_str) != Some(PROOF_EFFORT) {
            return Err(
                "created proof thread did not retain explicit effort; no turn submitted".into(),
            );
        }
        if response.pointer("/sandbox/type").and_then(Value::as_str) != Some("readOnly")
            || response.get("approvalPolicy").and_then(Value::as_str) != Some(approval_policy)
            || response.get("approvalsReviewer").and_then(Value::as_str) != Some("user")
        {
            return Err(
                "proof thread did not retain read-only and explicit user-review policy".into(),
            );
        }
        if !self.threads.insert(id.clone()) {
            return Err("native creation reused an identity".into());
        }
        Ok(id)
    }
    pub fn require_owned(&self, id: &str) -> Result<(), &'static str> {
        if self.threads.contains(id) {
            Ok(())
        } else {
            Err("acceptance target was not created by this run")
        }
    }
    pub async fn inspect(
        &self,
        client: &mut NativeProtocolConnection,
        id: &str,
    ) -> Result<(), Box<dyn Error>> {
        self.require_owned(id)?;
        let _thread = client.inspect_thread(id).await?;
        Ok(())
    }

    pub async fn submit_text(
        &self,
        client: &mut NativeProtocolConnection,
        id: &str,
        text: &str,
    ) -> Result<OwnedTurnReceipt, Box<dyn Error>> {
        self.require_owned(id)?;
        let response = client
            .request(
                "turn/start",
                json!({"threadId":id,"model":PROOF_MODEL,"effort":PROOF_EFFORT,"input":[{"type":"text","text":text}]}),
            )
            .await?;
        let turn = response
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or("native acceptance lacked turn identity")?
            .to_owned();
        println!(
            "{}",
            json!({"kind":"ownedTurnAccepted","threadId":id,"turnId":turn})
        );
        Ok(OwnedTurnReceipt {
            thread_id: id.to_owned(),
            turn_id: turn,
        })
    }

    pub async fn observe_turn(
        &self,
        client: &mut NativeProtocolConnection,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<String, Box<dyn Error>> {
        self.require_owned(thread_id)?;
        self.observe_text(
            client,
            OwnedTurnReceipt {
                thread_id: thread_id.to_owned(),
                turn_id: turn_id.to_owned(),
            },
        )
        .await
    }
    pub async fn observe_text(
        &self,
        client: &mut NativeProtocolConnection,
        receipt: OwnedTurnReceipt,
    ) -> Result<String, Box<dyn Error>> {
        self.require_owned(&receipt.thread_id)?;
        let id = receipt.thread_id.as_str();
        let turn = receipt.turn_id;
        let mut observed_methods = std::collections::BTreeMap::<String, u64>::new();
        let observed = tokio::time::timeout(std::time::Duration::from_secs(90), async {
            let mut output = String::new();
            loop {
                let event = client.next_message().await?;
                if let Some(method) = event.get("method").and_then(Value::as_str)
                    && method.len() <= 96
                    && (observed_methods.len() < 32 || observed_methods.contains_key(method))
                {
                    *observed_methods.entry(method.to_owned()).or_default() += 1;
                }
                let params = event
                    .get("params")
                    .ok_or("native event lacked parameters")?;
                if params.get("threadId").and_then(Value::as_str) != Some(id) {
                    continue;
                }
                if event.get("id").is_some() {
                    return Err::<String, Box<dyn Error>>(
                        "proof encountered a callback; no approval granted".into(),
                    );
                }
                match event.get("method").and_then(Value::as_str) {
                    Some("error")
                        if params.get("turnId").and_then(Value::as_str) == Some(turn.as_str()) =>
                    {
                        println!("{}", serde_json::to_string(
                            &super::owned_turn_error_diagnostic::OwnedTurnErrorDiagnostic::from_parameters(params)
                        )?);
                        if params.get("willRetry").and_then(Value::as_bool) == Some(false) {
                            return Err("owned native turn reported a terminal error".into());
                        }
                    }
                    Some("item/agentMessage/delta")
                        if params.get("turnId").and_then(Value::as_str) == Some(turn.as_str()) =>
                    {
                        let text = params
                            .get("delta")
                            .and_then(Value::as_str)
                            .ok_or("invalid text delta")?;
                        if output.len().saturating_add(text.len()) > 8192 {
                            return Err("proof output exceeded its bound".into());
                        }
                        output.push_str(text);
                    }
                    Some("item/completed")
                        if params.get("turnId").and_then(Value::as_str) == Some(turn.as_str()) =>
                    {
                        if params.pointer("/item/type").and_then(Value::as_str)
                            == Some("agentMessage")
                            && params.pointer("/item/phase").and_then(Value::as_str)
                                != Some("commentary")
                            && let Some(text) = params.pointer("/item/text").and_then(Value::as_str)
                        {
                            if text.len() > 8192 {
                                return Err("proof output exceeded its bound".into());
                            }
                            output = text.to_owned();
                        }
                    }
                    Some("turn/completed")
                        if params.pointer("/turn/id").and_then(Value::as_str)
                            == Some(turn.as_str()) =>
                    {
                        if params.pointer("/turn/status").and_then(Value::as_str)
                            != Some("completed")
                        {
                            return Err("owned native turn did not complete successfully".into());
                        }
                        return Ok(output);
                    }
                    _ => {}
                }
            }
        })
        .await;
        println!(
            "{}",
            json!({"kind":"ownedTurnObservation","methods":observed_methods})
        );
        observed?
    }
}

#[cfg(test)]
#[path = "owned_thread_registry_tests.rs"]
mod ownership_tests;
