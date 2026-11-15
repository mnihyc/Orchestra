//! Native temporary files for transit uploads and version-checked ranged reads.

use super::*;
use crate::blobs::{BlobReader, BlobWriter, RelaySink, RelaySource};
use async_trait::async_trait;
use bytes::Bytes;
use orch_link_contract::commands::{FsRelayCommit, FsRelayReceive, FsRelaySend};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, PoisonError, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Only upload metadata lives in this known directory; no source tree is scanned for temporary files.
const RECORDS: &str = ".orch/file-relays";

#[derive(Debug, Default)]
struct Expiries {
    by_id: std::collections::BTreeMap<Id, Time>,
    due: BTreeSet<(Time, Id)>,
}

#[derive(Debug, Default)]
struct Locks {
    sessions: Mutex<HashMap<Id, Weak<AsyncMutex<()>>>>,
    changed: tokio::sync::Notify,
}

fn locks() -> &'static Arc<Locks> {
    static LOCKS: std::sync::OnceLock<Arc<Locks>> = std::sync::OnceLock::new();
    LOCKS.get_or_init(|| Arc::new(Locks::default()))
}

#[derive(Debug)]
struct Lease {
    held: Option<OwnedMutexGuard<()>>,
    locks: Arc<Locks>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        drop(self.held.take());
        self.locks.changed.notify_waiters();
    }
}

#[derive(Debug, Default)]
pub(super) struct Stages {
    expiries: Mutex<Expiries>,
    loaded: std::sync::atomic::AtomicBool,
    started: std::sync::atomic::AtomicBool,
    pub(super) stop: tokio_util::sync::CancellationToken,
}

impl Stages {
    fn mutex(&self, session: Id) -> Arc<AsyncMutex<()>> {
        let mut sessions = locks()
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        sessions.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = sessions.get(&session).and_then(Weak::upgrade) {
            return lock;
        }
        let mutex = Arc::new(AsyncMutex::new(()));
        sessions.insert(session, Arc::downgrade(&mutex));
        mutex
    }

    async fn lease(&self, session: Id) -> Arc<Lease> {
        Arc::new(Lease {
            held: Some(self.mutex(session).lock_owned().await),
            locks: Arc::clone(locks()),
        })
    }

    fn try_lease(&self, session: Id) -> Option<Arc<Lease>> {
        self.mutex(session).try_lock_owned().ok().map(|held| {
            Arc::new(Lease {
                held: Some(held),
                locks: Arc::clone(locks()),
            })
        })
    }

    fn schedule(&self, session: Id, at: Time) {
        let mut index = self.expiries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(before) = index.by_id.insert(session, at) {
            index.due.remove(&(before, session));
        }
        index.due.insert((at, session));
        drop(index);
        locks().changed.notify_waiters();
    }

    fn forget(&self, session: Id) {
        let mut index = self.expiries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(before) = index.by_id.remove(&session) {
            index.due.remove(&(before, session));
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Stage {
    receive_path: String,
    parent: String,
    /// Every location of a partial local preparation, so cancellation and expiry remove them too.
    parents: BTreeSet<String>,
    size: i64,
    expires_at: Time,
    verified: Option<Verified>,
    retry_secs: u64,
    commit: Option<Commit>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Commit {
    body: FsRelayCommit,
    before: Meta,
    receipt: Option<FileStat>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Verified {
    hash: Hash,
    meta: Meta,
}

fn temporary(session: Id) -> OsString {
    format!("{TEMP_PREFIX}up-{}", session.simple()).into()
}

impl Files {
    fn load_stage_expiries(&self) -> Result<()> {
        use std::sync::atomic::Ordering;
        if self.inner.relay.loaded.load(Ordering::Acquire) {
            return Ok(());
        }
        let directory = match self
            .ws()
            .locate(&self.parse(&format!("{RECORDS}/unused"))?, Locate::ENTRY)
        {
            Ok(located) => located.parent,
            Err(HostError::NotFound(_)) => {
                self.inner.relay.loaded.store(true, Ordering::Release);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        directory.visit_names(&mut |name| {
            let Some(session) = name.to_str().and_then(|name| name.strip_suffix(".json")).and_then(|name| name.parse::<Id>().ok()) else { return true; };
            match self.read_stage(session) {
                Ok(stage) => self.inner.relay.schedule(session, stage.expires_at),
                Err(HostError::NotFound(_)) => {},
                Err(error) => tracing::warn!(%session, %error, "an unreadable native upload record was left intact"),
            }
            true
        }).map_err(|e| HostError::io("reading the native upload metadata directory", e))?;
        self.inner.relay.loaded.store(true, Ordering::Release);
        Ok(())
    }

    /// Removes expired native upload artifacts by their own metadata index, without walking workspace files.
    pub async fn prune_file_relays(&self, now: Time) -> Result<()> {
        let this = self.clone();
        blocking(move || this.load_stage_expiries()).await?;
        let due: Vec<Id> = self
            .inner
            .relay
            .expiries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .due
            .iter()
            .take_while(|(at, _)| *at <= now)
            .map(|(_, session)| *session)
            .collect();
        for session in due {
            let Some(held) = self.inner.relay.try_lease(session) else {
                continue;
            };
            let this = self.clone();
            blocking(move || {
                let _held = held;
                let stage = match this.read_stage(session) {
                    Ok(stage) => stage,
                    Err(HostError::NotFound(_)) => { this.inner.relay.forget(session); return Ok(()); },
                    Err(error) => return Err(error),
                };
                let result = if stage.expires_at <= now {
                    this.forget_stage(session, &stage)
                } else {
                    this.forget_stage_files(session, &stage)
                };
                match result {
                    Ok(()) if stage.expires_at > now => this.inner.relay.schedule(session, stage.expires_at),
                    Ok(()) => {},
                    Err(error) => {
                        tracing::warn!(%session, %error, "native upload cleanup will be tried again");
                        this.retry_cleanup(session, &stage, now);
                    }
                }
                Ok(())
            }).await?;
        }
        Ok(())
    }

    /// One wakeable expiry task belongs to this Files instance and ends when its last owner is gone.
    pub(crate) fn start_relay_cleanup(&self, clock: Arc<dyn orch_types::Clock>) {
        use std::sync::atomic::Ordering;
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.inner.relay.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let (inner, stages) = (Arc::downgrade(&self.inner), Arc::clone(&self.inner.relay));
        let _task = runtime.spawn(async move {
            loop {
                let changed = locks().changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                let files = Files {
                    inner,
                    recorded_operation: None,
                };
                let now = Time::new(clock.now());
                let failed = match files.prune_file_relays(now).await {
                    Ok(()) => false,
                    Err(error) => {
                        tracing::warn!(%error, "native upload expiries could not be inspected");
                        true
                    }
                };
                let retry = files.inner.limits.manifest_interval;
                let next = stages
                    .expiries
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .due
                    .iter()
                    .find(|(at, _)| *at > now)
                    .map(|(at, _)| *at);
                drop(files);
                let wait = if failed {
                    Some(retry)
                } else {
                    next.and_then(|at| (at.get() - clock.now()).to_std().ok())
                };
                let until = wait.and_then(|wait| tokio::time::Instant::now().checked_add(wait));
                let wake = async {
                    match until {
                        Some(until) => tokio::time::sleep_until(until).await,
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    () = stages.stop.cancelled() => return,
                    () = changed => {},
                    () = wake => {},
                }
            }
        });
    }

    fn relay_record(&self, session: Id, create: bool) -> Result<Located> {
        self.ws().locate(
            &self.parse(&format!("{RECORDS}/{session}.json"))?,
            if create {
                Locate::FOR_CREATE
            } else {
                Locate::ENTRY
            },
        )
    }

    fn read_stage(&self, session: Id) -> Result<Stage> {
        let record = self.relay_record(session, false)?;
        let file = record
            .parent
            .open_read(record.name()?)
            .map_err(|e| HostError::io("opening an upload's native record", e))?;
        serde_json::from_reader(file).map_err(|e| {
            HostError::Internal(format!("the native upload record cannot be read: {e}"))
        })
    }

    fn save_stage(&self, session: Id, stage: &Stage) -> Result<()> {
        let record = self.relay_record(session, true)?;
        let name = temp_name();
        let mut file = record
            .parent
            .create_new(&name, 0o600)
            .map_err(|e| HostError::io("making an upload's native record", e))?;
        let mut cleanup = Cleanup::new(&record.parent, &name, false)?;
        serde_json::to_writer(&mut file, stage).map_err(|e| {
            HostError::Internal(format!("the native upload record cannot be written: {e}"))
        })?;
        file.sync_all()
            .map_err(|e| HostError::io("flushing an upload's native record", e))?;
        record
            .parent
            .rename_to(&name, &record.parent, record.name()?, true)
            .map_err(|e| HostError::io("publishing an upload's native record", e))?;
        cleanup.disarm();
        record
            .parent
            .sync()
            .map_err(|e| HostError::io("flushing an upload's record directory", e))?;
        self.inner.relay.schedule(session, stage.expires_at);
        Ok(())
    }

    fn parent_at(&self, parent: &str) -> Result<Dir> {
        let dummy = self.parse(parent)?.join("unused")?;
        Ok(self.ws().locate(&dummy, Locate::ENTRY)?.parent)
    }

    fn stage_parent(&self, stage: &Stage) -> Result<Dir> {
        self.parent_at(&stage.parent)
    }

    fn checked_stage(&self, session: Id, receive_path: &str, now: Time) -> Result<Stage> {
        let stage = self.read_stage(session)?;
        if stage.receive_path != receive_path || stage.expires_at <= now {
            return Err(HostError::NotFound(
                "the upload session is not available".into(),
            ));
        }
        Ok(stage)
    }

    /// Computes a content version without sending the file to any store or applying a blob-storage limit.
    pub async fn relay_describe(&self, path: &RelPath) -> Result<FsFetchResult> {
        let (this, path) = (self.clone(), path.clone());
        let opened = blocking(move || this.open_regular(&path)).await?;
        let hash = self.hash_opened(&opened).await?;
        Ok(FsFetchResult {
            hash,
            size: i64::try_from(opened.meta.size).map_err(|_| {
                HostError::Unsupported("the file size cannot be represented".into())
            })?,
            mtime_ns: opened.meta.mtime_ns,
        })
    }

    /// Sends only the selected range, checking the opened file before and after the stream.
    pub async fn relay_send(&self, body: &FsRelaySend, sink: &dyn RelaySink) -> Result<()> {
        let (this, path) = (self.clone(), self.parse(&body.path)?);
        let opened = blocking(move || this.open_regular(&path)).await?;
        let size = i64::try_from(opened.meta.size)
            .map_err(|_| HostError::Unsupported("the file size cannot be represented".into()))?;
        if body.offset < 0
            || body.len < 0
            || body
                .offset
                .checked_add(body.len)
                .is_none_or(|end| end > size)
        {
            return Err(HostError::Unsupported(
                "the range does not belong to this file".into(),
            ));
        }
        if self.hash_opened(&opened).await? != body.hash {
            return Err(HostError::Changed(
                "the file is no longer the version that was opened".into(),
            ));
        }
        let mut source = SliceReader {
            file: FileReader::new(Arc::clone(&opened.file)),
            offset: body.offset,
            len: body.len,
        };
        sink.send(body.len, &mut source)
            .await
            .map_err(blob_failure)?;
        let file = Arc::clone(&opened.file);
        let after = blocking(move || {
            file_meta(&file).map_err(|e| HostError::io("checking a relayed file", e))
        })
        .await?;
        if after.identity() != opened.meta.identity() {
            return Err(HostError::Changed(
                "the file changed while it was being read".into(),
            ));
        }
        Ok(())
    }

    /// Receives into the destination's own hidden file and preserves it across a cut connection.
    pub async fn relay_receive(
        &self,
        body: &FsRelayReceive,
        source: &dyn RelaySource,
        clock: Arc<dyn orch_types::Clock>,
    ) -> Result<FsFetchResult> {
        self.require_write()?;
        self.start_relay_cleanup(Arc::clone(&clock));
        let now = Time::new(clock.now());
        if body.size < 0 || body.expires_at <= now {
            return Err(HostError::NotFound(
                "the upload session is not available".into(),
            ));
        }
        let (size, session) = (body.size, body.session_id);
        let lease = self.inner.relay.lease(body.session_id).await;
        let (this, body, held, entered) = (
            self.clone(),
            body.clone(),
            Arc::clone(&lease),
            Arc::clone(&clock),
        );
        let file = blocking(move || {
            let _held = held;
            crate::error::check_cancelled()?;
            this.require_write()?;
            let now = Time::new(entered.now());
            if body.expires_at <= now {
                return Err(HostError::NotFound(
                    "the upload session expired while it waited".into(),
                ));
            }
            let path = this.parse(&body.path)?;
            let located = this.ws().locate(&path, Locate::FOR_WRITE)?;
            this.check_visible(&path, &located, false)?;
            let _ = located.name()?;
            let stage = match this.read_stage(body.session_id) {
                Ok(stage)
                    if stage.receive_path == body.path
                        && stage.size == body.size
                        && stage.expires_at > now =>
                {
                    stage
                }
                Ok(_) => {
                    return Err(HostError::Changed(
                        "this upload session names another destination or size".into(),
                    ));
                }
                Err(HostError::NotFound(_)) => {
                    let parent = located
                        .real
                        .parent()
                        .ok_or_else(|| {
                            HostError::Unsupported("the workspace root is not a file".into())
                        })?
                        .to_string();
                    let stage = Stage {
                        receive_path: body.path.clone(),
                        parent: parent.clone(),
                        parents: BTreeSet::from([parent]),
                        size: body.size,
                        expires_at: body.expires_at,
                        verified: None,
                        retry_secs: body.idle_secs,
                        commit: None,
                    };
                    this.save_stage(body.session_id, &stage)?;
                    stage
                }
                Err(error) => return Err(error),
            };
            if stage
                .commit
                .as_ref()
                .is_some_and(|commit| commit.receipt.is_some())
            {
                return Err(HostError::Changed(
                    "this upload is already committed".into(),
                ));
            }
            let parent = this.stage_parent(&stage)?;
            let name = temporary(body.session_id);
            let file = match parent.create_new(&name, 0o600) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => parent
                    .open_update(&name)
                    .map_err(|e| HostError::io("resuming an upload's native temporary file", e))?,
                Err(error) => {
                    return Err(HostError::io(
                        "receiving an upload beside its destination",
                        error,
                    ));
                }
            };
            if file
                .metadata()
                .map_err(|e| HostError::io("checking a resumed upload's size", e))?
                .len()
                > u64::try_from(body.size).unwrap_or(0)
            {
                return Err(HostError::Changed(
                    "the native temporary file extends past the announced upload".into(),
                ));
            }
            Ok(Arc::new(file))
        })
        .await?;
        let mut writer = DurableWriter {
            files: self.clone(),
            held: Arc::clone(&lease),
            file: Arc::clone(&file),
            writer: FileWriter::new(Arc::clone(&file)),
        };
        let received = source.receive(size, &mut writer).await;
        self.require_write()?;
        received.map_err(blob_failure)?;
        let (finished, held, expected, this, session) =
            (Arc::clone(&file), lease, size, self.clone(), session);
        blocking(move || {
            let _held = held;
            finished
                .sync_all()
                .map_err(|e| HostError::io("flushing a received upload", e))?;
            let before =
                file_meta(&finished).map_err(|e| HostError::io("checking a received upload", e))?;
            if i64::try_from(before.size).ok() != Some(expected) {
                return Err(HostError::Changed(
                    "the received upload has another size".into(),
                ));
            }
            let hash =
                hash_file(&finished).map_err(|e| HostError::io("hashing a received upload", e))?;
            let after = file_meta(&finished)
                .map_err(|e| HostError::io("checking a received upload after hashing it", e))?;
            if before.identity() != after.identity() {
                return Err(HostError::Changed(
                    "the received upload changed while it was checked".into(),
                ));
            }
            let mut stage = this.read_stage(session)?;
            stage.verified = Some(Verified { hash, meta: after });
            this.save_stage(session, &stage)?;
            Ok(FsFetchResult {
                hash,
                size: expected,
                mtime_ns: after.mtime_ns,
            })
        })
        .await
    }

    /// Places an already received file beside its chosen destination without an environment serial key.
    pub async fn relay_prepare(
        &self,
        session: Id,
        receive_path: &str,
        path: &str,
        clock: Arc<dyn orch_types::Clock>,
    ) -> Result<()> {
        self.require_write()?;
        let held = self.inner.relay.lease(session).await;
        let (this, receive_path, path) = (self.clone(), receive_path.to_owned(), self.parse(path)?);
        blocking(move || {
            let _held = held;
            crate::error::check_cancelled()?;
            this.require_write()?;
            let now = Time::new(clock.now());
            let mut stage = this.checked_stage(session, &receive_path, now)?;
            if stage
                .commit
                .as_ref()
                .is_some_and(|commit| commit.receipt.is_some())
            {
                return Ok(());
            }
            let target = this.ws().locate(&path, Locate::FOR_WRITE)?;
            this.check_visible(&path, &target, false)?;
            let _ = target.name()?;
            let parent = target
                .real
                .parent()
                .ok_or_else(|| HostError::Unsupported("the workspace root is not a file".into()))?
                .to_string();
            let source = this.stage_parent(&stage)?;
            let name = temporary(session);
            let from_file = source
                .open_read(&name)
                .map_err(|e| HostError::io("opening an upload for local preparation", e))?;
            let verified = stage
                .verified
                .as_ref()
                .ok_or_else(|| HostError::Busy("the upload has not finished receiving".into()))?;
            let before = file_meta(&from_file)
                .map_err(|e| HostError::io("checking an upload before local preparation", e))?;
            if before.identity() != verified.meta.identity()
                && hash_file(&from_file).map_err(|e| {
                    HostError::io("checking a changed upload before local preparation", e)
                })? != verified.hash
            {
                return Err(HostError::Changed(
                    "the received upload changed before its local preparation".into(),
                ));
            }
            let expected_hash = verified.hash;
            if stage.parent == parent {
                stage.verified = Some(Verified {
                    hash: expected_hash,
                    meta: before,
                });
                this.save_stage(session, &stage)?;
                return Ok(());
            }
            stage.parents.insert(parent.clone());
            this.save_stage(session, &stage)?;
            // Both copies remain complete until the new location's record is durable. An
            // interrupted preparation can resume from either its old source or its checked prefix.
            let from = source.open_read(&name).map_err(|e| {
                HostError::io("opening a received upload for its final location", e)
            })?;
            let to = match target.parent.create_new(&name, 0o600) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => target
                    .parent
                    .open_update(&name)
                    .map_err(|e| HostError::io("resuming an upload's local preparation", e))?,
                Err(error) => {
                    return Err(HostError::io(
                        "preparing an upload beside its chosen destination",
                        error,
                    ));
                }
            };
            let size = from
                .metadata()
                .map_err(|e| HostError::io("checking a received upload's size", e))?
                .len();
            let mut at = to
                .metadata()
                .map_err(|e| HostError::io("checking a locally prepared prefix", e))?
                .len()
                .min(size);
            if hash_file_prefix(&from, at)
                .map_err(|e| HostError::io("checking an upload's source prefix", e))?
                != hash_file_prefix(&to, at)
                    .map_err(|e| HostError::io("checking an upload's prepared prefix", e))?
            {
                at = 0;
            }
            to.set_len(at)
                .map_err(|e| HostError::io("resuming a locally prepared upload", e))?;
            let mut buffer = vec![0; COPY_CHUNK_BYTES];
            while at < size {
                crate::error::check_cancelled()?;
                let wanted = usize::try_from(size - at)
                    .unwrap_or(usize::MAX)
                    .min(buffer.len());
                let read = read_at(&from, &mut buffer[..wanted], at)
                    .map_err(|e| HostError::io("reading a received upload", e))?;
                if read == 0 {
                    return Err(HostError::Changed(
                        "the received upload ended during its local preparation".into(),
                    ));
                }
                let mut written = 0;
                while written < read {
                    let count = write_at(&to, &buffer[written..read], at + written as u64)
                        .map_err(|e| {
                            HostError::io("preparing an upload beside its destination", e)
                        })?;
                    if count == 0 {
                        return Err(HostError::io(
                            "preparing an upload beside its destination",
                            std::io::ErrorKind::WriteZero.into(),
                        ));
                    }
                    written += count;
                }
                at += read as u64;
            }
            to.sync_all()
                .map_err(|e| HostError::io("flushing an upload's local preparation", e))?;
            let prepared = target
                .parent
                .open_read(&name)
                .map_err(|e| HostError::io("checking the prepared upload", e))?;
            let prepared_meta = file_meta(&prepared)
                .map_err(|e| HostError::io("checking the prepared upload's identity", e))?;
            if hash_file(&prepared)
                .map_err(|e| HostError::io("checking the prepared upload's content", e))?
                != expected_hash
            {
                return Err(HostError::Changed(
                    "the upload changed during its local preparation".into(),
                ));
            }
            stage.parent = parent;
            stage.verified = Some(Verified {
                hash: expected_hash,
                meta: prepared_meta,
            });
            this.save_stage(session, &stage)?;
            Ok(())
        })
        .await
    }

    /// Publishes a checked upload by a short rename and keeps a replayable metadata receipt.
    pub async fn relay_commit(
        &self,
        body: &FsRelayCommit,
        clock: Arc<dyn orch_types::Clock>,
    ) -> Result<FileStat> {
        self.require_write()?;
        let held = self.inner.relay.lease(body.session_id).await;
        let (this, body) = (self.clone(), body.clone());
        blocking(move || {
            let _held = held;
            crate::error::check_cancelled()?;
            let now = Time::new(clock.now());
            let mut stage = this.checked_stage(body.session_id, &body.receive_path, now)?;
            if let Some(commit) = &stage.commit && let Some(receipt) = &commit.receipt {
                if commit.body != body { return Err(HostError::Changed("this upload was already committed to another destination".into())); }
                return Ok(receipt.clone());
            }
            let asked = this.parse(&body.path)?;
            let target = this.ws().locate(&asked, Locate::FOR_WRITE)?;
            this.check_visible(&asked, &target, false)?;
            let target_name = target.name()?.to_owned();
            if target.real.parent().is_none_or(|parent| parent.to_string() != stage.parent) {
                return Err(HostError::Busy("the upload must first be prepared beside its chosen destination".into()));
            }
            let source = this.stage_parent(&stage)?;
            let name = temporary(body.session_id);
            let file = match source.open_read(&name) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // The process may have ended after the atomic rename and before saving its receipt.
                    // Its recorded source inode and content prove that exact accepted publication.
                    let intent = stage.commit.as_ref().filter(|commit| commit.body == body)
                        .ok_or_else(|| HostError::NotFound("the upload's temporary file is not there".into()))?;
                    let file = target.parent.open_read(&target_name).map_err(|e| HostError::io("checking an interrupted upload publication", e))?;
                    let meta = file_meta(&file).map_err(|e| HostError::io("checking an interrupted publication's identity", e))?;
                    let identity_matches = intent.before.ino == 0 || (meta.ino == intent.before.ino && meta.dev == intent.before.dev);
                    if !identity_matches || i64::try_from(meta.size).ok() != Some(body.size) || hash_file(&file).map_err(|e| HostError::io("checking an interrupted publication's content", e))? != body.hash {
                        return Err(HostError::Changed("the interrupted upload publication cannot be proved from its destination".into()));
                    }
                    let mut stat = this.stat(&asked)?;
                    stat.hash = Some(body.hash);
                    if let Some(commit) = &mut stage.commit { commit.receipt = Some(stat.clone()); }
                    this.save_stage(body.session_id, &stage)?;
                    this.remove_committed_temps(body.session_id, &stage, now);
                    return Ok(stat);
                }
                Err(error) => return Err(HostError::io("opening the upload to commit", error)),
            };
            let meta = file_meta(&file).map_err(|e| HostError::io("checking the upload to commit", e))?;
            if i64::try_from(meta.size).ok() != Some(body.size) || stage.size != body.size || stage.verified.as_ref().is_none_or(|verified| verified.hash != body.hash || verified.meta.identity() != meta.identity()) {
                return Err(HostError::Changed("the received upload is no longer the complete content that was checked".into()));
            }
            if !this.matches_expected(&target.parent, &target_name, &target.real, body.expected_hash)? {
                return Err(this.stale(&asked));
            }
            let bits = body.mode.or(target.meta.map(|meta| meta.mode)).unwrap_or(NEW_FILE_MODE) & WRITABLE_MODE_BITS;
            set_file_mode(&file, bits).map_err(|e| HostError::io("setting the uploaded file's permissions", e))?;
            file.sync_all().map_err(|e| HostError::io("flushing an upload before committing it", e))?;
            stage.commit = Some(Commit { body: body.clone(), before: file_meta(&file).map_err(|e| HostError::io("recording an upload publication's identity", e))?, receipt: None });
            this.save_stage(body.session_id, &stage)?;
            crate::error::check_cancelled()?;
            this.require_write()?;
            if !this.matches_expected(&target.parent, &target_name, &target.real, body.expected_hash)? { return Err(this.stale(&asked)); }
            source.rename_to(&name, &target.parent, &target_name, !matches!(body.expected_hash, Some(ExpectedHash::Absent)))
                .map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { this.stale(&asked) } else { HostError::io("putting the uploaded file in place", e) })?;
            target.parent.sync().map_err(|e| HostError::io("flushing the uploaded file's directory", e))?;
            this.note_orch_write(&target.real);
            let after = file_meta(&file).map_err(|e| HostError::io("checking the uploaded file in place", e))?;
            this.inner.cache.put(&target.real, after.identity(), body.hash);
            let mut stat = this.stat(&asked)?;
            stat.hash = Some(body.hash);
            if let Some(commit) = &mut stage.commit { commit.receipt = Some(stat.clone()); }
            this.save_stage(body.session_id, &stage)?;
            this.remove_committed_temps(body.session_id, &stage, now);
            Ok(stat)
        }).await
    }

    fn retry_cleanup(&self, session: Id, stage: &Stage, now: Time) {
        let at = i64::try_from(stage.retry_secs)
            .ok()
            .and_then(chrono::Duration::try_seconds)
            .and_then(|wait| now.get().checked_add_signed(wait))
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
        self.inner.relay.schedule(session, Time::new(at));
    }

    fn remove_committed_temps(&self, session: Id, stage: &Stage, now: Time) {
        if let Err(error) = self.forget_stage_files(session, stage) {
            tracing::warn!(%session, %error, "the temporary files of a committed upload await cleanup");
            self.retry_cleanup(session, stage, now);
        }
    }

    fn forget_stage_files(&self, session: Id, stage: &Stage) -> Result<()> {
        for parent in &stage.parents {
            let directory = match self.parent_at(parent) {
                Ok(parent) => parent,
                Err(HostError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            match directory.unlink(&temporary(session)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(HostError::io(
                        "removing an upload's native temporary file",
                        error,
                    ));
                }
            }
        }
        Ok(())
    }

    fn forget_stage(&self, session: Id, stage: &Stage) -> Result<()> {
        self.forget_stage_files(session, stage)?;
        let record = self.relay_record(session, false)?;
        match record.parent.unlink(record.name()?) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(HostError::io("removing an upload's native record", error)),
        }
        self.inner.relay.forget(session);
        Ok(())
    }

    /// Removes only this session's native temporary files; an already committed destination stays.
    pub async fn relay_abort(&self, session: Id, receive_path: &str) -> Result<()> {
        let held = self.inner.relay.lease(session).await;
        let (this, receive_path) = (self.clone(), receive_path.to_owned());
        blocking(move || {
            let _held = held;
            let stage = match this.read_stage(session) {
                Ok(stage) => stage,
                Err(HostError::NotFound(_)) => return Ok(()),
                Err(error) => return Err(error),
            };
            if stage.receive_path != receive_path {
                return Err(HostError::NotFound(
                    "this upload has another destination".into(),
                ));
            }
            this.forget_stage(session, &stage)
        })
        .await
    }
}

struct SliceReader {
    file: FileReader,
    offset: i64,
    len: i64,
}

#[async_trait]
impl BlobReader for SliceReader {
    async fn read(&mut self, offset: i64, len: i64) -> std::io::Result<Bytes> {
        if offset < 0 || offset > self.len {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        self.file
            .read(self.offset + offset, len.min(self.len - offset))
            .await
    }
}

struct DurableWriter {
    files: Files,
    held: Arc<Lease>,
    file: Arc<File>,
    writer: FileWriter,
}

#[async_trait]
impl BlobWriter for DurableWriter {
    async fn received(&mut self) -> std::io::Result<i64> {
        self.writer.received().await
    }
    async fn write(&mut self, offset: i64, bytes: Bytes) -> std::io::Result<()> {
        self.files.require_write().map_err(std::io::Error::other)?;
        if self.writer.received().await? != offset {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let held: Arc<dyn Send + Sync> = self.held.clone();
        self.writer.write_held(offset, bytes, Some(held)).await?;
        let (file, held) = (Arc::clone(&self.file), Arc::clone(&self.held));
        crate::error::blocking_io(move || {
            let _held = held;
            file.sync_data()
        })
        .await
    }
    async fn hash(&mut self) -> std::io::Result<Hash> {
        self.writer.hash().await
    }
    async fn discard(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
