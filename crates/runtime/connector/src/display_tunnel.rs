//! Each display viewer gets one locally authenticated shared RFB connection.

use crate::roles::environment::EnvironmentRole;
use orch_link::tunnels::channel::IncomingTunnel;
use orch_link_contract::tunnels::{OpenErrorCode, Service};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serves raw display bytes after answering server authentication inside the environment.
pub async fn serve(role: &EnvironmentRole, incoming: IncomingTunnel) {
    let id = match incoming.service.parse::<Service>() {
        Ok(Service::Rfb { display_id }) => display_id,
        _ => {
            incoming.refuse(
                OpenErrorCode::NoSuchService,
                "a display is reached under rfb/<id>",
            );
            return;
        }
    };
    if role
        .services()
        .policy
        .policy()
        .is_some_and(|p| !p.environment_vnc)
    {
        incoming.refuse(
            OpenErrorCode::Refused,
            "displays are switched off on this machine",
        );
        return;
    }
    let mut found = None;
    for environment in role.host().environments() {
        if environment
            .displays()
            .list()
            .await
            .iter()
            .any(|d| d.display_id == id)
        {
            found = Some(std::sync::Arc::clone(environment.displays()));
            break;
        }
    }
    let Some(registry) = found else {
        incoming.refuse(
            OpenErrorCode::NoSuchService,
            "the display no longer runs here",
        );
        return;
    };
    let connection = match registry.connect(id).await {
        Ok(connection) => connection,
        Err(_) => {
            incoming.refuse(
                OpenErrorCode::Refused,
                "the display could not be authenticated",
            );
            return;
        }
    };
    let Ok(mut tunnel) = incoming.accept() else {
        return;
    };
    let mut stream = connection.authenticated.stream;
    let relay = async {
        if tunnel
            .write_all(&connection.authenticated.version)
            .await
            .is_err()
        {
            return;
        }
        // The API completes its own None security handshake. Force shared even for a hostile client.
        if !matches!(tunnel.read_u8().await, Ok(0 | 1)) {
            return;
        }
        if registry.require_allowed().is_err() || stream.write_u8(1).await.is_err() {
            return;
        }
        let (mut local_read, mut local_write) = tokio::io::split(stream);
        let (mut tunnel_read, mut tunnel_write) = tokio::io::split(tunnel);
        // The terminal-sized buffers preserve back-pressure and gate before forwarding any new bytes.
        const READ_BYTES: usize = 16 * 1024;
        let input = async {
            let mut bytes = [0; READ_BYTES];
            loop {
                let n = match tunnel_read.read(&mut bytes).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                if registry.require_allowed().is_err()
                    || role
                        .services()
                        .policy
                        .policy()
                        .is_some_and(|p| !p.environment_vnc)
                {
                    return;
                }
                if local_write.write_all(&bytes[..n]).await.is_err() {
                    return;
                }
            }
        };
        let output = async {
            let mut bytes = [0; READ_BYTES];
            loop {
                let n = match local_read.read(&mut bytes).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                if registry.require_allowed().is_err()
                    || role
                        .services()
                        .policy
                        .policy()
                        .is_some_and(|p| !p.environment_vnc)
                {
                    return;
                }
                if tunnel_write.write_all(&bytes[..n]).await.is_err() {
                    return;
                }
            }
        };
        tokio::select! {_=input=>{},_=output=>{}}
    };
    tokio::select! { _=relay=>{},_=connection.stopped.cancelled()=>{} }
}
