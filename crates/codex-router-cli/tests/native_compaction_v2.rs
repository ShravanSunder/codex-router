#![cfg(feature = "keychain-test-support")]

#[path = "support/native_compaction/fixture.rs"]
mod fixture;
#[path = "support/native_compaction/upstream.rs"]
mod upstream;

use codex_native_integration::NativeProtocolConnection;
use fixture::{FixtureProfile, NativeFixture};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::error::Error;
use std::time::Duration;
use upstream::ResponsesFixture;

const EVENT_BOUND: Duration = Duration::from_secs(30);

#[tokio::test]
#[ignore = "real installed Codex 0.160.x and compiled Router, isolated external Responses fixture"]
async fn both_profiles_compact_v2_recover_transport_and_continue() -> Result<(), Box<dyn Error>> {
    for profile in [FixtureProfile::Production, FixtureProfile::Debug] {
        run_journey(profile, false).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "negative control uses real installed Codex and a legacy provider-name fixture"]
async fn legacy_name_selects_inline_instead_of_native_v2() -> Result<(), Box<dyn Error>> {
    run_journey(FixtureProfile::Production, true).await
}

async fn run_journey(profile: FixtureProfile, legacy: bool) -> Result<(), Box<dyn Error>> {
    let upstream = ResponsesFixture::start().await?;
    let mut fixture = match NativeFixture::start(profile, legacy, upstream.address()).await {
        Ok(fixture) => fixture,
        Err(error) => {
            upstream.stop().await?;
            return Err(error);
        }
    };
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(EVENT_BOUND, async {
    let mut client = fixture.connect().await?;
    let result = client.request("thread/start", json!({
        "model": "gpt-5.4", "cwd": fixture.workspace(),
        "approvalPolicy": "never", "sandbox": "read-only", "ephemeral": true
    })).await?;
    let thread = result.pointer("/thread/id").unwrap_or(&Value::Null).as_str().ok_or("missing thread id")?.to_owned();
    complete_turn(&mut client, &thread, "SEED_NATIVE_COMPACTION").await?;
    assert_eq!(client.request("thread/compact/start", json!({"threadId":thread})).await?, json!({}));
    let (compaction_id, compaction_turn) = compaction_started(&mut client, &thread).await?;
    compaction_completed(&mut client, &thread, &compaction_id, &compaction_turn).await?;
    complete_turn(&mut client, &thread, "CONTINUE_NATIVE_COMPACTION").await?;
    let observed = upstream.observations().await?;
    if legacy {
        assert!(!observed.iter().any(|request| request.compaction_trigger));
        assert!(observed.iter().any(|request| request.inline_summary));
    } else {
        let websocket_compactions = observed.iter().filter(|request| request.transport == "websocket" && request.compaction_trigger).count();
        // stream_max_retries=1 permits the original attempt and one reconnect.
        assert_eq!(websocket_compactions, 2, "native retry budget did not exhaust");
        let http_compaction = observed.iter().find(|request| request.transport == "http" && request.compaction_trigger).ok_or("missing HTTP native compaction fallback")?;
        assert!(!http_compaction.content_encoding);
        assert!(http_compaction.parseable_json);
        assert_eq!(http_compaction.path, "/v1/responses");
        assert!(observed.iter().any(|request| request.continuation && request.retained_compaction && request.transport == "http"));
    }
    assert!(observed.iter().all(|request| request.path == "/v1/responses"));
    assert!(observed.iter().all(|request| request.synthetic_pool_authorization));
    eprintln!("native_compaction_receipt profile={profile:?} legacy={legacy} codex={} router={} observations={observed:?}", fixture.version(), env!("CARGO_BIN_EXE_codex-router"));
    Ok::<(), Box<dyn Error>>(())
    })).catch_unwind().await;
    let fixture_cleanup = fixture.stop().await;
    let upstream_cleanup = upstream.stop().await;
    fixture_cleanup?;
    upstream_cleanup?;
    match outcome {
        Ok(result) => result?,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

async fn next_event(client: &mut NativeProtocolConnection) -> Result<Value, Box<dyn Error>> {
    Ok(tokio::time::timeout(EVENT_BOUND, client.next_message()).await??)
}

async fn complete_turn(
    client: &mut NativeProtocolConnection,
    thread: &str,
    prompt: &str,
) -> Result<(), Box<dyn Error>> {
    let response = client
        .request(
            "turn/start",
            json!({"threadId":thread,"input":[{"type":"text","text":prompt}]}),
        )
        .await?;
    let turn = response
        .pointer("/turn/id")
        .unwrap_or(&Value::Null)
        .as_str()
        .ok_or("missing turn id")?;
    let mut assistant_reply = false;
    loop {
        let event = next_event(client).await?;
        if event.pointer("/method").unwrap_or(&Value::Null) == "item/completed"
            && event.pointer("/params/turnId").unwrap_or(&Value::Null) == turn
            && event.pointer("/params/item/type").unwrap_or(&Value::Null) == "agentMessage"
            && event
                .pointer("/params/item/text")
                .unwrap_or(&Value::Null)
                .as_str()
                .is_some_and(|text| text.contains("NATIVE_COMPACTION_FIXTURE_OK"))
        {
            assistant_reply = true;
        }
        if event.pointer("/method").unwrap_or(&Value::Null) == "turn/completed"
            && event.pointer("/params/turn/id").unwrap_or(&Value::Null) == turn
        {
            assert_eq!(
                event.pointer("/params/turn/status").unwrap_or(&Value::Null),
                "completed"
            );
            assert!(
                assistant_reply,
                "completed native turn omitted fixture assistant response"
            );
            return Ok(());
        }
    }
}

async fn compaction_started(
    client: &mut NativeProtocolConnection,
    thread: &str,
) -> Result<(String, String), Box<dyn Error>> {
    loop {
        let event = next_event(client).await?;
        if event.pointer("/method").unwrap_or(&Value::Null) == "item/started"
            && event.pointer("/params/item/type").unwrap_or(&Value::Null) == "contextCompaction"
        {
            assert_eq!(
                event.pointer("/params/threadId").unwrap_or(&Value::Null),
                thread
            );
            return Ok((
                event
                    .pointer("/params/item/id")
                    .unwrap_or(&Value::Null)
                    .as_str()
                    .ok_or("missing compaction item id")?
                    .to_owned(),
                event
                    .pointer("/params/turnId")
                    .unwrap_or(&Value::Null)
                    .as_str()
                    .ok_or("missing compaction turn id")?
                    .to_owned(),
            ));
        }
    }
}

async fn compaction_completed(
    client: &mut NativeProtocolConnection,
    thread: &str,
    item: &str,
    turn: &str,
) -> Result<(), Box<dyn Error>> {
    let mut item_completed = false;
    loop {
        let event = next_event(client).await?;
        if event.pointer("/method").unwrap_or(&Value::Null) == "item/completed"
            && event.pointer("/params/item/type").unwrap_or(&Value::Null) == "contextCompaction"
        {
            assert_eq!(
                event.pointer("/params/threadId").unwrap_or(&Value::Null),
                thread
            );
            assert_eq!(
                event.pointer("/params/item/id").unwrap_or(&Value::Null),
                item
            );
            assert_eq!(
                event.pointer("/params/turnId").unwrap_or(&Value::Null),
                turn
            );
            item_completed = true;
        }
        if event.pointer("/method").unwrap_or(&Value::Null) == "turn/completed"
            && event.pointer("/params/turn/id").unwrap_or(&Value::Null) == turn
        {
            assert!(item_completed);
            assert_eq!(
                event.pointer("/params/turn/status").unwrap_or(&Value::Null),
                "completed"
            );
            return Ok(());
        }
    }
}
