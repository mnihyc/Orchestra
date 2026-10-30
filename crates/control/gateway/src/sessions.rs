//! Active machine sessions and replacement.
//!
//! A machine has one session. When a second one is accepted for it, the first
//! is told that it is replaced and is ended, and from then on only the second
//! counts: its tunnels are the ones the relay knows, its frames are the ones
//! passed on, and it is the one the system is told about. What the first one
//! still had in flight is dropped as an ended session's would be, which the
//! rules of the link make good.
//!
//! The registry also knows the token each session was given, because a
//! machine behind the public edge shows no certificate and presents that
//! token to the requests it makes beside its session.

use crate::Gateway;
use orch_link::framing::Connection;
use orch_link::session::{self, Accepted, EndReason, SessionEvent, SessionSender};
use orch_link_contract::backplane::{BackplaneFrame, MachineDown, MachineUp};
use orch_link_contract::version::ByeCode;
use orch_types::Id;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// One session that stands.
#[derive(Debug, Clone)]
pub(crate) struct SessionEntry {
    pub(crate) session_id: Id,
    pub(crate) sender: SessionSender,
    pub(crate) token: String,
    pub(crate) fingerprint: String,
    pub(crate) ip: Option<IpAddr>,
    pub(crate) placement: String,
    pub(crate) trust: &'static str,
}

#[derive(Default)]
struct Registry {
    by_machine: HashMap<Id, SessionEntry>,
    by_token: HashMap<String, Id>,
}

/// The sessions a gateway holds.
#[derive(Default)]
pub struct Sessions {
    inner: Mutex<Registry>,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("held", &self.len())
            .finish()
    }
}

impl Sessions {
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many sessions stand.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().by_machine.len()
    }

    /// Whether none stands.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Puts a session in a machine's place and returns the one it replaced.
    pub(crate) fn install(&self, machine: Id, entry: SessionEntry) -> Option<SessionEntry> {
        let mut registry = self.lock();
        metrics::gauge!("orch_link_sessions","placement"=>entry.placement.clone(),"trust"=>entry.trust).increment(1.0);
        registry.by_token.insert(entry.token.clone(), machine);
        let replaced = registry.by_machine.insert(machine, entry);
        if let Some(old) = &replaced {
            metrics::gauge!("orch_link_sessions","placement"=>old.placement.clone(),"trust"=>old.trust).decrement(1.0);
            registry.by_token.remove(&old.token);
        }
        replaced
    }

    /// Takes a session out, if it is still the machine's. Returns whether it
    /// was.
    pub(crate) fn remove_if(&self, machine: Id, session_id: Id) -> bool {
        let mut registry = self.lock();
        let ours = registry
            .by_machine
            .get(&machine)
            .is_some_and(|entry| entry.session_id == session_id);
        if ours && let Some(entry) = registry.by_machine.remove(&machine) {
            metrics::gauge!("orch_link_sessions","placement"=>entry.placement.clone(),"trust"=>entry.trust).decrement(1.0);
            registry.by_token.remove(&entry.token);
        }
        ours
    }

    /// The way to send to a machine's session.
    pub(crate) fn sender_of(&self, machine: Id) -> Option<SessionSender> {
        self.lock()
            .by_machine
            .get(&machine)
            .map(|entry| entry.sender.clone())
    }

    /// The machine a session token was given to, and the fingerprint of the
    /// certificate that session was opened with.
    pub(crate) fn machine_of_token(&self, token: &str) -> Option<(Id, String)> {
        let registry = self.lock();
        let machine = *registry.by_token.get(token)?;
        let entry = registry.by_machine.get(&machine)?;
        Some((machine, entry.fingerprint.clone()))
    }

    /// Every session that stands, as the system is told of it.
    pub(crate) fn announcements(&self) -> Vec<MachineUp> {
        self.lock()
            .by_machine
            .iter()
            .map(|(machine, entry)| MachineUp {
                machine_id: *machine,
                session_id: entry.session_id,
                ip: entry
                    .ip
                    .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
            })
            .collect()
    }

    /// Ends a machine's session now, telling the machine why.
    pub(crate) fn kick(&self, machine: Id, code: ByeCode) {
        if let Some(sender) = self.sender_of(machine) {
            sender.bye(code);
        }
    }

    /// Ends every session.
    pub(crate) fn end_all(&self) {
        let senders: Vec<SessionSender> = self
            .lock()
            .by_machine
            .values()
            .map(|entry| entry.sender.clone())
            .collect();
        for sender in senders {
            sender.end();
        }
    }

    /// Observes the actual gateway-side link meters, without machine identifiers in labels.
    pub(crate) fn sample_round_trips(&self) {
        let samples: Vec<_> = self
            .lock()
            .by_machine
            .values()
            .map(|entry| (entry.placement.clone(), entry.sender.clone()))
            .collect();
        for (placement, sender) in samples {
            if let Some(ms) = sender.meter().measurements().round_trip_ms {
                metrics::histogram!("orch_link_round_trip_seconds","placement"=>placement)
                    .record((ms.max(0) as f64) / 1000.0);
            }
        }
    }
}

/// Accepts the session on a connection that was just upgraded, and serves it
/// until it ends.
pub(crate) async fn serve_connection(gateway: Gateway, connection: Connection) {
    let authority = crate::authentication::BackplaneAuthority::new(&gateway);
    let config = gateway.session_config();
    let accepted = session::accept(connection, &config, &authority, gateway.clock().as_ref()).await;
    match accepted {
        Ok(accepted) => serve_session(gateway, accepted).await,
        Err(error) => tracing::debug!(%error, "a session was not accepted"),
    }
}

async fn serve_session(gateway: Gateway, accepted: Accepted) {
    let Accepted {
        auth,
        session_id,
        session_token,
        fingerprint,
        remote_ip,
        tls,
        placement,
        sender,
        mut inbox,
        ..
    } = accepted;
    let machine = auth.machine_id;
    let inner = &gateway.inner;
    let replaced = inner.sessions.install(
        machine,
        SessionEntry {
            session_id,
            sender: sender.clone(),
            token: session_token,
            fingerprint,
            ip: remote_ip,
            placement: placement.unwrap_or_else(|| "unknown".to_owned()),
            trust: if tls == orch_link_contract::session::TlsMode::Mutual {
                "pinned"
            } else {
                "public"
            },
        },
    );
    if let Some(old) = replaced {
        // The old session's tunnels wait for the new one; the old session is
        // told why it ends.
        inner
            .relay
            .with(|relay| relay.session_gone(machine, old.session_id));
        old.sender.bye(ByeCode::Replaced);
        tracing::info!(%machine, "a session replaced the one before it");
    }
    inner
        .relay
        .with(|relay| relay.session_up(machine, session_id, sender.clone()));
    inner.backplane.send(BackplaneFrame::MachineUp(MachineUp {
        machine_id: machine,
        session_id,
        ip: remote_ip.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
    }));
    tracing::info!(%machine, session = %session_id, "a session stands");

    let mut stopping = false;
    let reason = loop {
        let event = tokio::select! {
            () = inner.shutdown.cancelled(), if !stopping => {
                stopping = true;
                sender.end();
                continue;
            }
            event = inbox.next() => event,
        };
        match event {
            Some(SessionEvent::Frame(frame)) => {
                crate::routing::from_machine(&gateway, machine, session_id, *frame);
            }
            Some(SessionEvent::Data(data)) => {
                crate::routing::data_from_machine(&gateway, machine, session_id, data);
            }
            Some(SessionEvent::Datagram(packet)) => {
                gateway
                    .inner
                    .relay
                    .with(|relay| relay.on_datagram(machine, session_id, packet));
            }
            Some(SessionEvent::Ended(reason)) => break reason,
            None => break EndReason::Local,
        }
    };
    inner.sessions.remove_if(machine, session_id);
    inner
        .relay
        .with(|relay| relay.session_gone(machine, session_id));
    inner
        .backplane
        .send(BackplaneFrame::MachineDown(MachineDown {
            machine_id: machine,
            session_id,
            reason: describe(&reason),
        }));
    tracing::info!(%machine, session = %session_id, reason = %describe(&reason), "a session ended");
}

/// A session's ending, as the system is told.
fn describe(reason: &EndReason) -> String {
    match reason {
        EndReason::PeerClosed => "the machine closed the connection".to_owned(),
        EndReason::Bye(code) => format!("ended with {code:?}"),
        EndReason::Dead => "the machine went silent".to_owned(),
        EndReason::Protocol(why) => format!("the protocol was broken: {why}"),
        EndReason::Transport(why) => format!("the connection failed: {why}"),
        EndReason::Local => "the gateway ended it".to_owned(),
    }
}
