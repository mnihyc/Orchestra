//! Environment measurements and directory watches shared by transient feed subscribers.
//!
//! The coordinator combines the two measurement switches into one command per environment.
//! Directory watch identifiers derive from the feed, so a coordinator that replaces another
//! reuses the machine's watch rather than leaving a duplicate behind.

use super::ephemeral::{Coordinator, Message, broadcast, merge_directory};
use super::{Fail, Inner};
use orch_db::Conn;
use orch_db::queries::{environments, resources};
use orch_link_contract::commands::{
    FsList, FsListResult, FsUnwatch, GitStatus, SerialKey, WatchSet,
};
use orch_link_contract::streams::Eph;
use orch_realtime_contract::feeds::{DirectoryWatchError, EventData, Feed};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::time::Instant;

/// Samples are requested at the five-second interval carried by the browser protocol.
const INTERVAL_MS: u32 = 5_000;
/// Presence and changed attachments are rechecked once a second, as the access epoch is.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// Reserve half of the existing frame for envelopes; directory page/reset data uses the other half.
const DIRECTORY_BYTES: usize = crate::backplane::MAX_BACKPLANE_FRAME_BYTES / 2;

#[derive(Clone, PartialEq, Eq)]
struct Wanted {
    machine: Id,
    session: Option<Id>,
    generation: i32,
    stats: bool,
    procs: bool,
    ports: bool,
    terminals: bool,
}
struct Directory {
    machine: Id,
    session: Option<Id>,
    generation: i32,
    watch: Id,
    activation: Option<Activation>,
}

struct Activation {
    command: Id,
    instance: Id,
    ready: bool,
    legacy: bool,
    buffered: Option<Json>,
    modern_buffer: bool,
    failure: Option<DirectoryWatchError>,
}

/// The outcome belongs to one exact attempted installation, never just the feed's name.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DirectoryCompletion {
    feed: Feed,
    command: Id,
    machine: Id,
    session: Option<Id>,
    generation: i32,
    result: Result<FsListResult, DirectoryWatchError>,
}

fn failed(error: crate::dispatch::RunError) -> DirectoryWatchError {
    let problem = crate::routes::workspace::live::command_failure(error)
        .problem(crate::routes::workspace::problems::Doing::Reading)
        .problem;
    DirectoryWatchError {
        retryable: problem.status >= 500
            || problem.code == orch_api_contract::common::ErrorCode::ConflictStaleRevision,
        code: problem.code,
        reason: problem
            .extra
            .get("reason")
            .and_then(Json::as_str)
            .map(str::to_owned),
        detail: problem.detail,
    }
}

fn watch_error(error: DirectoryWatchError, at: Time) -> Json {
    serde_json::json!({"as_of":at,"reset":false,"truncated":false,"changed":[],"removed":[],"error":error})
}

fn initialization_failed(detail: &str) -> DirectoryWatchError {
    DirectoryWatchError {
        code: orch_api_contract::common::ErrorCode::EnvironmentUnavailable,
        reason: Some("directory_initialization".to_owned()),
        detail: detail.to_owned(),
        retryable: true,
    }
}

fn directory_binding(
    feed: &Feed,
    machine: Id,
    generation: i32,
) -> Result<crate::routes::workspace::pages::Binding, Fail> {
    let Feed::EnvDir {
        environment_id,
        path,
    } = feed
    else {
        return Err(Fail::other("a directory result needs a directory feed"));
    };
    Ok(crate::routes::workspace::pages::Binding {
        environment: *environment_id,
        generation,
        machine: Some(machine),
        path: path.clone(),
        mount: None,
        source: crate::routes::workspace::pages::Source::Live,
    })
}

fn directory_data(
    binding: &crate::routes::workspace::pages::Binding,
    listing: FsListResult,
    at: Time,
) -> Result<Json, Fail> {
    let metadata = listing
        .page
        .map(|page| binding.native(page, at))
        .transpose()
        .map_err(Fail::other)?;
    Ok(
        serde_json::json!({"as_of":metadata.as_ref().map_or(at, |meta| meta.as_of), "reset":true,
        "truncated":listing.truncated,"changed":listing.entries,"removed":[],
        "revision":metadata.as_ref().map(|meta| &meta.revision),
        "next_cursor":metadata.as_ref().and_then(|meta| meta.next_cursor.as_ref()),
        "observed_from":metadata.as_ref().and_then(|meta| meta.observed_from),
        "expires_at":metadata.as_ref().map(|meta| meta.expires_at)}),
    )
}

/// The underlying watches this coordinator put on the machine.
#[derive(Default)]
pub(crate) struct Sources {
    measurements: HashMap<Id, Wanted>,
    directories: HashMap<Feed, Directory>,
    git: HashMap<(Id, String), Directory>,
    git_targets: HashMap<Feed, (Id, String)>,
    online: HashMap<Id, (Id, Option<Id>, i32)>,
    offline: HashSet<Feed>,
    last: Option<Instant>,
}

/// The environment a transient feed belongs to.
pub(crate) fn environment(feed: &Feed) -> Option<Id> {
    match feed {
        Feed::EnvStats(id)
        | Feed::EnvProcs(id)
        | Feed::EnvBrowsers(id)
        | Feed::EnvDisplays(id)
        | Feed::EnvTerminals(id)
        | Feed::EnvPorts(id) => Some(*id),
        Feed::EnvDir { environment_id, .. } | Feed::EnvGit { environment_id, .. } => {
            Some(*environment_id)
        }
        _ => None,
    }
}

/// The current reachable machine, including attachment, session and environment generation.
pub(crate) async fn reachable(
    inner: &Inner,
    conn: &mut Conn,
    id: Id,
) -> Result<Option<(Id, Option<Id>, i32)>, Fail> {
    let Some(link) = inner.ephemeral.link.get() else {
        return Ok(None);
    };
    let Some(env) = environments::workspace::environment(conn, id).await? else {
        return Ok(None);
    };
    let Some(machine) = env.machine_id else {
        return Ok(None);
    };
    let attached = if orch_db::queries::environment_facts::of(conn, id)
        .await?
        .is_some_and(|facts| facts.attaches())
    {
        environments::hosting::connected(conn, id)
            .await?
            .is_some_and(|row| row.attached())
    } else {
        env.status == "running"
    };
    if !attached {
        return Ok(None);
    }
    let Some(machine_row) = resources::machine(conn, machine).await? else {
        return Ok(None);
    };
    let session = inner.ephemeral.session(machine);
    Ok((machine_row.is_active()
        && session != Some(None)
        && (machine_row.online || link.is_online(machine)))
    .then(|| {
        (
            machine,
            session.flatten().or_else(|| link.session_of(machine)),
            env.generation,
        )
    }))
}

// An unreachable machine must not make us forget a watch that still needs to be stopped.
async fn cleanup_reachable(inner: &Inner, machine: Id) -> Result<Option<bool>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let row = resources::machine(&mut conn, machine).await?;
    let Some(row) = row.filter(|row| row.is_active()) else {
        return Ok(None);
    };
    Ok(Some(
        inner.ephemeral.session(machine) != Some(None)
            && (row.online
                || inner
                    .ephemeral
                    .link
                    .get()
                    .is_some_and(|link| link.is_online(machine))),
    ))
}

fn watch_id(feed: &Feed) -> Id {
    let hash = Hash::of(format!("realtime.directory:{feed}").as_bytes());
    let mut bytes = [0; size_of::<Id>()];
    bytes.copy_from_slice(&hash.as_bytes()[..size_of::<Id>()]);
    Id::from_bytes(bytes)
}

impl Sources {
    /// A presence change is reconciled before the next report is accepted.
    pub(crate) fn changed(&mut self) {
        self.last = None;
    }

    /// Whether a feed can currently publish, suppressing every environment event while away.
    pub(crate) fn available(&self, feed: &Feed) -> bool {
        environment(feed)
            .or_else(|| self.git_targets.get(feed).map(|target| target.0))
            .is_none_or(|id| self.online.contains_key(&id))
    }

    /// Reconciles the union of subscribers with what the connector is asked for.
    pub(crate) async fn reconcile(
        &mut self,
        inner: &Inner,
        active: &HashSet<Feed>,
        coordinator: &mut Coordinator,
    ) -> Result<(), Fail> {
        if self.last.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return Ok(());
        }
        self.last = Some(Instant::now());
        let Some(link) = inner.ephemeral.link.get() else {
            return Ok(());
        };
        let mut wanted_ids: HashSet<_> = active.iter().filter_map(environment).collect();
        let mut online = HashMap::new();
        {
            let mut conn = inner.db.acquire().await?;
            self.git_targets.clear();
            for feed in active
                .iter()
                .filter(|feed| matches!(feed, Feed::ThreadGit(_) | Feed::EnvGit { .. }))
            {
                if let Some(target) = super::machine_views::git_target(&mut conn, feed).await? {
                    wanted_ids.insert(target.0);
                    self.git_targets.insert(feed.clone(), target);
                }
            }
            for id in &wanted_ids {
                if let Some(machine) = reachable(inner, &mut conn, *id).await? {
                    online.insert(*id, machine);
                }
            }
        }
        for feed in active {
            let Some(id) =
                environment(feed).or_else(|| self.git_targets.get(feed).map(|target| target.0))
            else {
                continue;
            };
            if !online.contains_key(&id) {
                if self.offline.insert(feed.clone()) {
                    broadcast(
                        &inner.db,
                        &Message::Event {
                            feed: feed.clone(),
                            data: serde_json::json!({"offline": true}),
                        },
                    )
                    .await?;
                }
            } else if self.offline.remove(feed) {
                // Lists held before disconnection become current again only after it returns.
                if let Some(data) = inner.ephemeral.list(feed) {
                    coordinator.queue(feed.clone(), data, false);
                }
                inner.ephemeral.online(feed);
            }
        }
        self.offline.retain(|feed| active.contains(feed));
        self.online = online;
        let mut wanted = HashMap::new();
        for (id, (machine, session, generation)) in &self.online {
            let stats = active.contains(&Feed::EnvStats(*id));
            let procs = active.contains(&Feed::EnvProcs(*id));
            let ports = active.contains(&Feed::EnvPorts(*id));
            let terminals = active.contains(&Feed::EnvTerminals(*id));
            if stats || procs || ports || terminals {
                wanted.insert(
                    *id,
                    Wanted {
                        machine: *machine,
                        session: *session,
                        generation: *generation,
                        stats,
                        procs,
                        ports,
                        terminals,
                    },
                );
            }
        }
        // Stop a superseded machine as well as the last subscriber's watch, where it is reachable.
        let obsolete: Vec<_> = self
            .measurements
            .iter()
            .filter(|(id, was)| wanted.get(id).is_none_or(|now| now.machine != was.machine))
            .map(|(id, was)| (*id, was.clone()))
            .collect();
        for (id, was) in obsolete {
            match cleanup_reachable(inner, was.machine).await? {
                Some(false) => continue,
                None => {
                    self.measurements.remove(&id);
                    continue;
                }
                Some(true) => {}
            }
            link.dispatcher()
                .issue(
                    was.machine,
                    &SerialKey::None,
                    &WatchSet {
                        environment_id: id,
                        stats: false,
                        procs: false,
                        ports: false,
                        terminals: false,
                        interval_ms: INTERVAL_MS,
                    },
                )
                .await
                .map_err(Fail::other)?;
            self.measurements.remove(&id);
        }
        for (id, now) in wanted {
            if self.measurements.get(&id) == Some(&now) {
                continue;
            }
            link.dispatcher()
                .issue(
                    now.machine,
                    &SerialKey::None,
                    &WatchSet {
                        environment_id: id,
                        stats: now.stats,
                        procs: now.procs,
                        ports: now.ports,
                        terminals: now.terminals,
                        interval_ms: INTERVAL_MS,
                    },
                )
                .await
                .map_err(Fail::other)?;
            self.measurements.insert(id, now);
        }
        let obsolete: Vec<_> = self
            .directories
            .iter()
            .filter(|(feed, was)| {
                !active.contains(feed)
                    || environment(feed)
                        .and_then(|id| self.online.get(&id))
                        .is_none_or(|(machine, session, generation)| {
                            *machine != was.machine
                                || *session != was.session
                                || *generation != was.generation
                        })
            })
            .map(|(feed, was)| (feed.clone(), was.machine, was.watch))
            .collect();
        for (feed, machine, watch) in obsolete {
            // Re-listing the same stable watch replaces it atomically on this machine.
            // An unwatch racing that re-list could otherwise remove the replacement.
            let replacing = active.contains(&feed)
                && environment(&feed)
                    .and_then(|id| self.online.get(&id))
                    .is_some_and(|(on, _, _)| *on == machine);
            if replacing {
                self.directories.remove(&feed);
                continue;
            }
            match cleanup_reachable(inner, machine).await? {
                Some(false) => continue,
                None => {
                    self.directories.remove(&feed);
                    continue;
                }
                Some(true) => {}
            }
            link.dispatcher()
                .issue(machine, &SerialKey::None, &FsUnwatch { watch_id: watch })
                .await
                .map_err(Fail::other)?;
            self.directories.remove(&feed);
        }
        for feed in active {
            let Feed::EnvDir {
                environment_id,
                path,
            } = feed
            else {
                continue;
            };
            let Some((machine, session, generation)) = self.online.get(environment_id).copied()
            else {
                continue;
            };
            if self.directories.contains_key(feed) {
                continue;
            }
            let watch = watch_id(feed);
            let instance = new_id();
            let issued = link
                .dispatcher()
                .issue(
                    machine,
                    &SerialKey::None,
                    &FsList {
                        environment_id: *environment_id,
                        mount: None,
                        path: path.clone(),
                        watch_id: Some(watch),
                        watch_instance: Some(instance),
                        page: Some(orch_link_contract::commands::FsListPage {
                            cursor: None,
                            limit: None,
                            generation: Some(i64::from(generation)),
                        }),
                    },
                )
                .await
                .map_err(Fail::other)?;
            self.directories.insert(
                feed.clone(),
                Directory {
                    machine,
                    session,
                    generation,
                    watch,
                    activation: Some(Activation {
                        command: issued.id,
                        instance,
                        ready: false,
                        legacy: false,
                        buffered: None,
                        modern_buffer: false,
                        failure: None,
                    }),
                },
            );
            let sender = inner.ephemeral.sender();
            let link = link.clone();
            let feed = feed.clone();
            let stop = inner.stop.clone();
            let remaining = (issued.deadline.get() - inner.clock.now())
                .to_std()
                .unwrap_or(Duration::ZERO);
            inner.tasks.spawn(async move {
                let dispatcher = link.dispatcher();
                let result = tokio::select! {
                    () = stop.cancelled() => return,
                    result = tokio::time::timeout(remaining, dispatcher.wait_for::<FsList>(issued.id)) => match result {
                        Ok(result) => result.map_err(failed),
                        Err(_) => {
                            let _ = dispatcher.cancel(issued.id).await;
                            Err(initialization_failed("The environment did not finish installing this directory watch."))
                        }
                    },
                };
                let completion = DirectoryCompletion { feed, command: issued.id, machine, session, generation, result };
                let _ = sender.send(Message::DirectoryCompleted { completion: Box::new(completion) }).await;
            });
        }
        // Threads and explicit directory views of the same repository share one native watch.
        let wanted_git: HashSet<_> = self.git_targets.values().cloned().collect();
        let obsolete: Vec<_> =
            self.git
                .iter()
                .filter(|(target, was)| {
                    !wanted_git.contains(*target)
                        || self.online.get(&target.0).is_none_or(
                            |(machine, session, generation)| {
                                *machine != was.machine
                                    || *session != was.session
                                    || *generation != was.generation
                            },
                        )
                })
                .map(|(target, was)| (target.clone(), was.machine, was.watch))
                .collect();
        for (target, machine, watch) in obsolete {
            let replacing = wanted_git.contains(&target)
                && self
                    .online
                    .get(&target.0)
                    .is_some_and(|(on, _, _)| *on == machine);
            if replacing {
                self.git.remove(&target);
                continue;
            }
            match cleanup_reachable(inner, machine).await? {
                Some(false) => continue,
                Some(true) => {
                    link.dispatcher()
                        .issue(machine, &SerialKey::None, &FsUnwatch { watch_id: watch })
                        .await
                        .map_err(Fail::other)?;
                }
                None => {}
            }
            self.git.remove(&target);
        }
        for target in wanted_git {
            if self.git.contains_key(&target) {
                continue;
            }
            let Some((machine, session, generation)) = self.online.get(&target.0).copied() else {
                continue;
            };
            let watch = watch_id(&Feed::EnvGit {
                environment_id: target.0,
                path: target.1.clone(),
            });
            link.dispatcher()
                .issue(
                    machine,
                    &SerialKey::None,
                    &GitStatus {
                        environment_id: target.0,
                        path: Some(target.1.clone()),
                        watch_id: Some(watch),
                    },
                )
                .await
                .map_err(Fail::other)?;
            self.git.insert(
                target,
                Directory {
                    machine,
                    session,
                    generation,
                    watch,
                    activation: None,
                },
            );
        }
        Ok(())
    }

    /// An authenticated machine's report is accepted only for an environment it currently holds.
    pub(crate) async fn report(
        &mut self,
        inner: &Inner,
        machine: Id,
        eph: Eph,
        coordinator: &mut Coordinator,
    ) -> Result<(), Fail> {
        let Some(id) = eph
            .stream
            .strip_prefix("env:")
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return Ok(());
        };
        if self.online.get(&id).is_none_or(|(on, _, _)| *on != machine) {
            return Ok(());
        }
        let (feed, data) = match eph.kind.as_str() {
            "stats.sample" => (Feed::EnvStats(id), eph.body),
            "procs.snapshot" => (Feed::EnvProcs(id), eph.body),
            "terminals.snapshot" => {
                let _: orch_link_contract::commands::PtyStatusResult =
                    serde_json::from_value(eph.body.clone()).map_err(Fail::other)?;
                coordinator.queue(Feed::EnvTerminals(id), eph.body, false);
                return Ok(());
            }
            "ports.snapshot" => {
                let report: orch_link_contract::ports::PortsSnapshot =
                    serde_json::from_value(eph.body).map_err(Fail::other)?;
                if report.snapshot.environment_id == id
                    && self.online.get(&id).is_some_and(|(_, _, generation)| {
                        i64::from(*generation) == report.snapshot.generation
                    })
                {
                    coordinator.queue(
                        Feed::EnvPorts(id),
                        serde_json::json!({"snapshot": report.snapshot}),
                        false,
                    );
                }
                return Ok(());
            }
            "git.changed" => {
                let report: orch_link_contract::events::GitChanged =
                    serde_json::from_value(eph.body).map_err(Fail::other)?;
                let Some((target, _)) = self
                    .git
                    .iter()
                    .find(|(target, watch)| target.0 == id && watch.watch == report.watch_id)
                else {
                    return Ok(());
                };
                let observation = orch_types::new_id();
                inner
                    .engine()?
                    .begin_git_read(id, &target.1, observation, eph.at)
                    .await
                    .map_err(Fail::other)?;
                inner
                    .engine()?
                    .finish_git_read(id, &target.1, observation, eph.at, &report.status)
                    .await
                    .map_err(Fail::other)?;
                for (feed, selected) in &self.git_targets {
                    if selected == target {
                        coordinator.queue(feed.clone(), serde_json::json!({"environment_id": id, "path": target.1, "status": report.status}), false);
                    }
                }
                return Ok(());
            }
            "fs.changed" => {
                let watch = eph.body["watch_id"]
                    .as_str()
                    .and_then(|id| id.parse::<Id>().ok());
                let Some((feed, held)) = self.directories.iter_mut().find(|(feed, held)| {
                    environment(feed) == Some(id) && Some(held.watch) == watch
                }) else {
                    return Ok(());
                };
                let instance = eph.body["watch_instance"]
                    .as_str()
                    .and_then(|value| value.parse::<Id>().ok());
                let binding = directory_binding(feed, held.machine, held.generation)?;
                let metadata = eph
                    .body
                    .get("page")
                    .filter(|page| !page.is_null())
                    .map(|page| serde_json::from_value(page.clone()).map_err(Fail::other))
                    .transpose()?
                    .map(|page| binding.native(page, inner.clock.time()))
                    .transpose()
                    .map_err(Fail::other)?;
                let data = serde_json::json!({"as_of": metadata.as_ref().map_or(eph.at, |meta| meta.as_of),
                    "changed": eph.body["changed"], "removed": eph.body["removed"],
                    "reset": eph.body["reset"].as_bool().unwrap_or(false), "truncated": eph.body["truncated"].as_bool().unwrap_or(false),
                    "invalidated":eph.body["invalidated"].as_bool().unwrap_or(false),
                    "revision":metadata.as_ref().map(|meta| &meta.revision),
                    "next_cursor":metadata.as_ref().and_then(|meta| meta.next_cursor.as_ref()),
                    "observed_from":metadata.as_ref().and_then(|meta| meta.observed_from),
                    "expires_at":metadata.as_ref().map(|meta| meta.expires_at)});
                EventData::decode(feed, data.clone()).map_err(Fail::other)?;
                if let Some(activation) = held.activation.as_mut() {
                    if instance.is_some_and(|instance| instance != activation.instance) {
                        return Ok(());
                    }
                    if activation.ready {
                        if activation.failure.is_some()
                            || (!activation.legacy && instance.is_none())
                        {
                            return Ok(());
                        }
                    } else {
                        if activation.failure.is_some() {
                            return Ok(());
                        }
                        if instance.is_some() && !activation.modern_buffer {
                            activation.buffered = None;
                            activation.modern_buffer = true;
                        } else if instance.is_none() && activation.modern_buffer {
                            return Ok(());
                        }
                        if let Some(buffer) = activation.buffered.as_mut() {
                            merge_directory(buffer, data);
                        } else {
                            activation.buffered = Some(data);
                        }
                        if let Some(buffer) = &activation.buffered
                            && serde_json::to_vec(buffer).map_err(Fail::other)?.len()
                                > DIRECTORY_BYTES
                        {
                            activation.buffered = None;
                            activation.failure = Some(initialization_failed(
                                "The directory changed too much while its watch was starting. Read it again.",
                            ));
                        }
                        return Ok(());
                    }
                }
                (feed.clone(), data)
            }
            _ => return Ok(()),
        };
        EventData::decode(&feed, data.clone()).map_err(Fail::other)?;
        // The connector batches directory changes itself; the API limits later batches.
        let immediate =
            matches!(feed, Feed::EnvStats(_) | Feed::EnvProcs(_)) || data["reset"] == true;
        coordinator.queue(feed, data, immediate);
        Ok(())
    }

    /// Fresh explicit reader demand retries a failed activation, not each reconciliation tick.
    pub(crate) fn retry(&mut self, feed: &Feed) -> bool {
        let failed = self
            .directories
            .get(feed)
            .and_then(|directory| directory.activation.as_ref())
            .is_some_and(|activation| activation.ready && activation.failure.is_some());
        if failed {
            self.directories.remove(feed);
            self.changed();
        }
        failed
    }

    pub(crate) fn completed(
        &mut self,
        inner: &Inner,
        completion: DirectoryCompletion,
        coordinator: &mut Coordinator,
    ) -> Result<(), Fail> {
        let Some(directory) = self.directories.get_mut(&completion.feed) else {
            return Ok(());
        };
        if directory.machine != completion.machine
            || directory.session != completion.session
            || directory.generation != completion.generation
        {
            return Ok(());
        }
        let binding = directory_binding(&completion.feed, directory.machine, directory.generation)?;
        let Some(activation) = directory
            .activation
            .as_mut()
            .filter(|activation| activation.command == completion.command && !activation.ready)
        else {
            return Ok(());
        };
        activation.ready = true;
        let result = match activation.failure.take() {
            Some(error) => Err(error),
            None => completion.result,
        };
        let mut data = match result {
            Ok(listing)
                if listing.watch_instance == Some(activation.instance)
                    || (listing.watch_instance.is_none() && !activation.modern_buffer) =>
            {
                activation.legacy = listing.watch_instance.is_none();
                let mut data = directory_data(&binding, listing, inner.clock.time())?;
                if (activation.legacy || activation.modern_buffer)
                    && let Some(buffer) = activation.buffered.take()
                {
                    merge_directory(&mut data, buffer);
                }
                data
            }
            Ok(_) => {
                let error = initialization_failed(
                    "The environment answered a different directory watch installation.",
                );
                activation.failure = Some(error.clone());
                watch_error(error, inner.clock.time())
            }
            Err(error) => {
                activation.failure = Some(error.clone());
                watch_error(error, inner.clock.time())
            }
        };
        activation.buffered = None;
        if serde_json::to_vec(&data).map_err(Fail::other)?.len() > DIRECTORY_BYTES {
            let error = DirectoryWatchError {
                code: orch_api_contract::common::ErrorCode::RequestTooLarge,
                reason: Some("too_large".to_owned()),
                detail: "The directory reset exceeds the bounded page size.".to_owned(),
                retryable: false,
            };
            activation.failure = Some(error.clone());
            data = watch_error(error, inner.clock.time());
        }
        EventData::decode(&completion.feed, data.clone()).map_err(Fail::other)?;
        coordinator.queue(completion.feed, data, true);
        Ok(())
    }
}
