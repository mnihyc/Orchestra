//! Scripted controller driving connector roles.
//!
//! A controller stands for the system's side of one machine: the part of the
//! API that takes a machine's events in and sends it commands. It applies
//! what it is delivered to an in-memory store, as the API applies it to its
//! database, and it answers a reset of a session stream the way the API
//! does, by asking the machine to report the session again. It can fail to
//! commit, hold commits back, and restart, so that a test can see what the
//! link does when the system stumbles.
//!
//! The store is the system's memory. It outlives a restart, which only
//! replaces the link: the cursors and the commands that did not end are read
//! back from it, as the API would read them from its database.

use crate::doubles::link::{Hub, MachineRecord};
use orch_link::commands::{CommandHandle, Progress};
use orch_link::controller::{ControllerLink, Notice};
use orch_link::streams::{DeliveredEvent, Delivery};
use orch_link_contract::commands::{Cmd, CommandBody, SerialKey, SessionResync};
use orch_link_contract::session::Role;
use orch_link_contract::streams::{ResetReason, Stream};
use orch_types::{Clock, Id, Json, Seq, Time, new_id};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An event as the system applied it.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// Its number in its stream.
    pub seq: Seq,
    /// When it happened.
    pub at: Time,
    /// Its kind.
    pub kind: String,
    /// Its body.
    pub body: Json,
}

/// A machine's word that events are gone, as the system recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetRecord {
    /// The stream.
    pub stream: String,
    /// The number it goes on from.
    pub from: Seq,
    /// Why.
    pub reason: ResetReason,
    /// Whether the system asked for the session to be reported again.
    pub resync_requested: bool,
}

/// How the controller commits what it is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMode {
    /// At once.
    Immediate,
    /// Not until [`ScriptedController::release_commits`] is called.
    Held,
}

#[derive(Default)]
struct StoreInner {
    applied: HashMap<String, Vec<Applied>>,
    cursors: BTreeMap<String, Seq>,
    violations: Vec<String>,
    resets: Vec<ResetRecord>,
    commands: HashMap<Id, Cmd>,
    handles: HashMap<Id, CommandHandle>,
    reports: Vec<String>,
    online: Vec<Id>,
}

/// The system's memory: what it applied, how far it got, which commands it
/// issued.
#[derive(Default)]
pub struct Store {
    inner: Mutex<StoreInner>,
    changed: Notify,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

struct Settings {
    commit: Mutex<CommitMode>,
    fail_commits: AtomicUsize,
    held: Mutex<Vec<Delivery>>,
    auto_resync: AtomicBool,
}

/// A controller for one machine.
pub struct ScriptedController {
    machine_id: Id,
    roles: Vec<Role>,
    hub: Hub,
    clock: Arc<dyn Clock>,
    store: Arc<Store>,
    link: Mutex<ControllerLink>,
    settings: Arc<Settings>,
}

impl std::fmt::Debug for ScriptedController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedController")
            .field("machine", &self.machine_id)
            .finish_non_exhaustive()
    }
}

impl ScriptedController {
    /// Registers the machine with the hub as active, and starts applying what
    /// it sends.
    #[must_use]
    pub fn attach(hub: &Hub, clock: Arc<dyn Clock>, machine_id: Id, roles: Vec<Role>) -> Self {
        let store = Arc::new(Store::default());
        let settings = Arc::new(Settings {
            commit: Mutex::new(CommitMode::Immediate),
            fail_commits: AtomicUsize::new(0),
            held: Mutex::new(Vec::new()),
            auto_resync: AtomicBool::new(true),
        });
        let (link, notices) = hub.add_machine(
            machine_id,
            roles.clone(),
            BTreeMap::new(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        tokio::spawn(apply(
            link.clone(),
            notices,
            Arc::clone(&store),
            Arc::clone(&settings),
            Arc::clone(&clock),
        ));
        Self {
            machine_id,
            roles,
            hub: hub.clone(),
            clock,
            store,
            link: Mutex::new(link),
            settings,
        }
    }

    /// The system's link of the machine.
    #[must_use]
    pub fn link(&self) -> ControllerLink {
        lock(&self.link).clone()
    }

    /// What the system applied on a stream, in the order it applied it.
    #[must_use]
    pub fn applied(&self, stream: &str) -> Vec<Applied> {
        lock(&self.store.inner)
            .applied
            .get(stream)
            .cloned()
            .unwrap_or_default()
    }

    /// How far the system has taken a stream in.
    #[must_use]
    pub fn cursor(&self, stream: &str) -> Seq {
        lock(&self.store.inner)
            .cursors
            .get(stream)
            .copied()
            .unwrap_or(0)
    }

    /// What went against the rules of exactly-once, in order: an event applied
    /// twice, applied out of its place, or applied past a hole.
    #[must_use]
    pub fn violations(&self) -> Vec<String> {
        lock(&self.store.inner).violations.clone()
    }

    /// The resets the machine sent.
    #[must_use]
    pub fn resets(&self) -> Vec<ResetRecord> {
        lock(&self.store.inner).resets.clone()
    }

    /// The measurements the machine sent, by their kind.
    #[must_use]
    pub fn reports(&self) -> Vec<String> {
        lock(&self.store.inner).reports.clone()
    }

    /// Waits until `count` events of a stream are applied.
    pub async fn wait_for_applied(&self, stream: &str, count: usize) {
        loop {
            let waiting = self.store.changed.notified();
            if self.applied(stream).len() >= count {
                return;
            }
            waiting.await;
        }
    }

    /// Waits, for as long as `limit` of virtual or real time, until `check`
    /// holds. Returns whether it did.
    pub async fn wait_until(&self, limit: Duration, mut check: impl FnMut(&Self) -> bool) -> bool {
        let wait = async {
            loop {
                let waiting = self.store.changed.notified();
                if check(self) {
                    return;
                }
                waiting.await;
            }
        };
        tokio::time::timeout(limit, wait).await.is_ok()
    }

    /// How many events are held back, delivered and not committed.
    #[must_use]
    pub fn held_events(&self) -> usize {
        lock(&self.settings.held)
            .iter()
            .map(|delivery| delivery.events.len())
            .sum()
    }

    /// Drops what was held back without committing it: the system stumbled
    /// and forgot.
    pub fn forget_held(&self) {
        let held: Vec<Delivery> = lock(&self.settings.held).drain(..).collect();
        let link = self.link();
        for delivery in held {
            link.abandoned(&delivery);
        }
    }

    /// How the controller commits.
    pub fn set_commit_mode(&self, mode: CommitMode) {
        *lock(&self.settings.commit) = mode;
    }

    /// The next `times` commits fail, as a transaction does when the database
    /// is away.
    pub fn fail_next_commits(&self, times: usize) {
        self.settings.fail_commits.store(times, Ordering::SeqCst);
    }

    /// Whether a reset of a session stream is answered with
    /// `session.resync`, as the system does.
    pub fn set_auto_resync(&self, on: bool) {
        self.settings.auto_resync.store(on, Ordering::SeqCst);
    }

    /// Commits what was held back.
    pub fn release_commits(&self) {
        let held: Vec<Delivery> = lock(&self.settings.held).drain(..).collect();
        let link = self.link();
        for delivery in held {
            commit(&link, &self.store, &self.settings, delivery);
        }
    }

    /// Issues a command of the catalogue to the machine and remembers it.
    pub fn issue<B: CommandBody>(&self, key: &SerialKey, deadline: Time, body: &B) -> Id {
        let id = new_id();
        self.issue_with_id(id, key, deadline, body);
        id
    }

    /// Issues a command under a given identifier.
    pub fn issue_with_id<B: CommandBody>(&self, id: Id, key: &SerialKey, deadline: Time, body: &B) {
        let cmd = Cmd::new(id, key, deadline, body)
            .unwrap_or_else(|error| panic!("a command could not be made: {error}"));
        let handle = self.link().commands().issue(cmd.clone());
        let mut inner = lock(&self.store.inner);
        inner.commands.insert(id, cmd);
        inner.handles.insert(id, handle);
    }

    /// Where an issued command stands.
    #[must_use]
    pub fn progress(&self, id: Id) -> Option<Progress> {
        self.link().commands().progress(id)
    }

    /// Waits for an issued command to end.
    pub async fn wait_command(&self, id: Id) -> Progress {
        let handle = lock(&self.store.inner).handles.get(&id).cloned();
        match handle {
            Some(mut handle) => handle.wait().await,
            None => Progress::Cancelled,
        }
    }

    /// The system restarts: the link is made anew from what the store holds,
    /// and the commands that did not end are issued again under their own
    /// identifiers.
    pub fn restart(&self) {
        let cursors = lock(&self.store.inner).cursors.clone();
        let (link, notices) = self
            .hub
            .restart_system(self.machine_id, self.roles.clone(), cursors);
        tokio::spawn(apply(
            link.clone(),
            notices,
            Arc::clone(&self.store),
            Arc::clone(&self.settings),
            Arc::clone(&self.clock),
        ));
        let unfinished: Vec<Cmd> = {
            let inner = lock(&self.store.inner);
            inner
                .commands
                .iter()
                .filter(|(id, _)| {
                    inner
                        .handles
                        .get(id)
                        .is_none_or(|handle| !handle.progress().is_final())
                })
                .map(|(_, cmd)| cmd.clone())
                .collect()
        };
        for cmd in unfinished {
            let handle = link.commands().adopt(cmd.clone(), Progress::Queued);
            lock(&self.store.inner).handles.insert(cmd.id, handle);
        }
        *lock(&self.link) = link;
    }
}

fn commit(link: &ControllerLink, store: &Store, settings: &Settings, delivery: Delivery) {
    let failing = settings
        .fail_commits
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(1)
        })
        .is_ok();
    if failing {
        link.abandoned(&delivery);
        return;
    }
    {
        let mut inner = lock(&store.inner);
        let name = delivery.stream.to_string();
        let cursor = inner.cursors.get(&name).copied().unwrap_or(0);
        let mut expected = cursor + 1;
        for event in &delivery.events {
            if event.seq != expected {
                inner.violations.push(format!(
                    "{name}: applied {} where {expected} was due",
                    event.seq
                ));
            }
            expected = event.seq + 1;
            let DeliveredEvent {
                seq,
                at,
                kind,
                body,
            } = event.clone();
            inner
                .applied
                .entry(name.clone())
                .or_default()
                .push(Applied {
                    seq,
                    at,
                    kind,
                    body,
                });
        }
        inner.cursors.insert(name, delivery.last_seq);
    }
    link.committed(&delivery);
    store.changed.notify_waiters();
}

async fn apply(
    link: ControllerLink,
    mut notices: tokio::sync::mpsc::UnboundedReceiver<Notice>,
    store: Arc<Store>,
    settings: Arc<Settings>,
    clock: Arc<dyn Clock>,
) {
    while let Some(notice) = notices.recv().await {
        match notice {
            Notice::Deliver(delivery) => {
                let mode = *lock(&settings.commit);
                if mode == CommitMode::Held {
                    lock(&settings.held).push(delivery);
                } else {
                    commit(&link, &store, &settings, delivery);
                }
            }
            Notice::Reset(reset) => {
                let resync = reset.resync && settings.auto_resync.load(Ordering::SeqCst);
                {
                    let mut inner = lock(&store.inner);
                    inner.resets.push(ResetRecord {
                        stream: reset.stream.to_string(),
                        from: reset.from,
                        reason: reset.reason,
                        resync_requested: resync,
                    });
                    let cursor = reset.from.max(1) - 1;
                    inner.cursors.insert(reset.stream.to_string(), cursor);
                }
                if let (true, Stream::Session { thread_id, .. }) = (resync, reset.stream) {
                    let deadline = Time::new(clock.now() + chrono::Duration::minutes(5));
                    if let Ok(cmd) = Cmd::new(
                        new_id(),
                        &SerialKey::Thread(thread_id),
                        deadline,
                        &SessionResync { thread_id },
                    ) {
                        let id = cmd.id;
                        let handle = link.commands().issue(cmd.clone());
                        let mut inner = lock(&store.inner);
                        inner.commands.insert(id, cmd);
                        inner.handles.insert(id, handle);
                    }
                }
                store.changed.notify_waiters();
            }
            Notice::Report(eph) => {
                lock(&store.inner).reports.push(eph.kind);
                store.changed.notify_waiters();
            }
            Notice::Online { session_id } => {
                lock(&store.inner).online.push(session_id);
                store.changed.notify_waiters();
            }
            Notice::Offline { .. } => store.changed.notify_waiters(),
        }
    }
}
