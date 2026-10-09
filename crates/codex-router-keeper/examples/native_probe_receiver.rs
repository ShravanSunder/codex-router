//! Compiled external stand-in for the future CLI/compiled-fingerprint dispatch.
use codex_router_descriptor_boundary::{DescriptorGate, PipeReader, PipeWriter};
use codex_router_keeper::{NativeProbeError, run_native_probe_receiver};
use codex_router_keeper_protocol::{
    BuildInfo, ComponentFingerprint, ComponentFingerprints, ComponentKind, NativeProbeJobWire,
    PipeFrameReader, ReceiverHelloWire,
};
use std::io::Write;
fn build_info() -> Result<BuildInfo, Box<dyn std::error::Error>> {
    let fingerprint = ComponentFingerprint::from_bytes(&[0x11; 32])?;
    Ok(BuildInfo {
        package_version: "1.2.3".parse()?,
        fingerprints: ComponentFingerprints {
            keeper: fingerprint,
            agent_collaboration_services: fingerprint,
            agent_proxy_services: fingerprint,
            agent_provider_services: fingerprint,
        },
    })
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["build-info", "--json"] {
        println!("{}", serde_json::to_string(&build_info()?)?);
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--fixture-descendant") {
        let _term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        std::future::pending::<()>().await;
        return Ok(());
    }
    let index = args
        .iter()
        .position(|value| value == "--internal-native-probe")
        .ok_or("internal fixture dispatch absent")?;
    let projected =
        ComponentFingerprint::from_hex(args.get(index + 1).ok_or("projected fingerprint absent")?)?;
    let mode = args
        .iter()
        .position(|value| value == "--fixture-mode")
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
        .unwrap_or("native");
    if mode == "native" {
        run_native_probe_receiver(ComponentKind::Keeper, projected, &build_info()?).await?;
        return Ok(());
    }
    if mode == "wrong-compiled-fingerprint" {
        run_native_probe_receiver(
            ComponentKind::Keeper,
            projected,
            &BuildInfo {
                fingerprints: ComponentFingerprints {
                    keeper: ComponentFingerprint::from_bytes(&[0x22; 32])?,
                    ..build_info()?.fingerprints
                },
                ..build_info()?
            },
        )
        .await?;
        return Ok(());
    }
    // Malformed producer modes are test-only alternatives, never a production codec.
    let gate = DescriptorGate::global();
    let output = PipeWriter::inherit_stdout(gate).await?;
    match mode {
        "missing-hello" => {
            drop(output);
            std::future::pending::<()>().await;
            return Ok(());
        }
        "partial-hello" => {
            output.write_all(&[0, 0]).await?;
            drop(output);
            std::future::pending::<()>().await;
            return Ok(());
        }
        "oversized-hello" => {
            output.write_all(&(1_048_577u32).to_be_bytes()).await?;
            drop(output);
            std::future::pending::<()>().await;
            return Ok(());
        }
        "invalid-hello" => {
            output.write_all(&2u32.to_be_bytes()).await?;
            output.write_all(b"{{").await?;
            drop(output);
            std::future::pending::<()>().await;
            return Ok(());
        }
        _ => {}
    }
    write_frame(
        &output,
        &ReceiverHelloWire::Hello {
            parent_role: ComponentKind::Keeper,
            fingerprint: if mode == "wrong-hello" {
                ComponentFingerprint::from_bytes(&[0x22; 32])?
            } else {
                projected
            },
        },
    )
    .await?;
    if mode == "wrong-hello" {
        std::future::pending::<()>().await;
    }
    let mut input = PipeFrameReader::new(PipeReader::inherit_stdin(gate).await?);
    let job = input
        .receive()
        .await?
        .ok_or(NativeProbeError::RecordCount)?
        .decode::<NativeProbeJobWire>()?;
    let typed_job = codex_router_keeper_protocol::NativeProbeJob::from(job);
    if input.receive().await?.is_some() {
        return Err("fixture extra job".into());
    }
    if mode == "backpressure" {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let socket = codex_router_descriptor_boundary::OwnedSocket::connect_unix(
            typed_job.alias().as_path(),
            gate,
        )
        .await?;
        let mut control = codex_router_descriptor_boundary::ReceiptByteStream::new(
            codex_router_descriptor_boundary::UnixReceipt::new(socket),
        );
        control.write_all(b"READY").await?;
        let mut permit = [0];
        control.read_exact(&mut permit).await?;
        if &permit != b"W" {
            return Err("fixture write permit differs".into());
        }
        output.write_all(&1_048_576u32.to_be_bytes()).await?;
        output.write_all(&vec![b' '; 1_048_576]).await?;
        control.write_all(b"DONE").await?;
        std::future::pending::<()>().await;
        return Ok(());
    }
    if mode == "missing-result" {
        drop(output);
        std::future::pending::<()>().await;
        return Ok(());
    }
    if mode == "partial-result" || mode == "oversized-result" {
        output
            .write_all(
                &(if mode == "partial-result" {
                    100u32
                } else {
                    1_048_577u32
                })
                .to_be_bytes(),
            )
            .await?;
        if mode == "partial-result" {
            output.write_all(b"{").await?;
        }
        drop(output);
        std::future::pending::<()>().await;
        return Ok(());
    }
    let server_name = if mode == "large-result" {
        "S".repeat(65_536)
    } else {
        "FIXTURE".to_owned()
    };
    let result = serde_json::json!({"type":"observed","running_version":"0.160.1-fixture","remote_control":{"state":"disabled","server_name":server_name,"environment_id":null}});
    if mode == "large-stderr" {
        std::io::stderr().write_all(&vec![b'e'; 256_000])?;
        std::io::stderr().flush()?;
    }
    let mut capacity_control = if mode == "large-result" {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let socket = codex_router_descriptor_boundary::OwnedSocket::connect_unix(
            typed_job.alias().as_path(),
            gate,
        )
        .await?;
        let mut control = codex_router_descriptor_boundary::ReceiptByteStream::new(
            codex_router_descriptor_boundary::UnixReceipt::new(socket),
        );
        control.write_all(b"READY").await?;
        let mut permit = [0];
        control.read_exact(&mut permit).await?;
        if &permit != b"W" {
            return Err("valid result write permit differs".into());
        }
        Some(control)
    } else {
        None
    };
    write_frame(&output, &result).await?;
    if let Some(control) = &mut capacity_control {
        use tokio::io::AsyncWriteExt;
        control.write_all(b"DONE").await?;
    }
    match mode {
        "extra-result" => write_frame(&output, &result).await?,
        "trailing-result" => output.write_all(b"X").await?,
        "missing-eof" => std::future::pending::<()>().await,
        "nonzero" => {
            drop(output);
            std::process::exit(23);
        }
        "signal" => {
            drop(output);
            rustix::process::kill_process(
                rustix::process::getpid(),
                rustix::process::Signal::KILL,
            )?;
            return Ok(());
        }
        "live-group" => {
            let mut command = tokio::process::Command::new(std::env::current_exe()?);
            command
                .arg("--fixture-descendant")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let child = gate.spawn_child(&mut command).await?;
            eprintln!(
                "LIVE_DESCENDANT {}",
                child.id().ok_or("descendant PID absent")?
            );
            drop(child);
        }
        _ => {}
    }
    drop(output);
    Ok(())
}

async fn write_frame<T: serde::Serialize>(
    output: &PipeWriter,
    message: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec(message)?;
    output
        .write_all(&u32::try_from(bytes.len())?.to_be_bytes())
        .await?;
    output.write_all(&bytes).await?;
    Ok(())
}
