//! The shared browser tunnel, carrying live pictures and checked input without a spool.

use crate::roles::environment::EnvironmentRole;
use bytes::BytesMut;
use orch_envhost::browser_screen::{BrowserInput, BrowserStream};
use orch_link::tunnels::channel::IncomingTunnel;
use orch_link_contract::tunnels::{ExecMessage, ExecOpcode, OpenErrorCode, Service};
use orch_realtime_contract::browser::BrowserClientFrame;
use orch_types::Json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A maximum viewport JPEG fits comfortably; anything larger is refused before allocation.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// A small read passes input at once and handles pictures cut into tunnel packets.
const READ_BYTES: usize = 16 * 1024;

/// Serves the browser named by a verified API tunnel ticket while the local policy permits it.
pub async fn serve(role: &EnvironmentRole, incoming: IncomingTunnel) {
    let id = match incoming.service.parse::<Service>() {
        Ok(Service::Browser { browser_id }) => browser_id,
        _ => {
            incoming.refuse(
                OpenErrorCode::NoSuchService,
                "a browser is reached under browser/<id>",
            );
            return;
        }
    };
    if role
        .services()
        .policy
        .policy()
        .is_some_and(|p| !p.environment_browser)
    {
        incoming.refuse(
            OpenErrorCode::Refused,
            "browsers are switched off on this machine",
        );
        return;
    }
    let mut found = None;
    for environment in role.host().environments() {
        if let Some(browser) = environment.browsers().get(id).await {
            found = Some((
                browser,
                environment.files().clone(),
                Arc::clone(environment.browsers()),
            ));
            break;
        }
    }
    let Some((browser, files, registry)) = found else {
        incoming.refuse(
            OpenErrorCode::NoSuchService,
            "the browser no longer runs here",
        );
        return;
    };
    if registry.require_allowed().await.is_err() {
        incoming.refuse(
            OpenErrorCode::Refused,
            "browsers are switched off on this machine",
        );
        return;
    }
    let Ok(tunnel) = incoming.accept() else {
        return;
    };
    let (mut reader, mut writer) = tokio::io::split(tunnel);
    let mut stream = BrowserStream::start(browser, files);
    let input = stream.input.clone();
    let upload = async {
        while let Some(output) = stream.output.recv().await {
            let Ok(wire) = output.wire() else {
                return;
            };
            let mut remaining = wire.as_ref();
            while !remaining.is_empty() {
                let Ok(n) = writer.write(remaining).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                stream.load.handed(n as u64);
                remaining = &remaining[n..];
            }
            stream.handed(&output, 0).await;
        }
    };
    let download = async {
        let mut inbox = BytesMut::new();
        let mut read = [0; READ_BYTES];
        loop {
            let Ok(n) = reader.read(&mut read).await else {
                return;
            };
            if n == 0 {
                return;
            }
            inbox.extend_from_slice(&read[..n]);
            loop {
                let message = match ExecMessage::take(&mut inbox, MAX_MESSAGE) {
                    Ok(Some(message)) => message,
                    Ok(None) => break,
                    Err(_) => return,
                };
                if message.opcode != ExecOpcode::Text {
                    return;
                }
                let Ok(mut value) = serde_json::from_slice::<Json>(&message.payload) else {
                    return;
                };
                let control_watch = value
                    .as_object_mut()
                    .and_then(|m| m.remove("control"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let Ok(frame) = serde_json::from_value::<BrowserClientFrame>(value) else {
                    return;
                };
                if registry.require_allowed().await.is_err() {
                    return;
                }
                if input
                    .send(BrowserInput {
                        frame,
                        control_watch,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    };
    tokio::select! {_=upload=>{},_=download=>{}}
}

use std::sync::Arc;
