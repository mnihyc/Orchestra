//! A person's shell attached through a tunnel that ends at the API: what the shell printed last first, then what comes, typing and resizes the other way.
//!
//! A shell is a pseudo-terminal the environment host owns and keeps whether or not anybody looks at it
//! (`pty.open` starts one, `pty.close` ends it). A **tunnel** to it is one attachment: the API opens it with a
//! ticket for `pty/<terminal id>` each time a person's screen takes the shell up, after a restart of the API
//! or the gateway as after a day away. The connector sends the last 256 KiB the shell wrote, then everything
//! that comes; what the person types goes into the shell, and a new size of the screen changes the shell's.
//! When the tunnel ends, the shell goes on; when a second tunnel is opened to the same shell the first is
//! ended and told why; when the shell exits, the tunnel is told the code and closed. Flow control belongs to
//! the tunnel: a screen that does not read stops the shell's output from being taken, and the shell then
//! waits to write, as it would at a terminal that cannot keep up.

use orch_envhost::error::HostError;
use orch_envhost::terminal::{Attachment, Event};
use orch_link::tunnels::channel::{IncomingTunnel, Tunnel, TunnelEvent};
use orch_link_contract::tunnels::{OpenErrorCode, Service};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::roles::environment::EnvironmentRole;

/// How many bytes of a person's typing are taken from the tunnel at a time: a pasted page fits.
const TYPING_BYTES: usize = 16 * 1024;

/// The code told for a shell that was ended by a signal and so has none: a code no process exits with.
const NO_EXIT_CODE: i64 = -1;

/// Takes the tunnel up if it names a shell that is here and that policy lets be reached, and attaches the
/// shell to it until either ends.
pub async fn serve(role: &EnvironmentRole, incoming: IncomingTunnel) {
    let terminal_id = match incoming.service.parse::<Service>() {
        Ok(Service::Pty { terminal_id }) => terminal_id,
        _ => {
            incoming.refuse(
                OpenErrorCode::NoSuchService,
                "this machine serves shells under pty/<terminal id>",
            );
            return;
        }
    };
    // A shell that was opened before its owner switched shells off is no longer reached.
    if role
        .services()
        .policy
        .policy()
        .is_some_and(|policy| !policy.environment_terminal)
    {
        incoming.refuse(
            OpenErrorCode::Refused,
            "shells are switched off on this machine",
        );
        return;
    }
    let attachment = match role.host().pty_attach(terminal_id, true) {
        Ok(attachment) => attachment,
        Err(error) => {
            let code = match error {
                HostError::NotFound(_) => OpenErrorCode::NoSuchService,
                _ => OpenErrorCode::Refused,
            };
            incoming.refuse(code, &error.to_string());
            return;
        }
    };
    match incoming.accept() {
        Ok(tunnel) => bridge(attachment, tunnel).await,
        Err(error) => tracing::debug!(%error, "a tunnel to a shell could not be taken up"),
    }
}

/// Carries what the shell prints into the tunnel and what comes out of the tunnel into the shell.
async fn bridge(mut attachment: Attachment, mut tunnel: Tunnel) {
    let mut events = tunnel.take_events();
    let length = u64::try_from(attachment.replay().len())
        .unwrap_or(u64::MAX)
        .to_be_bytes();
    if tunnel.write_all(&length).await.is_err() {
        return;
    }
    if !attachment.replay().is_empty() && tunnel.write_all(attachment.replay()).await.is_err() {
        return;
    }
    let mut typing = vec![0u8; TYPING_BYTES];
    loop {
        tokio::select! {
            read = tunnel.read(&mut typing) => match read {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if attachment.send_input(typing[..count].to_vec()).is_err() {
                        return;
                    }
                }
            },
            printed = attachment.next() => match printed {
                Some(Event::Data(bytes)) => {
                    if tunnel.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
                Some(Event::Exit { code }) => {
                    tunnel.exited(code.unwrap_or(NO_EXIT_CODE));
                    tunnel.close();
                    return;
                }
                Some(Event::Taken) => {
                    tunnel.fail("another screen took this shell");
                    return;
                }
                None => return,
            },
            told = async {
                match events.as_mut() {
                    Some(events) => events.recv().await,
                    None => std::future::pending().await,
                }
            } => match told {
                Some(TunnelEvent::Resize { cols, rows }) => {
                    if let Err(error) = attachment.resize(cols, rows) {
                        tracing::debug!(%error, "a shell could not be resized");
                    }
                }
                Some(TunnelEvent::Exit { .. }) => {}
                None => events = None,
            },
        }
    }
}
