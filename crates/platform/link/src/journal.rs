//! Durable command deduplication and result replay.
//!
//! A machine writes a command down before it says that it has it, and the
//! result before it says how the command ended. The journal is where it is
//! written: one small file for each command, replaced whole at each step, so
//! that a reader finds the step before or the step after and never a half.
//!
//! The journal is why a command can be sent again without harm. The system
//! sends it again after a broken session and after its own restart; the
//! machine looks the command up by identifier and, if it has run, answers
//! with what was written instead of running it a second time. Entries are
//! kept for a day, longer than any command runs and longer than a system
//! stays down for an update.
//!
//! A command in hand is held in memory whole, since it is resumed from what
//! is held. One that ended is held as the little its answers need: its result
//! stays in its file until it is asked for again. A result can be far larger
//! than its command (a vendor's list of models, each with the tools its
//! endpoint offers, runs to megabytes, and a connector lists one every hour),
//! and a day of them held in memory is what a machine would grow by.

use orch_link_contract::commands::{Cmd, CmdRes, CommandError};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Milliseconds in a second.
const SECOND_MILLIS: i64 = 1_000;

/// Why the journal could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// A file could not be read or written.
    #[error("journal file {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// What went wrong.
        #[source]
        source: std::io::Error,
    },
    /// The command is not in the journal.
    #[error("the command {0} is not in the journal")]
    Unknown(Id),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> JournalError + '_ {
    move |source| JournalError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// How far a command got.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Written down and acknowledged; not started.
    Accepted,
    /// Started; no result yet.
    Started,
    /// Ended, with this result.
    Done {
        /// The result as it is sent.
        result: CmdRes,
    },
}

/// What is written about a command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The command as it arrived.
    pub cmd: Cmd,
    /// When it was written down.
    pub accepted_at: Time,
    /// How far it got.
    #[serde(flatten)]
    pub step: Step,
}

impl Entry {
    /// The result, if the command ended.
    #[must_use]
    pub fn result(&self) -> Option<&CmdRes> {
        match &self.step {
            Step::Done { result } => Some(result),
            Step::Accepted | Step::Started => None,
        }
    }
}

/// What is to be written about a command, staged by the journal and written by whoever staged it, with no
/// hold on the journal while the disk is at work.
#[derive(Debug, Clone)]
pub struct Staged {
    dir: PathBuf,
    /// What is written.
    pub entry: Entry,
    #[cfg(test)]
    control: Option<std::sync::Arc<TestWrites>>,
}

impl Staged {
    /// Writes it down, durably: blocks while the disk is at work.
    pub fn write(&self) -> Result<(), JournalError> {
        #[cfg(test)]
        if let Some(control) = &self.control
            && control.gates(&self.entry.step)
        {
            control.before(&self.entry)?;
        }
        write_entry(&self.dir, &self.entry)
    }
}

fn path_in(dir: &Path, id: Id) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// One command's file, replaced whole: a reader finds the step before or the step after, never a half.
/// Files of different commands are written side by side.
fn write_entry(dir: &Path, entry: &Entry) -> Result<(), JournalError> {
    let path = path_in(dir, entry.cmd.id);
    let bytes = serde_json::to_vec(entry).map_err(|e| JournalError::Io {
        path: path.display().to_string(),
        source: std::io::Error::other(e),
    })?;
    let mut staged = tempfile::NamedTempFile::new_in(dir).map_err(io(&path))?;
    staged.write_all(&bytes).map_err(io(&path))?;
    staged.as_file().sync_all().map_err(io(&path))?;
    staged.persist(&path).map_err(|e| io(&path)(e.error))?;
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(io(&path))?;
    Ok(())
}

/// A command that ended, as the journal holds it in memory: its result is in its file.
#[derive(Debug, Clone)]
pub struct Ended {
    path: PathBuf,
    /// The command as it arrived, without its body.
    cmd: Cmd,
    accepted_at: Time,
    ok: bool,
}

/// What an ended command is told to have ended with when its file can no longer be read.
const UNREADABLE: &str =
    "the command ended, but what it ended with can no longer be read on its machine";

impl Ended {
    fn of(dir: &Path, entry: &Entry) -> Self {
        let cmd = Cmd {
            native_members: entry.cmd.native_members.clone(),
            native_revision: entry.cmd.native_revision,
            native_target: entry.cmd.native_target.clone(),
            id: entry.cmd.id,
            kind: entry.cmd.kind.clone(),
            key: entry.cmd.key.clone(),
            deadline: entry.cmd.deadline,
            authority_until: entry.cmd.authority_until,
            body: Json::Null,
        };
        Self {
            path: path_in(dir, cmd.id),
            cmd,
            accepted_at: entry.accepted_at,
            ok: entry.result().is_some_and(|result| result.outcome.is_ok()),
        }
    }

    fn unreadable(&self) -> CmdRes {
        CmdRes {
            id: self.cmd.id,
            outcome: Err(CommandError::new(
                orch_link_contract::commands::ErrorCode::Internal,
                UNREADABLE,
            )),
        }
    }

    /// The command's kind.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.cmd.kind
    }

    /// Whether the command succeeded.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.ok
    }

    /// The whole entry, read back from its file: blocks while the disk is at work. A file that can no longer
    /// be read is told as the command having ended with that failure, never as a command unknown, so that it
    /// is not carried out a second time.
    #[must_use]
    pub fn entry(&self) -> Entry {
        let read = std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok())
            .filter(|entry| entry.cmd.id == self.cmd.id && entry.result().is_some());
        read.unwrap_or_else(|| Entry {
            cmd: self.cmd.clone(),
            accepted_at: self.accepted_at,
            step: Step::Done {
                result: self.unreadable(),
            },
        })
    }

    /// How the command ended, read back from its file as [`Ended::entry`] reads it.
    #[must_use]
    pub fn result(&self) -> CmdRes {
        match self.entry().step {
            Step::Done { result } => result,
            // What is read back always has a result; anything else is told as unreadable.
            Step::Accepted | Step::Started => self.unreadable(),
        }
    }
}

/// What the journal holds of a command, without reading a result from the disk.
#[derive(Debug, Clone)]
pub enum Known {
    /// Written down or started: the command whole, as it is resumed.
    Open(Entry),
    /// Ended; its result is read back from its file.
    Ended(Ended),
}

/// The journal of a machine.
#[derive(Debug)]
pub struct Journal {
    dir: PathBuf,
    keep: Duration,
    entries: HashMap<Id, Known>,
    /// The first terminal outcome, staged but not observable until its write succeeds. Shared with the one
    /// writing it: a result can run to megabytes.
    finishing: HashMap<Id, Arc<Staged>>,
    #[cfg(test)]
    control: Option<std::sync::Arc<TestWrites>>,
}

impl Journal {
    /// Opens the journal in `dir`, reading what is there, and forgetting
    /// what is older than `keep`. A file that cannot be read is left alone
    /// and ignored: it is not a command the machine can answer.
    pub fn open(dir: &Path, keep: Duration, now: Time) -> Result<Self, JournalError> {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
        let mut journal = Self {
            dir: dir.to_owned(),
            keep,
            entries: HashMap::new(),
            finishing: HashMap::new(),
            #[cfg(test)]
            control: None,
        };
        for item in std::fs::read_dir(dir).map_err(io(dir))? {
            let item = item.map_err(io(dir))?;
            let path = item.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(entry) = serde_json::from_slice::<Entry>(&bytes) else {
                continue;
            };
            let id = entry.cmd.id;
            let known = journal.held(entry);
            journal.entries.insert(id, known);
        }
        journal.purge(now)?;
        Ok(journal)
    }

    pub(crate) fn native_fences_directory(&self) -> PathBuf {
        self.dir.join("native-fences")
    }

    fn path_of(&self, id: Id) -> PathBuf {
        path_in(&self.dir, id)
    }

    fn write(&self, entry: &Entry) -> Result<(), JournalError> {
        write_entry(&self.dir, entry)
    }

    /// What is held in memory of an entry: an ended command without its result.
    fn held(&self, entry: Entry) -> Known {
        match entry.step {
            Step::Done { .. } => Known::Ended(Ended::of(&self.dir, &entry)),
            Step::Accepted | Step::Started => Known::Open(entry),
        }
    }

    /// What is written about a command: one in hand as it is held, one that ended read back from its file,
    /// which blocks while the disk is at work. [`Journal::known`] tells which without reading.
    #[must_use]
    pub fn get(&self, id: Id) -> Option<Entry> {
        match self.entries.get(&id)? {
            Known::Open(entry) => Some(entry.clone()),
            Known::Ended(ended) => Some(ended.entry()),
        }
    }

    /// What the journal holds of a command, without reading a result from the disk.
    #[must_use]
    pub fn known(&self, id: Id) -> Option<Known> {
        self.entries.get(&id).cloned()
    }

    /// The kind of a command the journal knows, without reading a result from the disk.
    #[must_use]
    pub fn kind(&self, id: Id) -> Option<&str> {
        match self.entries.get(&id)? {
            Known::Open(entry) => Some(&entry.cmd.kind),
            Known::Ended(ended) => Some(ended.kind()),
        }
    }

    /// Whether a command ended and succeeded, without reading its result from the disk.
    #[must_use]
    pub fn succeeded(&self, id: Id) -> bool {
        matches!(self.entries.get(&id), Some(Known::Ended(ended)) if ended.succeeded())
    }

    /// Writes a command down, durably, before the machine says that it has
    /// it. A command already there is left as it is.
    pub fn accept(&mut self, cmd: &Cmd, now: Time) -> Result<(), JournalError> {
        let Some(staged) = self.stage_accept(cmd, now) else {
            return Ok(());
        };
        self.write(&staged.entry)?;
        self.keep(staged.entry);
        Ok(())
    }

    /// Writes that a command started.
    pub fn started(&mut self, id: Id) -> Result<(), JournalError> {
        if let Some(staged) = self.stage_started(id)? {
            self.write(&staged.entry)?;
        }
        Ok(())
    }

    /// Writes how a command ended, durably, before the machine says so.
    pub fn finish(
        &mut self,
        id: Id,
        outcome: Result<Json, CommandError>,
    ) -> Result<CmdRes, JournalError> {
        let (result, staged) = self.stage_finish(id, outcome)?;
        if let Some(staged) = staged {
            staged.write()?;
            self.keep_finished(&staged.entry);
        }
        Ok(result)
    }

    /// What writing a command down would write, where nothing is written about it yet: written by
    /// [`Staged::write`] with no hold on the journal, and kept with [`Journal::keep`] once it is on the
    /// disk. Commands arriving together can be persisted independently rather than waiting for each
    /// other's disk flushes.
    #[must_use]
    pub fn stage_accept(&self, cmd: &Cmd, now: Time) -> Option<Staged> {
        if self.entries.contains_key(&cmd.id) {
            return None;
        }
        Some(Staged {
            dir: self.dir.clone(),
            #[cfg(test)]
            control: self.control.clone(),
            entry: Entry {
                cmd: cmd.clone(),
                accepted_at: now,
                step: Step::Accepted,
            },
        })
    }

    /// Keeps a command that [`Journal::stage_accept`] staged, once it is written down.
    pub fn keep(&mut self, entry: Entry) {
        let id = entry.cmd.id;
        if !self.entries.contains_key(&id) {
            let known = self.held(entry);
            self.entries.insert(id, known);
        }
    }

    /// Notes that a command started, and says what is to be written for it, if anything.
    pub fn stage_started(&mut self, id: Id) -> Result<Option<Staged>, JournalError> {
        let known = self.entries.get_mut(&id).ok_or(JournalError::Unknown(id))?;
        let Known::Open(entry) = known else {
            return Ok(None);
        };
        if entry.step != Step::Accepted {
            return Ok(None);
        }
        entry.step = Step::Started;
        Ok(Some(Staged {
            dir: self.dir.clone(),
            entry: entry.clone(),
            #[cfg(test)]
            control: self.control.clone(),
        }))
    }

    /// Notes how a command ended, and says what is to be written for it: nothing when it had ended
    /// already, since the first result stands.
    pub fn stage_finish(
        &mut self,
        id: Id,
        outcome: Result<Json, CommandError>,
    ) -> Result<(CmdRes, Option<Arc<Staged>>), JournalError> {
        let entry = match self.entries.get(&id).ok_or(JournalError::Unknown(id))? {
            // The first result stands: a command ends once.
            Known::Ended(ended) => return Ok((ended.result(), None)),
            Known::Open(entry) => entry,
        };
        if let Some(staged) = self.finishing.get(&id) {
            let Step::Done { result } = &staged.entry.step else {
                return Err(JournalError::Unknown(id));
            };
            return Ok((result.clone(), Some(Arc::clone(staged))));
        }
        let result = CmdRes { id, outcome };
        let mut entry = entry.clone();
        entry.step = Step::Done {
            result: result.clone(),
        };
        let staged = Arc::new(Staged {
            dir: self.dir.clone(),
            entry,
            #[cfg(test)]
            control: self.control.clone(),
        });
        self.finishing.insert(id, Arc::clone(&staged));
        Ok((result, Some(staged)))
    }

    /// Makes a terminal entry observable only after its staged bytes are durable. What is held of it in
    /// memory is what [`Known::Ended`] holds: its result is in the file just written.
    pub fn keep_finished(&mut self, entry: &Entry) {
        let id = entry.cmd.id;
        if matches!(entry.step, Step::Done { .. }) {
            let known = Known::Ended(Ended::of(&self.dir, entry));
            self.entries.insert(id, known);
            self.finishing.remove(&id);
        }
    }

    /// The commands that were accepted or started and have no result: what
    /// was in the machine's hands when it stopped.
    #[must_use]
    pub fn unfinished(&self) -> Vec<&Entry> {
        let mut found: Vec<&Entry> = self
            .entries
            .values()
            .filter_map(|known| match known {
                Known::Open(entry) => Some(entry),
                Known::Ended(_) => None,
            })
            .collect();
        found.sort_by_key(|entry| entry.accepted_at);
        found
    }

    /// Forgets what is older than the journal keeps. Returns how many
    /// commands that was.
    pub fn purge(&mut self, now: Time) -> Result<usize, JournalError> {
        self.purge_except(now, &HashSet::new())
    }

    /// Live commands and staged terminal writes remain replayable regardless of journal age.
    pub fn purge_except(&mut self, now: Time, live: &HashSet<Id>) -> Result<usize, JournalError> {
        let keep_millis =
            i64::try_from(self.keep.as_secs()).unwrap_or(i64::MAX / SECOND_MILLIS) * SECOND_MILLIS;
        let old: Vec<Id> = self
            .entries
            .iter()
            .filter(|(id, known)| {
                let accepted_at = match known {
                    Known::Open(entry) => entry.accepted_at,
                    Known::Ended(ended) => ended.accepted_at,
                };
                !live.contains(*id)
                    && !self.finishing.contains_key(*id)
                    && accepted_at.unix_millis() + keep_millis <= now.unix_millis()
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &old {
            self.entries.remove(id);
            let path = self.path_of(*id);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(&path)(error)),
            }
        }
        Ok(old.len())
    }

    /// How many commands are written.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn terminal_writes(&mut self, control: std::sync::Arc<TestWrites>) {
        self.control = Some(control);
    }
}

/// Per-journal deterministic fault/latch, compiled only into the library's unit tests.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TestWrites {
    id: Id,
    blocked: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
    failures: std::sync::atomic::AtomicUsize,
    hold_retry: bool,
    /// Gate the first write of the command (the one that writes it down) instead of the one that ends it.
    on_accept: bool,
    entered: tokio::sync::Semaphore,
}

#[cfg(test)]
impl TestWrites {
    pub(crate) fn new(id: Id, blocked: bool, failures: usize) -> Self {
        Self {
            id,
            blocked: std::sync::Mutex::new(blocked),
            changed: std::sync::Condvar::new(),
            failures: std::sync::atomic::AtomicUsize::new(failures),
            hold_retry: false,
            on_accept: false,
            entered: tokio::sync::Semaphore::new(0),
        }
    }

    /// Gates the write that writes the command down, which is the first, instead of the write that ends it.
    pub(crate) fn accepting(mut self) -> Self {
        self.on_accept = true;
        self
    }

    fn gates(&self, step: &Step) -> bool {
        if self.on_accept {
            matches!(step, Step::Accepted)
        } else {
            matches!(step, Step::Done { .. })
        }
    }

    pub(crate) fn hold_retry(mut self) -> Self {
        self.hold_retry = true;
        self
    }

    pub(crate) async fn entered(&self) -> Result<(), tokio::sync::AcquireError> {
        self.entered.acquire().await?.forget();
        Ok(())
    }

    pub(crate) fn release(&self) {
        *self
            .blocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        self.changed.notify_all();
    }

    fn before(&self, entry: &Entry) -> Result<(), JournalError> {
        if entry.cmd.id != self.id {
            return Ok(());
        }
        self.entered.add_permits(1);
        let mut blocked = self
            .blocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *blocked {
            blocked = self
                .changed
                .wait(blocked)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if self
            .failures
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            if self.hold_retry {
                *blocked = true;
            }
            return Err(JournalError::Io {
                path: "test terminal journal".to_owned(),
                source: std::io::Error::other("injected terminal write failure"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod durable_tests;
