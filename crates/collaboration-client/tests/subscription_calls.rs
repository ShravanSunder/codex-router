use collaboration_client::{BoardClientError, ControlClient};
use collaboration_protocol::{
    ThreadSubscribeRequest, ThreadSubscriptionState, ThreadSubscriptionsRequest,
    ThreadUnsubscribeRequest,
};
use message_board::{Identity, ResourceIdentity, SubscriptionScope};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

type TestError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), TestError>;

fn ensure(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn actor() -> Value {
    json!({"kind":"human","humanId":"subscription-client"})
}

fn scope() -> Value {
    json!({"kind":"thread","rootMessageId":"01900000-0000-7000-8000-000000000001"})
}

fn view() -> Value {
    json!({"scope":scope(),"policy":{"mode":"poll","whenIdle":"hold",
        "timing":{"quietSeconds":120,"capSeconds":600},"lifetime":86400},
        "state":"active","expiresAt":"2026-10-02T00:00:00Z","pendingCount":2,
        "presence":{"kind":"unreachable","reason":"human reader"}})
}

#[tokio::test]
async fn subscription_methods_exchange_typed_requests_and_views() -> TestResult {
    let (mut client, mut peer) = initialized_client().await?;
    let server = tokio::spawn(async move {
        let subscribe = read_request(&mut peer).await?;
        ensure(
            subscribe["method"] == "board/threadSubscribe",
            "wrong subscribe method",
        )?;
        ensure(
            subscribe["params"]["actor"] == actor(),
            "subscribe actor changed",
        )?;
        ensure(
            subscribe["params"]["scope"] == scope(),
            "subscribe scope changed",
        )?;
        ensure(
            subscribe["params"]["policy"]["quietSeconds"] == 0,
            "policy units changed",
        )?;
        respond(&mut peer, &subscribe, view()).await?;

        let list = read_request(&mut peer).await?;
        ensure(
            list["method"] == "board/threadSubscriptions",
            "wrong subscriptions method",
        )?;
        ensure(
            list["params"] == json!({"actor":actor()}),
            "list request changed",
        )?;
        respond(&mut peer, &list, json!({"subscriptions":[view()]})).await?;

        let unsubscribe = read_request(&mut peer).await?;
        ensure(
            unsubscribe["method"] == "board/threadUnsubscribe",
            "wrong unsubscribe method",
        )?;
        ensure(
            unsubscribe["params"] == json!({"actor":actor(),"scope":scope()}),
            "unsubscribe request changed",
        )?;
        let mut ended = view();
        ended["state"] = json!("ended");
        ended["endReason"] = json!({"kind":"cancelled"});
        respond(&mut peer, &unsubscribe, ended).await
    });

    let subscribed = client
        .board_thread_subscribe(serde_json::from_value::<ThreadSubscribeRequest>(
            json!({"actor":actor(),"scope":scope(),"policy":{"mode":"poll","quietSeconds":0}}),
        )?)
        .await?;
    ensure(
        subscribed.pending_count == 2,
        "subscribe view was not decoded",
    )?;
    let subscriptions = client
        .board_thread_subscriptions(ThreadSubscriptionsRequest {
            actor: serde_json::from_value::<Identity>(actor())?,
        })
        .await?;
    ensure(
        subscriptions.subscriptions.len() == 1,
        "subscription list was not decoded",
    )?;
    let ended = client
        .board_thread_unsubscribe(serde_json::from_value::<ThreadUnsubscribeRequest>(
            json!({"actor":actor(),"scope":scope()}),
        )?)
        .await?;
    ensure(
        ended.state == ThreadSubscriptionState::Ended,
        "unsubscribe did not return ended view",
    )?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn invalid_mutation_results_preserve_thread_or_topic_inspection_identity() -> TestResult {
    for subscription_scope in [
        scope(),
        json!({"kind":"topic","topicId":"01900000-0000-7000-8000-000000000002"}),
    ] {
        let (mut client, mut peer) = initialized_client().await?;
        let server = tokio::spawn(async move {
            let request = read_request(&mut peer).await?;
            respond(&mut peer, &request, json!({"invalid":true})).await
        });
        let scope: SubscriptionScope = serde_json::from_value(subscription_scope)?;
        let expected_resource = match &scope {
            SubscriptionScope::Thread { root_message_id } => ResourceIdentity::Thread {
                root_message_id: root_message_id.clone(),
            },
            SubscriptionScope::Topic { topic_id } => ResourceIdentity::Topic {
                topic_id: topic_id.clone(),
            },
        };
        let outcome = client
            .board_thread_unsubscribe(ThreadUnsubscribeRequest {
                actor: serde_json::from_value(actor())?,
                scope,
            })
            .await;
        match outcome {
            Err(BoardClientError::OutcomeUnknown { resource, .. }) => {
                ensure(
                    resource == expected_resource,
                    "mutation inspection identity changed",
                )?;
            }
            _ => return Err("invalid result did not classify uncertain mutation".into()),
        }
        server.await??;
    }
    Ok(())
}

async fn initialized_client() -> Result<(ControlClient, BufReader<UnixStream>), TestError> {
    let (client_stream, peer_stream) = UnixStream::pair()?;
    let initialize = tokio::spawn(async move {
        ControlClient::initialize(client_stream, "subscription-client-test", "1").await
    });
    let mut peer = BufReader::new(peer_stream);
    let request = read_request(&mut peer).await?;
    respond(
        &mut peer,
        &request,
        json!({"version":{"major":1,"minor":0},
        "serviceId":"00000000-0000-4000-8000-000000000001",
        "serviceEpoch":"00000000-0000-4000-8000-000000000002",
        "controlSchemaDigest":format!("sha256:{}", "a".repeat(64))}),
    )
    .await?;
    Ok((initialize.await??, peer))
}

async fn read_request(peer: &mut BufReader<UnixStream>) -> Result<Value, TestError> {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), peer.read_line(&mut line)).await??;
    Ok(serde_json::from_str(&line)?)
}

async fn respond(peer: &mut BufReader<UnixStream>, request: &Value, result: Value) -> TestResult {
    let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
    peer.get_mut()
        .write_all(format!("{response}\n").as_bytes())
        .await?;
    Ok(())
}
