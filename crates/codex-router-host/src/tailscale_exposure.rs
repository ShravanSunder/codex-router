//! One Host's private foreground Serve lease, readiness, failure and retirement.

use crate::{
    RemoteAccess,
    tailscale_process::{OwnedServeProcess, TailscaleCommand},
    tailscale_status::{ServeSessionId, ServeStatus, https_node_name},
};
use collaboration_mcp::ProxyAuthority;
use collaboration_protocol::UuidIdentity;
use std::{io, net::SocketAddr};

#[path = "tailscale_exposure/https_readiness.rs"]
mod https_readiness;
#[cfg(test)]
#[path = "tailscale_exposure/tests.rs"]
mod tests;

/// A validated private authority before any listener advertises acceptance of it.
pub(crate) struct PreparedTailscaleExposure {
    command: TailscaleCommand,
    authority: ProxyAuthority,
}

impl PreparedTailscaleExposure {
    pub(crate) async fn prepare(access: Option<RemoteAccess>) -> io::Result<Option<Self>> {
        Self::prepare_with_command(access, TailscaleCommand::default()).await
    }

    async fn prepare_with_command(
        access: Option<RemoteAccess>,
        command: TailscaleCommand,
    ) -> io::Result<Option<Self>> {
        let Some(RemoteAccess::Tailscale { https_port }) = access else {
            return Ok(None);
        };
        let node_name = https_node_name(&command.capture(&["status", "--json"]).await?)?;
        let before = read_serve_status(&command).await?;
        if before.port_in_use(https_port)? {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "Tailscale Serve/Funnel port {https_port} is already configured; select a free --tailscale-https-port"
                ),
            ));
        }
        Ok(Some(Self {
            command,
            authority: ProxyAuthority::new(node_name, https_port)?,
        }))
    }

    pub(crate) fn authority(&self) -> &ProxyAuthority {
        &self.authority
    }

    pub(crate) async fn start(
        self,
        backend: SocketAddr,
        service_id: &UuidIdentity,
        service_epoch: &UuidIdentity,
    ) -> io::Result<TailscaleExposure> {
        let https_port = self.authority.https_port();
        let node_name = self.authority.node_name();
        let mut process = self.command.spawn_serve(https_port, backend, node_name)?;
        let startup = async {
            process.advertised().await?;
            let after = read_serve_status(&self.command).await?;
            let session_id =
                after.owned_session(node_name, https_port, &format!("http://{backend}"))?;
            let url = format!("https://{node_name}:{https_port}/mcp");
            tokio::select! {
                ready = https_readiness::verify(&url, service_id, service_epoch) => ready?,
                failure = process.failure() => return Err(failure),
            }
            Ok::<_, io::Error>((session_id, url))
        }
        .await;
        match startup {
            Ok((session_id, url)) => Ok(TailscaleExposure {
                command: self.command,
                process,
                session_id,
                url,
            }),
            Err(startup_error) => {
                let stopped = process.stop().await;
                let retired = read_serve_status(&self.command).await.and_then(|status| {
                    if status.port_in_use(https_port)? {
                        Err(io::Error::other("Tailscale startup failed and the selected port remains configured; no foreign configuration was removed"))
                    } else { Ok(()) }
                });
                if let Err(error) = stopped.and(retired) {
                    return Err(io::Error::other(format!(
                        "{startup_error}; cleanup: {error}"
                    )));
                }
                Err(startup_error)
            }
        }
    }
}

pub(crate) struct TailscaleExposure {
    command: TailscaleCommand,
    process: OwnedServeProcess,
    session_id: ServeSessionId,
    url: String,
}

impl TailscaleExposure {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
    pub(crate) async fn failure(&mut self) -> io::Error {
        self.process.failure().await
    }
    pub(crate) async fn shutdown(&mut self) -> io::Result<()> {
        let stopped = self.process.stop().await;
        let retired = read_serve_status(&self.command).await.and_then(|status| {
            if status.has_session(&self.session_id) {
                Err(io::Error::other(
                    "Tailscale foreground Serve session remains after its child was reaped",
                ))
            } else {
                Ok(())
            }
        });
        stopped.and(retired)
    }
}

async fn read_serve_status(command: &TailscaleCommand) -> io::Result<ServeStatus> {
    ServeStatus::parse(&command.capture(&["serve", "status", "--json"]).await?)
}
