//! Acceptance targets originate only from this run's successful native thread/start calls.
use codex_native_integration::NativeProtocolConnection;
use serde_json::{Value, json};
use std::{collections::BTreeSet, error::Error, path::Path};

const PROOF_MODEL: &str = "gpt-5.6-luna";

pub struct OwnedTurnReceipt {
    thread_id: String,
    turn_id: String,
}

#[derive(Default)]
pub struct OwnedThreadRegistry {
    threads: BTreeSet<String>,
}
impl OwnedThreadRegistry {
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
        control_socket: &Path,
    ) -> Result<String, Box<dyn Error>> {
        let socket = std::fs::canonicalize(control_socket)?;
        let profile = "debug-agent-communication";
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
            fields.insert("permissions".to_owned(), json!("debug-agent-communication"));
        } else {
            fields.insert("sandbox".to_owned(), json!("read-only"));
        }
        fields.insert(
            "config".to_owned(),
            super::proof_environment_settings::thread_overrides(configuration)?,
        );
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
                json!({"threadId":id,"model":PROOF_MODEL,"input":[{"type":"text","text":text}]}),
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
        let observed = tokio::time::timeout(std::time::Duration::from_secs(90), async {
            let mut output = String::new();
            loop {
                let event = client.next_message().await?;
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
        .await??;
        Ok(observed)
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};

    #[tokio::test]
    async fn proof_creation_selects_luna_and_rejects_model_substitution() {
        // Arrange: a real local carrier whose backend returns a different model.
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
            client,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let backend = tokio::spawn(async move {
            let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
                server,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            let request: Value =
                serde_json::from_str(server.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            server
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"id":request["id"],"result":{
                        "thread":{"id":"wrong-model-thread"},
                        "model":"gpt-5.6-sol","modelProvider":"codex-router-debug",
                        "sandbox":{"type":"readOnly"},"approvalPolicy":"never"
                    }})
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            request
        });
        let mut client = NativeProtocolConnection::from_websocket(wire);
        let mut registry = OwnedThreadRegistry::default();
        // Act: no real model is invoked by this protocol fixture.
        let result = registry.create(&mut client, Path::new("/tmp")).await;
        let request = backend.await.unwrap();
        // Assert: explicit model selection and fail-closed ownership admission.
        assert_eq!(
            request.pointer("/params/model").and_then(Value::as_str),
            Some("gpt-5.6-luna")
        );
        assert_eq!(
            request
                .pointer("/params/approvalsReviewer")
                .and_then(Value::as_str),
            Some("user")
        );
        assert_eq!(
            request.pointer("/params/config/features.hooks"),
            Some(&json!(false))
        );
        assert!(result.is_err());
        assert!(registry.require_owned("wrong-model-thread").is_err());
    }
    #[test]
    fn discovery_or_caller_text_does_not_grant_test_ownership() {
        // Arrange: one explicit creation receipt, unrelated user-provided IDs.
        let registry = OwnedThreadRegistry {
            threads: BTreeSet::from(["created-here".to_owned()]),
        };
        // Act / Assert.
        assert!(registry.require_owned("created-here").is_ok());
        assert!(registry.require_owned("existing-user-thread").is_err());
        assert!(registry.require_owned("").is_err());
    }

    #[tokio::test]
    async fn unowned_submission_cannot_write_a_native_frame() {
        // Arrange: a real carrier with no server protocol implementation.
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let wire = tokio_tungstenite::WebSocketStream::from_raw_socket(
            client,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let mut client = NativeProtocolConnection::from_websocket(wire);
        let registry = OwnedThreadRegistry::default();
        // Act: ownership rejection must complete before a request/response exchange can begin.
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            registry.submit_text(&mut client, "existing-user-thread", "must not send"),
        )
        .await
        .unwrap();
        // Assert: no frame was submitted and the untouched carrier is still open.
        assert!(result.is_err());
        let mut bytes = [0_u8; 128];
        assert!(
            matches!(server.try_read(&mut bytes), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}
