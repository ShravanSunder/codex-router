//! Timestamped debug recovery observations; never resumes or submits to the target.
//!
//! The endpoint directory is read through the collaboration API every half second; each new
//! publication sequence is one timeline event.
use codex_native_integration::NativeProtocolConnection;
use collaboration_client::CollaborationClient;
use serde_json::{Value, json};
use std::{error::Error, path::Path, time::Duration};
use tokio::io::AsyncBufReadExt;

const DIRECTORY_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub async fn hold(directory: &Path, socket: &Path, thread: &str) -> Result<(), Box<dyn Error>> {
    let client =
        CollaborationClient::connect(directory, "recovery-timeline", env!("CARGO_PKG_VERSION"))
            .await?;
    let started = std::time::Instant::now();
    let snapshot = client.list_endpoints().await?;
    println!(
        "{}",
        json!({"kind":"recoveryTimelineInitial","elapsedMs":0,"endpoints":snapshot.endpoints})
    );
    let mut sequence = snapshot.sequence;
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut poll = tokio::time::interval(DIRECTORY_POLL_INTERVAL);
    loop {
        tokio::select! {
            line=input.next_line()=>{
                if line?.as_deref()!=Some("finish"){return Err("unexpected recovery control input".into());}
                break;
            },
            _=poll.tick()=>{
                let current=client.list_endpoints().await?;
                if current.sequence==sequence { continue; }
                sequence=current.sequence;
                let event=json!({"sequence":current.sequence,"endpoints":current.endpoints});
                println!("{}",json!({"kind":"recoveryTimelineEvent","elapsedMs":started.elapsed().as_millis(),"event":event}));
                let available=event["endpoints"].as_array().is_some_and(|endpoints| endpoints.iter().any(|endpoint| {
                    endpoint.pointer("/endpoint/endpointId").and_then(Value::as_str)==Some("codex-local")
                        && endpoint.pointer("/availability/state").and_then(Value::as_str)==Some("available")
                }));
                if available {
                    let probe=async {
                        let mut native=NativeProtocolConnection::connect(socket).await?;
                        let _config=native.request("config/read",json!({"includeLayers":false})).await?;
                        let metadata=native.inspect_thread(thread).await?;
                        Ok::<_,codex_native_integration::NativeConnectionError>(metadata)
                    }.await;
                    match probe {
                        Ok(metadata)=>println!("{}",json!({"kind":"replacementMetadataProbe","elapsedMs":started.elapsed().as_millis(),"initialized":true,"threadStatus":metadata.get("status")})),
                        Err(error)=>println!("{}",json!({"kind":"replacementMetadataProbe","elapsedMs":started.elapsed().as_millis(),"initialized":false,"error":error.to_string()})),
                    }
                }
            }
        }
    }
    Ok(())
}
