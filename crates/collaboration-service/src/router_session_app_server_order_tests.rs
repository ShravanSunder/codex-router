use super::test_support::ScriptedSessionBackend;
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use tokio::{
    net::UnixListener,
    sync::Notify,
    time::{Duration, timeout},
};
use tokio_tungstenite::{WebSocketStream, client_async};

async fn receive_reply(
    client: &mut WebSocketStream<UnixStream>,
    id: i64,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let frame: Value = serde_json::from_str(
            timeout(Duration::from_secs(2), client.next())
                .await?
                .ok_or("missing RPC reply")??
                .to_text()?,
        )?;
        if frame.get("id") == Some(&json!(id)) {
            return Ok(frame);
        }
    }
}

#[tokio::test]
async fn turn_commands_keep_steer_and_interrupt_arrival_order()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let backend = ScriptedSessionBackend::new()?;
    let context = Arc::new(RouterSessionAppServerContext::new(
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        tokio::sync::watch::channel(Vec::new()).1,
    ));
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept client");
        serve_router_session_app_server_connection(stream, context)
            .await
            .expect("serve client");
    });
    let stream = UnixStream::connect(&socket_path).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;
    client
        .send(Message::Text(
            json!({"id":1,"method":"thread/start","params":{
                "cwd":directory.path()
            }})
            .to_string()
            .into(),
        ))
        .await?;
    let started = receive_reply(&mut client, 1).await?;
    let thread_id = started["result"]["thread"]["id"]
        .as_str()
        .ok_or("thread ID")?;

    let first_gate = Arc::new(Notify::new());
    *backend.steer_gate.lock().expect("test lock") = Some(Arc::clone(&first_gate));
    client
        .send(Message::Text(
            json!({"id":2,"method":"turn/steer","params":{
                "threadId":thread_id,"expectedTurnId":"turn-1",
                "input":[{"type":"text","text":"first"}]
            }})
            .to_string()
            .into(),
        ))
        .await?;
    backend.steer_started.notified().await;
    client
        .send(Message::Text(
            json!({"id":3,"method":"turn/steer","params":{
                "threadId":thread_id,"expectedTurnId":"turn-1",
                "input":[{"type":"text","text":"second"}]
            }})
            .to_string()
            .into(),
        ))
        .await?;
    client
        .send(Message::Text(
            json!({"id":4,"method":"thread/list","params":{}})
                .to_string()
                .into(),
        ))
        .await?;
    receive_reply(&mut client, 4).await?;
    assert!(
        timeout(Duration::from_millis(500), backend.steer_started.notified())
            .await
            .is_err(),
        "second steer reached the port before the first settled"
    );
    first_gate.notify_one();
    receive_reply(&mut client, 2).await?;
    receive_reply(&mut client, 3).await?;
    {
        let steers = backend.steer_commands.lock().expect("test lock");
        assert_eq!(steers.len(), 2);
        assert_eq!(
            steers[0].content,
            vec![CommandContent::text("first".into())?]
        );
        assert_eq!(
            steers[1].content,
            vec![CommandContent::text("second".into())?]
        );
    }

    let second_gate = Arc::new(Notify::new());
    *backend.steer_gate.lock().expect("test lock") = Some(Arc::clone(&second_gate));
    client
        .send(Message::Text(
            json!({"id":5,"method":"turn/steer","params":{
                "threadId":thread_id,"expectedTurnId":"turn-1",
                "input":[{"type":"text","text":"third"}]
            }})
            .to_string()
            .into(),
        ))
        .await?;
    backend.steer_started.notified().await;
    client
        .send(Message::Text(
            json!({"id":6,"method":"turn/interrupt","params":{
                "threadId":thread_id,"turnId":"turn-1"
            }})
            .to_string()
            .into(),
        ))
        .await?;
    client
        .send(Message::Text(
            json!({"id":7,"method":"thread/list","params":{}})
                .to_string()
                .into(),
        ))
        .await?;
    receive_reply(&mut client, 7).await?;
    assert_eq!(
        backend.turn_command_order.lock().expect("test lock").len(),
        2,
        "interrupt overtook a pending steer"
    );
    second_gate.notify_one();
    receive_reply(&mut client, 5).await?;
    receive_reply(&mut client, 6).await?;
    assert_eq!(
        *backend.turn_command_order.lock().expect("test lock"),
        ["steer", "steer", "steer", "interrupt"]
    );
    client.close(None).await?;
    server.await?;
    Ok(())
}
