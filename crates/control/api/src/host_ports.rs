//! A host's ports at the API: the round trip that has its node check a declaration, and the proposal its node
//! last made for the host's page.
//!
//! A declaration is saved only once the host's node has accepted it under its own ceiling (`host.ports`), so a
//! declaration the node refuses, or that no connected node acknowledged, is never saved and never usable. The
//! node's proposal opens nothing: it is kept in this application's state from the node's last answer, with the
//! address the gateway last saw the host connect from as its incoming address where the node knows none (a node
//! behind NAT cannot know its own outside address). A process that has not asked yet shows none, and asks in the
//! background when a manager reads the host.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use orch_api_contract::ports::{HostPorts, HostPortsProposal};
use orch_link_contract::commands::SerialKey;
use orch_link_contract::ports::{HostPortsCheck, HostPortsCheckResult};
use orch_types::Id;

use crate::dispatch::RunError;
use crate::state::App;

/// What the gateway's address is called, where it stands in for what the node could not know.
const SEEN_SOURCE: &str = "the address the system's gateway last saw this host connect from";

/// One application's transient host proposals and its in-flight node reads; clones share that app's cache.
#[derive(Debug, Clone, Default)]
pub struct Proposals {
    inner: Arc<Mutex<Remembered>>,
}

#[derive(Debug, Default)]
struct Remembered {
    proposals: HashMap<Id, HostPortsProposal>,
    asking: HashSet<Id>,
}

impl Proposals {
    fn locked(&self) -> MutexGuard<'_, Remembered> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The last node proposal known to this application; none before a successful answer.
    #[must_use]
    pub fn get(&self, host: Id) -> Option<HostPortsProposal> {
        self.locked().proposals.get(&host).cloned()
    }

    fn remember(&self, host: Id, proposal: HostPortsProposal) {
        self.locked().proposals.insert(host, proposal);
    }
    fn begin(&self, host: Id) -> bool {
        let mut held = self.locked();
        !held.proposals.contains_key(&host) && held.asking.insert(host)
    }
    fn finish(&self, host: Id) {
        self.locked().asking.remove(&host);
    }
}

// Constructed before spawning: cancellation even before the task's first poll permits another read.
struct InFlight {
    proposals: Proposals,
    host: Id,
}
impl Drop for InFlight {
    fn drop(&mut self) {
        self.proposals.finish(self.host);
    }
}

/// What the host's node last proposed, as this application heard it.
#[must_use]
pub fn proposal_of(app: &App, host: Id) -> Option<HostPortsProposal> {
    app.host_ports.get(host)
}

/// Asks the node of `host` (on `machine`) to check `ports` at `revision`, or only to propose when there is
/// none, and keeps its proposal.
pub async fn ask(
    app: &App,
    host: Id,
    machine: Id,
    ports: Option<HostPorts>,
    revision: i64,
) -> Result<HostPortsCheckResult, RunError> {
    let mut answer: HostPortsCheckResult = app
        .link
        .dispatcher()
        .run(
            machine,
            &SerialKey::Machine,
            &HostPortsCheck { ports, revision },
        )
        .await?;
    if answer.proposal.values.incoming_address.is_none() {
        let seen = match app.db.acquire().await {
            Ok(mut conn) => orch_db::queries::resources::machine(&mut conn, machine)
                .await
                .ok()
                .flatten()
                .and_then(|row| row.last_ip),
            Err(_) => None,
        };
        if let Some(address) = seen {
            answer.proposal.values.incoming_address = Some(address);
            SEEN_SOURCE.clone_into(&mut answer.proposal.sources.incoming_address);
        }
    }
    app.host_ports.remember(host, answer.proposal.clone());
    Ok(answer)
}

/// How long a publication's tickets hold. The system signs them anew each time it publishes, which it does again
/// for every held host opening each [`REPUBLISH_EVERY`] and when an API process starts (whose ticket key is new),
/// so a ticket runs out under no standing opening.
const PUBLISH_TICKET_SECS: i64 = 12 * 3_600;
/// How often every held host opening is published again, with fresh tickets: within a ticket's life by hours.
pub const REPUBLISH_EVERY: std::time::Duration = std::time::Duration::from_secs(4 * 3_600);
/// How often the API looks for host nodes that came back (and so how long after it starts it publishes the held
/// openings: time for the gateways and the nodes to come back to it). A node that started again serves its
/// openings within about this long.
pub const REPUBLISH_AFTER_START: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a host opening could not be published or closed on its node.
#[derive(Debug, thiserror::Error)]
pub enum PublishFault {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] orch_db::DbError),
    /// Something the publication needs is not there.
    #[error("{0}")]
    Absent(&'static str),
    /// A ticket could not be signed.
    #[error("{0}")]
    Ticket(String),
    /// The node did not carry it out.
    #[error(transparent)]
    Run(#[from] RunError),
}

/// Publishes the host opening `opening` on its host's node: its reviewed number bound for its transports, with
/// tickets signed now, and records what the node answered.
pub async fn publish(
    app: &App,
    opening: Id,
) -> Result<orch_link_contract::ports::PortPublishResult, PublishFault> {
    use orch_api_contract::ports::{PortCaller, PortConnectionOutside, PortTransport};
    use orch_link_contract::ports::{
        PortPublish, PortPublishState, PortPublishTicket, PortTicketClaims,
    };
    let mut conn = app.db.acquire().await?;
    let row = orch_db::queries::ports::opening(&mut conn, opening)
        .await?
        .ok_or(PublishFault::Absent("the opening is gone"))?;
    let publisher = row.publish_machine_id.ok_or(PublishFault::Absent(
        "the opening has no publishing machine",
    ))?;
    let host_port = row
        .host_port
        .and_then(|number| u16::try_from(number).ok())
        .ok_or(PublishFault::Absent("the opening has no host number"))?;
    let port = u16::try_from(row.port)
        .map_err(|_| PublishFault::Absent("the opening's port is not one"))?;
    let (node, saved, allocation_revision) =
        orch_db::queries::ports::publisher_declaration(&mut conn, publisher)
            .await?
            .ok_or(PublishFault::Absent(
                "the opening's publishing machine is gone",
            ))?;
    let declaration: HostPorts = saved
        .and_then(|saved| serde_json::from_value(saved).ok())
        .ok_or(PublishFault::Absent("ports (environment role): the machine's operator or managers must allow publishing and declare its reachable address and ranges"))?;
    let env =
        orch_db::queries::environments::routes::environments(&mut conn, &[row.environment_id])
            .await?
            .into_iter()
            .next()
            .ok_or(PublishFault::Absent("the opening's environment is gone"))?;
    let machine = env.machine_id.ok_or(PublishFault::Absent(
        "the environment's connector is absent",
    ))?;
    let (program, _) =
        orch_db::queries::ports::program(&mut conn, row.environment_id, port).await?;
    drop(conn);
    let transports: Vec<PortTransport> = row
        .transports
        .iter()
        .filter_map(|word| match word.as_str() {
            "tcp" => Some(PortTransport::Tcp),
            "udp" => Some(PortTransport::Udp),
            _ => None,
        })
        .collect();
    let expires =
        orch_types::Time::new(app.clock.now() + chrono::Duration::seconds(PUBLISH_TICKET_SECS));
    let mut tickets = Vec::with_capacity(transports.len());
    for transport in &transports {
        let claims = PortTicketClaims {
            environment_id: row.environment_id,
            generation: i64::from(env.generation),
            port,
            transport: *transport,
            opening_id: Some(opening),
            audience: Some(orch_api_contract::ports::PortOpeningKind::Host),
            authority_revision: row.revision,
            allocation_revision: Some(allocation_revision),
            host_template: Some(orch_link_contract::ports::HostPortTemplate {
                host_id: row.host_id.unwrap_or(publisher),
                proxy_protocol: program.proxy_protocol,
            }),
            // A host caller is known by its address alone, which the node gives the program in a PROXY header.
            source: PortConnectionOutside {
                address: declaration.incoming_address.clone(),
                port: host_port,
                caller: PortCaller::Address,
            },
        };
        let ticket = app
            .link
            .tickets()
            .port_publish(node, machine, claims, expires)
            .map_err(|error| PublishFault::Ticket(error.to_string()))?;
        tickets.push(PortPublishTicket {
            transport: *transport,
            ticket,
        });
    }
    let body = PortPublish {
        opening_id: opening,
        environment_id: row.environment_id,
        environment_machine_id: machine,
        generation: i64::from(env.generation),
        port,
        transports,
        incoming_address: declaration.incoming_address,
        listen: declaration.listen,
        host_port,
        revision: row.revision,
        allocation_revision,
        proxy_protocol: program.proxy_protocol,
        tickets,
    };
    let answer = app
        .link
        .dispatcher()
        .run(node, &SerialKey::Machine, &body)
        .await?;
    let mut conn = app.db.acquire().await?;
    orch_db::queries::ports::host_published(
        &mut conn,
        opening,
        row.revision,
        answer.state == PortPublishState::Open,
        answer.reason.as_deref(),
    )
    .await?;
    Ok(answer)
}

/// Publishes `opening` in the background, where a request must not wait for a node.
pub fn publish_in_background(app: &App, opening: Id) {
    let app = app.clone();
    tokio::spawn(async move {
        if let Err(error) = publish(&app, opening).await {
            tracing::warn!(%error, %opening, "a host opening could not be published on its node");
        }
    });
}

/// Ends the host opening `opening` on its node, for its revision now: its listeners and connections close and
/// its number is free. An opening that is not a host's needs nothing.
pub async fn unpublish(app: &App, opening: Id) -> Result<bool, PublishFault> {
    use orch_link_contract::ports::PortClose;
    let mut conn = app.db.acquire().await?;
    let Some(row) = orch_db::queries::ports::opening(&mut conn, opening).await? else {
        return Ok(false);
    };
    let Some(node) = row.publish_machine_id else {
        return Ok(false);
    };
    let generation =
        orch_db::queries::environments::routes::environments(&mut conn, &[row.environment_id])
            .await?
            .into_iter()
            .next()
            .map_or(0, |env| i64::from(env.generation));
    drop(conn);
    let answer = app
        .link
        .dispatcher()
        .run(
            node,
            &SerialKey::Machine,
            &PortClose {
                opening_id: opening,
                environment_id: row.environment_id,
                generation,
                revision: row.revision,
            },
        )
        .await?;
    Ok(answer.closed)
}

/// Ends the host openings among `closed` on their nodes, in the background.
pub fn unpublish_in_background(app: &App, closed: &[Id]) {
    for opening in closed.iter().copied() {
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(error) = unpublish(&app, opening).await {
                tracing::debug!(%error, %opening, "a closed host opening was not ended on its node");
            }
        });
    }
}

/// Publishes the held host openings again until `stop`: every one soon after this process starts and then each
/// [`REPUBLISH_EVERY`], so that every node holds tickets of this process's key that have not run out; and those
/// of a node as soon as it comes back, since a node that started again holds nothing.
pub async fn keep_published(app: App, stop: tokio_util::sync::CancellationToken) {
    let mut tick = tokio::time::interval(REPUBLISH_AFTER_START);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick of an interval is at once: the first look waits for the gateways and nodes.
    tick.tick().await;
    let mut whole_at: Option<tokio::time::Instant> = None;
    let mut online_before: std::collections::HashSet<Id> = std::collections::HashSet::new();
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {}
        }
        let online: std::collections::HashSet<Id> =
            app.link.online_machines().into_iter().collect();
        let back: std::collections::HashSet<Id> =
            online.difference(&online_before).copied().collect();
        online_before = online;
        let whole = whole_at.is_none_or(|at| at.elapsed() >= REPUBLISH_EVERY);
        // Stops, starts and moves of environments are followed at every look; the openings a node could not
        // serve are asked again with the whole look or when a node came back.
        follow_environments(&app, whole || !back.is_empty()).await;
        if !whole && back.is_empty() {
            continue;
        }
        if whole {
            whole_at = Some(tokio::time::Instant::now());
        }
        let held = match app.db.acquire().await {
            Ok(mut conn) => orch_db::queries::ports::host_openings_held(&mut conn)
                .await
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        for (opening, machine) in held {
            if !whole && !back.contains(&machine) {
                continue;
            }
            if let Err(error) = publish(&app, opening).await {
                tracing::debug!(%error, %opening, "a held host opening was not published again");
            }
        }
    }
}

/// The statuses of an environment whose unit serves nothing for a while: its host openings give their numbers
/// back. A recreate, a move or a node that is away for a moment keeps them.
const DOWN: [&str; 4] = ["stopped", "disabled", "failed", "removed"];
/// Why a host opening holds no number while its environment is down.
const STOPPED: &str =
    "the environment is stopped: its host number is given back, and taken again when it starts";
/// Why a host opening holds no number after its environment moved, until the new host serves it.
const MOVED: &str =
    "the environment moved to another machine: publishing follows that machine's ports policy";
/// Why a host opening waits on a host that declares no ports.
const NO_DECLARATION: &str = "ports (environment role): the machine's operator or managers must allow publishing and declare its reachable address and ranges";
/// Why an automatic host opening waits: there is no free number.
const NO_NUMBER: &str = "every number its host declares is held by an opening";

/// Follows each standing host opening's environment, so that a number is held only while there is a unit to
/// serve: a stopped environment's opening gives its number back (its node ends the listener, and the number stays
/// the one it prefers), a moved one leaves the old host for the one its unit is on now, and one whose environment
/// runs on its host takes a number again (its preferred one where it is free; another of the declaration for an
/// automatic one, while a fixed one waits for its own) and is published. `retry` asks again for openings that
/// could not be served before.
pub async fn follow_environments(app: &App, retry: bool) {
    let lives = match app.db.acquire().await {
        Ok(mut conn) => orch_db::queries::ports::host_lives(&mut conn)
            .await
            .unwrap_or_default(),
        Err(_) => return,
    };
    for life in lives {
        let moved =
            life.current_machine.is_some() && life.current_machine != life.publish_machine_id;
        let down = DOWN.contains(&life.status.as_str());
        if moved || (down && life.host_allocated) {
            if let Err(error) = give_back(app, &life, moved).await {
                tracing::debug!(%error, opening = %life.id, "a host opening did not give its number back yet");
            }
            continue;
        }
        let waiting = match life.state.as_str() {
            "stopped" => true,
            "unavailable" => retry,
            _ => false,
        };
        if life.host_allocated || life.status != "running" || !waiting {
            continue;
        }
        if let Err(error) = take_again(app, &life).await {
            tracing::debug!(%error, opening = %life.id, "a host opening did not take a number again");
        }
    }
}

/// Ends a host opening's listener on its node and gives its number back, which stays the one it prefers; when
/// `moved`, the opening now stands on the host its environment is on. A node that is there but did not end it is
/// asked again at the next look; one that is away holds nothing once it starts again.
async fn give_back(
    app: &App,
    life: &orch_db::queries::ports::HostLife,
    moved: bool,
) -> Result<(), PublishFault> {
    // Only a node that is there is asked: a command to one that is away would wait for its deadline.
    if life.host_allocated
        && life
            .publish_machine_id
            .is_some_and(|machine| app.link.is_online(machine))
        && let Err(error) = unpublish(app, life.id).await
    {
        return Err(error);
    }
    // Either way it waits as stopped, so that the next look serves it as soon as its unit runs on its host.
    let (reason, host, machine) = if moved {
        (MOVED, life.current_host, life.current_machine)
    } else {
        (STOPPED, life.host_id, life.publish_machine_id)
    };
    let mut conn = app.db.acquire().await?;
    orch_db::queries::ports::host_release(&mut conn, life.id, "stopped", reason, host, machine)
        .await?;
    Ok(())
}

/// Takes a number again for a host opening whose environment runs on its host, and publishes it there.
async fn take_again(
    app: &App,
    life: &orch_db::queries::ports::HostLife,
) -> Result<(), PublishFault> {
    let Some(machine) = life.publish_machine_id else {
        return Ok(());
    };
    let mut conn = app.db.acquire().await?;
    let Some((machine, saved, _)) =
        orch_db::queries::ports::publisher_declaration(&mut conn, machine).await?
    else {
        return Ok(());
    };
    if !app.link.is_online(machine) {
        return Ok(());
    }
    let Some(declared) = saved.and_then(|saved| serde_json::from_value::<HostPorts>(saved).ok())
    else {
        orch_db::queries::ports::host_waits(&mut conn, life.id, NO_DECLARATION).await?;
        return Ok(());
    };
    let held: std::collections::BTreeSet<i32> =
        orch_db::queries::ports::publisher_ports_held(&mut conn, machine)
            .await?
            .into_iter()
            .collect();
    let within = |number: i32| {
        u16::try_from(number).is_ok_and(|number| {
            declared
                .ranges
                .iter()
                .any(|range| range.from <= number && number <= range.to)
        })
    };
    let number = match life
        .host_port
        .filter(|number| within(*number) && !held.contains(number))
    {
        Some(number) => number,
        None if life.fixed_host_port => {
            let reason = format!(
                "its fixed number {} is held by another opening on its host or lies outside the host's ranges",
                life.host_port.unwrap_or_default()
            );
            orch_db::queries::ports::host_waits(&mut conn, life.id, &reason).await?;
            return Ok(());
        }
        None => {
            let mut ranges = declared.ranges.clone();
            ranges.sort_by_key(|range| range.from);
            let free = ranges
                .iter()
                .flat_map(|range| range.from..=range.to)
                .map(i32::from)
                .find(|number| *number > 0 && !held.contains(number));
            let Some(number) = free else {
                orch_db::queries::ports::host_waits(&mut conn, life.id, NO_NUMBER).await?;
                return Ok(());
            };
            number
        }
    };
    if !orch_db::queries::ports::host_take(&mut conn, life.id, number).await? {
        return Ok(());
    }
    drop(conn);
    publish(app, life.id).await?;
    Ok(())
}

/// Asks the node of `host` for a proposal in the background, once at a time for a host, when this application has
/// none; a node that does not answer leaves none, and the next reader asks again.
pub fn ask_in_background(app: &App, host: Id, machine: Id) {
    if !app.host_ports.begin(host) {
        return;
    }
    let finished = InFlight {
        proposals: app.host_ports.clone(),
        host,
    };
    let app = app.clone();
    tokio::spawn(async move {
        let _finished = finished;
        if let Err(error) = ask(&app, host, machine, None, 0).await {
            tracing::debug!(%error, %host, "the host's node gave no proposal for its ports");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn applications_never_share_proposals_and_a_cancelled_read_can_be_tried_again() {
        let first = Proposals::default();
        let same_app = first.clone();
        let independent = Proposals::default();
        let host = orch_types::new_id();
        assert!(first.begin(host));
        let cancelled = InFlight {
            proposals: first.clone(),
            host,
        };
        assert!(
            !same_app.begin(host),
            "concurrent readers of one app deduplicate their node read"
        );
        assert!(
            independent.begin(host),
            "another application's read has its own latch"
        );
        assert!(first.get(host).is_none());
        drop(cancelled); // Includes a spawned future cancelled before its first poll.
        assert!(
            same_app.begin(host),
            "an unanswered/cancelled read does not block the next reader forever"
        );
        let completed = InFlight {
            proposals: same_app.clone(),
            host,
        };
        let proposal: HostPortsProposal = serde_json::from_value(json!({
            "values":{"incoming_address":"node.example","listen":"all","ranges":[]},
            "sources":{"incoming_address":"node","listen":"node","ranges":"node"},
            "as_of":"2026-10-15T00:00:00.000Z","reason":null,
            "limits":{"incoming_address":null,"listen":null,"ranges":null,"revision":1}
        }))
        .unwrap();
        first.remember(host, proposal.clone());
        drop(completed);
        assert_eq!(same_app.get(host), Some(proposal));
        assert!(
            !first.begin(host),
            "a successful proposal is reused by later readers of this app"
        );
        assert!(
            independent.get(host).is_none(),
            "an equal host id cannot disclose another app's proposal"
        );
        independent.finish(host);
    }
}
