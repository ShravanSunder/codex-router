//! Timestamped debug recovery observations; never resumes or submits to the target.
use codex_native_integration::NativeProtocolConnection;
use communication_client::ControlClient;
use serde_json::{Value, json};
use std::{error::Error, path::Path};
use tokio::io::AsyncBufReadExt;

pub async fn hold(directory: &Path, socket: &Path, thread: &str) -> Result<(), Box<dyn Error>> {
    let mut control =
        ControlClient::connect(directory, "recovery-timeline", env!("CARGO_PKG_VERSION")).await?;
    let started = std::time::Instant::now();
    let snapshot = control.list_endpoints().await?;
    println!(
        "{}",
        json!({"kind":"recoveryTimelineInitial","elapsedMs":0,"endpoints":snapshot.endpoints})
    );
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line=input.next_line()=>{
                if line?.as_deref()!=Some("finish"){return Err("unexpected recovery control input".into());}
                break;
            },
            event=control.next_notification()=>{
                let event=event?;
                println!("{}",json!({"kind":"recoveryTimelineEvent","elapsedMs":started.elapsed().as_millis(),"event":event}));
                if event.pointer("/params/endpoint/availability/state").and_then(Value::as_str)==Some("available") {
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
    control.close().await?;
    Ok(())
}
