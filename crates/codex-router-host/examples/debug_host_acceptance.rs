//! Real debug Host discovery/configuration/owned-thread proof. Never selects a user thread.
#[path = "debug_host_acceptance/acp_client_proof.rs"]
mod acp_client_proof;
#[path = "debug_host_acceptance/acp_permission_proof.rs"]
mod acp_permission_proof;
#[path = "debug_host_acceptance/acp_replacement_proof.rs"]
mod acp_replacement_proof;
#[path = "debug_host_acceptance/agent_tool_proof.rs"]
mod agent_tool_proof;
#[path = "debug_host_acceptance/cli_event_listener.rs"]
mod cli_event_listener;
#[path = "debug_host_acceptance/cli_message_proof.rs"]
mod cli_message_proof;
#[path = "debug_host_acceptance/lifecycle_reader_proof.rs"]
mod lifecycle_reader_proof;
#[path = "debug_host_acceptance/native_delivery_proof.rs"]
mod native_delivery_proof;
#[path = "debug_host_acceptance/owned_thread_registry.rs"]
mod owned_thread_registry;
#[path = "debug_host_acceptance/process_identity_guard.rs"]
mod process_identity_guard;
#[path = "debug_host_acceptance/proof_environment_settings.rs"]
mod proof_environment_settings;
#[path = "debug_host_acceptance/recovery_observation.rs"]
mod recovery_observation;

enum MessageProof {
    LifecycleReaders,
    None,
    Native,
    Cli(PathBuf),
    Agents(PathBuf),
    Acp(PathBuf),
    AcpCli(PathBuf),
    AcpPermission(PathBuf),
    AcpPermissionRace(PathBuf),
    Delivery(PathBuf),
    AcpReplacement(PathBuf),
    Recovery,
}

use codex_native_integration::{DebugCodexProfile, NativeProtocolConnection};
use codex_router_host::ProcessGroupChild;
use communication_client::ControlClient;
use communication_protocol::{ChannelDescription, EndpointAvailability};
use owned_thread_registry::OwnedThreadRegistry;
use process_identity_guard::capture_production_identity;
use serde_json::json;
use std::{
    error::Error,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let router_cli = std::fs::canonicalize(
        std::env::args_os()
            .nth(1)
            .ok_or("expected built debug router CLI path")?,
    )?;
    let messages = match std::env::args().nth(2).as_deref() {
        None => MessageProof::None,
        Some("--lifecycle-readers") => MessageProof::LifecycleReaders,
        Some("--recovery-hold") => MessageProof::Recovery,
        Some("--acp-replacement") => MessageProof::AcpReplacement(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--acp-replacement requires pinned SDK module path")?,
        )?),
        Some("--messages") => MessageProof::Native,
        Some("--delivery") => MessageProof::Delivery(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--delivery requires built agent-sessions path")?,
        )?),
        Some("--cli-messages") => MessageProof::Cli(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--cli-messages requires built agent-sessions path")?,
        )?),
        Some("--acp-cli") => MessageProof::AcpCli(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--acp-cli requires built agent-sessions path")?,
        )?),
        Some("--acp-client") => MessageProof::Acp(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--acp-client requires pinned SDK module path")?,
        )?),
        Some("--acp-permission") => MessageProof::AcpPermission(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--acp-permission requires pinned SDK module path")?,
        )?),
        Some("--acp-permission-race") => MessageProof::AcpPermissionRace(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--acp-permission-race requires pinned SDK module path")?,
        )?),
        Some("--agent-messages") => MessageProof::Agents(std::fs::canonicalize(
            std::env::args_os()
                .nth(3)
                .ok_or("--agent-messages requires built agent-sessions path")?,
        )?),
        _ => {
            return Err("expected --messages, --cli-messages PATH or --agent-messages PATH".into());
        }
    };
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME unavailable")?);
    let codex_home = home.join(".codex");
    if let Some(selected) = std::env::var_os("CODEX_HOME")
        && std::fs::canonicalize(selected)? != std::fs::canonicalize(&codex_home)?
    {
        return Err("acceptance requires normal Codex home".into());
    }
    let router_root = home.join(".codex-router-debug");
    codex_native_integration::validate_debug_directory(&router_root, &home.join(".codex-router"))?;
    let _profile = DebugCodexProfile::read(&codex_home, 18787)?;
    // Refuse occupied debug routing; never stop a discovered listener.
    drop(std::net::TcpListener::bind((
        std::net::Ipv4Addr::LOCALHOST,
        18787,
    ))?);
    let production_before = capture_production_identity().await?;
    let run_root = PathBuf::from("/tmp").join(format!(
        "debug-host-acceptance-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&run_root)?;
    let backend = run_root.join("backend.sock");
    let log = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(run_root.join("host-diagnostics.log"))?;
    let mut command = Command::new(&router_cli);
    command
        .args([
            "host",
            "--require-debug-isolation",
            "--port",
            "18787",
            "--router-root",
        ])
        .arg(&router_root)
        .env("CODEX_ROUTER_DEBUG_READINESS_TIMING", "1")
        .env("CODEX_HOME", &codex_home)
        .env("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET", &backend)
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    proof_environment_settings::configure_owned_host(&mut command);
    let mut host = ProcessGroupChild::spawn(&mut command)?;
    println!(
        "{}",
        json!({"kind":"ownedHostStarted","pid":host.process_id(),"evidenceDirectory":run_root})
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(if matches!(messages, MessageProof::Recovery) {
            600
        } else if matches!(messages, MessageProof::Agents(_)) {
            360
        } else {
            180
        }),
        probe(&router_root, &mut host, &messages, &router_cli),
    )
    .await;
    let probe_status = match &outcome {
        Err(_) => "overall_timeout",
        Ok(Err(error)) if error.is::<tokio::time::error::Elapsed>() => "native_observation_timeout",
        Ok(Err(_)) => "probe_failed",
        Ok(Ok(())) => "passed",
    };
    println!("{}", json!({"kind":"probeOutcome","status":probe_status}));
    // Cleanup addresses only the retained child; Host owns termination of its backend children.
    let cleanup = async {
        if host.try_wait()?.is_none() {
            host.send_terminate()?;
            let budget = codex_router_host::APP_SERVER_SHUTDOWN_TOTAL
                + codex_router_host::ROUTER_SHUTDOWN_TIMEOUT
                + Duration::from_secs(10);
            tokio::time::timeout(budget, host.wait()).await??;
        }
        Ok::<_, Box<dyn Error>>(())
    }
    .await;
    let debug_port_closed =
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 18787)).is_ok();
    let debug_backend_closed = std::os::unix::net::UnixStream::connect(&backend).is_err();
    let production_after = capture_production_identity().await?;
    println!(
        "{}",
        json!({"kind":"cleanup","ownedHostExited":cleanup.is_ok(),"debugListenersClosed":debug_port_closed && debug_backend_closed,"productionIdentityUnchanged":production_before==production_after})
    );
    cleanup?;
    if !debug_port_closed || !debug_backend_closed {
        return Err("owned debug listeners remain after Host shutdown".into());
    }
    if production_before != production_after {
        return Err("production identity changed during proof; isolation is unconfirmed".into());
    }
    outcome??;
    Ok(())
}

async fn probe(
    router_root: &Path,
    host: &mut ProcessGroupChild,
    messages: &MessageProof,
    router_cli: &Path,
) -> Result<(), Box<dyn Error>> {
    let directory = router_root.join("agent-communication");
    let endpoint = loop {
        if host.try_wait()?.is_some() {
            return Err("owned Host exited before readiness; inspect private diagnostics".into());
        }
        if let Ok(mut client) =
            ControlClient::connect(&directory, "debug_acceptance", env!("CARGO_PKG_VERSION")).await
            && let Ok(inventory) = client.list_endpoints().await
        {
            let candidate = inventory
                .endpoints
                .into_iter()
                .filter(|endpoint| {
                    String::from(endpoint.endpoint.endpoint_id.clone()) == "codex-local"
                })
                .filter(|endpoint| {
                    matches!(
                        endpoint.availability,
                        EndpointAvailability::Available { .. }
                    )
                })
                .flat_map(|endpoint| endpoint.channels)
                .find_map(|channel| match channel {
                    ChannelDescription::NativeCodex {
                        path,
                        generation: Some(_),
                        ..
                    } => Some(String::from(path)),
                    _ => None,
                });
            if let Some(path) = candidate {
                break path;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let root = std::fs::canonicalize(&directory)?;
    let socket = std::fs::canonicalize(root.join(endpoint))?;
    if socket.parent() != Some(root.as_path()) {
        return Err("native public selector escaped its service directory".into());
    }
    let mut native = NativeProtocolConnection::connect(&socket).await?;
    let configuration = native
        .request("config/read", json!({"includeLayers":false}))
        .await?;
    let config = configuration
        .get("config")
        .ok_or("missing effective configuration")?;
    if config
        .get("model_provider")
        .and_then(serde_json::Value::as_str)
        != Some("codex-router-debug")
        || config
            .pointer("/model_providers/codex-router-debug/base_url")
            .and_then(serde_json::Value::as_str)
            != Some("http://127.0.0.1:18787/v1")
    {
        return Err("backend effective provider routing is not debug".into());
    }
    let remote = native
        .request("remoteControl/status/read", json!({}))
        .await?;
    if remote.get("status").and_then(serde_json::Value::as_str) != Some("disabled") {
        return Err("debug backend Remote Control was not disabled".into());
    }
    println!(
        "{}",
        json!({"kind":"debugRoutingVerified","remoteControl":"disabled"})
    );
    let mut owned = OwnedThreadRegistry::default();
    let cwd = std::env::current_dir()?;
    if matches!(messages, MessageProof::LifecycleReaders) {
        return lifecycle_reader_proof::run_reader_proof(&mut native, &mut owned, &directory).await;
    }
    if let MessageProof::AcpReplacement(sdk) = messages {
        return acp_replacement_proof::run_replacement_proof(
            &mut native,
            &mut owned,
            acp_replacement_proof::ReplacementProofRequest {
                directory: &directory,
                cwd: &cwd,
                router_cli,
                sdk,
                native_socket: &socket,
            },
        )
        .await;
    }
    if let MessageProof::Delivery(executable) = messages {
        return native_delivery_proof::run_delivery_proof(
            &mut native,
            &mut owned,
            native_delivery_proof::DeliveryProofRequest {
                executable,
                directory: &directory,
                cwd: &cwd,
            },
        )
        .await;
    }
    if let MessageProof::AcpPermission(sdk) | MessageProof::AcpPermissionRace(sdk) = messages {
        return acp_permission_proof::run_permission_proof(
            &mut native,
            &mut owned,
            acp_permission_proof::PermissionProofRequest {
                directory: &directory,
                sdk,
                cwd: &cwd,
                competing_responder: matches!(messages, MessageProof::AcpPermissionRace(_)),
            },
        )
        .await;
    }
    let first = if matches!(messages, MessageProof::Agents(_)) {
        owned
            .create_with_socket_access(&mut native, &cwd, &directory.join("control.sock"))
            .await?
    } else {
        owned.create(&mut native, &cwd).await?
    };
    owned.inspect(&mut native, &first).await?;
    let mut peer = NativeProtocolConnection::connect(&socket).await?;
    let second = if matches!(messages, MessageProof::Agents(_)) {
        owned
            .create_with_socket_access(&mut peer, &cwd, &directory.join("control.sock"))
            .await?
    } else {
        owned.create(&mut peer, &cwd).await?
    };
    owned.inspect(&mut peer, &second).await?;
    for id in [&first, &second] {
        println!(
            "{}",
            json!({"kind":"ownedThreadCreated","threadId":id,"durability":"notEstablished"})
        );
    }
    if matches!(messages, MessageProof::Native) {
        let receipt = owned
            .submit_text(
                &mut native,
                &second,
                "Do not use tools or modify files. Reply with exactly: DEBUG_REPLY_B",
            )
            .await?;
        let reply = owned.observe_text(&mut peer, receipt).await?;
        if reply.trim() != "DEBUG_REPLY_B" {
            return Err("first owned reply did not match the proof marker".into());
        }
        let receipt = owned.submit_text(&mut peer, &first, &format!("A separate thread returned this ordinary message: {reply}. Do not use tools or modify files. Reply with exactly: DEBUG_REPLY_A")).await?;
        let returned = owned.observe_text(&mut native, receipt).await?;
        if returned.trim() != "DEBUG_REPLY_A" {
            return Err("explicit return did not match the proof marker".into());
        }
        println!(
            "{}",
            json!({"kind":"ownedMessageExchangePassed","proof":"two native clients, two turns and client-explicit return; no autonomous agent CLI or TUI recovery claim"})
        );
    }
    if let MessageProof::Cli(executable) = messages {
        let mut control =
            ControlClient::connect(&directory, "cli-proof", env!("CARGO_PKG_VERSION")).await?;
        let inventory = control.list_endpoints().await?;
        let endpoint = inventory
            .endpoints
            .into_iter()
            .find(|endpoint| String::from(endpoint.endpoint.endpoint_id.clone()) == "codex-local")
            .ok_or("missing endpoint")?
            .endpoint;
        let target = communication_protocol::SessionRef {
            endpoint: endpoint.clone(),
            session_id: second.clone().try_into()?,
        };
        let sender = communication_protocol::SessionRef {
            endpoint,
            session_id: first.clone().try_into()?,
        };
        owned.require_owned(&second)?;
        let turn = cli_message_proof::submit(cli_message_proof::CliSubmission {
            executable,
            directory: &directory,
            target: &target,
            sender: &sender,
            text: "Do not use tools or modify files. Reply with exactly: DEBUG_CLI_REPLY_B",
        })
        .await?;
        let reply = owned.observe_turn(&mut peer, &second, &turn).await?;
        if reply.trim() != "DEBUG_CLI_REPLY_B" {
            return Err("CLI target reply mismatch".into());
        }
        owned.require_owned(&first)?;
        let turn = cli_message_proof::submit(cli_message_proof::CliSubmission { executable, directory:&directory, target:&sender, sender:&target, text:"Do not use tools or modify files. A separate thread replied. Reply with exactly: DEBUG_CLI_REPLY_A" }).await?;
        let reply = owned.observe_turn(&mut native, &first, &turn).await?;
        if reply.trim() != "DEBUG_CLI_REPLY_A" {
            return Err("CLI return reply mismatch".into());
        }
        control.close().await?;
        println!(
            "{}",
            json!({"kind":"ownedCliExchangePassed","proof":"built CLI to public Control, two Luna turns, explicit CLI return; autonomous agent tool use not yet proven"})
        );
    }
    if let MessageProof::Agents(executable) = messages {
        let mut control =
            ControlClient::connect(&directory, "agent-proof", env!("CARGO_PKG_VERSION")).await?;
        let endpoint = control
            .list_endpoints()
            .await?
            .endpoints
            .into_iter()
            .find(|e| String::from(e.endpoint.endpoint_id.clone()) == "codex-local")
            .ok_or("missing endpoint")?
            .endpoint;
        let first_ref = communication_protocol::SessionRef {
            endpoint: endpoint.clone(),
            session_id: first.clone().try_into()?,
        };
        let second_ref = communication_protocol::SessionRef {
            endpoint,
            session_id: second.clone().try_into()?,
        };
        let mut denied = agent_tool_proof::DeniedTargets::bind()?;
        let task = agent_tool_proof::initial_task(
            executable,
            &directory,
            &first_ref,
            &second_ref,
            &mut denied,
        )?;
        owned.require_owned(&second)?;
        let _receipt = owned.submit_text(&mut native, &first, &task).await?;
        agent_tool_proof::observe(&mut native, &mut peer, &first, &second).await?;
        control.close().await?;
    }
    acp_client_proof::run_client_proof(
        &owned,
        &mut peer,
        acp_client_proof::ClientProofRequest {
            messages,
            second: &second,
            cwd: &cwd,
            directory: &directory,
        },
    )
    .await?;
    if matches!(messages, MessageProof::Recovery) {
        let receipt = owned
            .submit_text(
                &mut native,
                &first,
                "Do not use tools. Reply exactly TUI_RECOVERY_READY.",
            )
            .await?;
        if owned.observe_text(&mut native, receipt).await?.trim() != "TUI_RECOVERY_READY" {
            return Err("recovery preparation failed".into());
        }
        println!(
            "{}",
            json!({"kind":"recoveryHoldReady","threadId":first,"serviceDirectory":directory,"nativeSocket":socket,"model":"gpt-5.6-luna"})
        );
        recovery_observation::hold(&directory, &socket, &first).await?;
        println!(
            "{}",
            json!({"kind":"recoveryHoldReleased","proof":"TTY evidence is separate; release alone proves no recovery"})
        );
    }
    println!(
        "{}",
        json!({"kind":"debugHostProbePassed","proof":"real discovery, routing, remote-disable and fresh-thread ownership; message proof reported separately; recovery unproven"})
    );
    Ok(())
}
