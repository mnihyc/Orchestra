//! The files of a copy kept until the thread's next turn writes them into its workspace, a delivered copy taken out again, and a file an agent downloads written where it asked: the engine's own keeper, over the store of content and the machines.
//!
//! **A copy** is kept in the transaction that shares it: each of its files is put in the store, a list of them
//! is written as a delivery writes one, and the list and every file in it are pinned, so that nothing is
//! collected before the thread's next turn. That turn, before its own files, writes each copy that waits into
//! `.orch/shared/<copy>/` of the thread's directory with the command that writes the files of a message
//! (`fs.materialize` for a delivery), records the copy as delivered into its environment, and lets the pins
//! go. A copy that cannot be written stays waiting and is tried again by the next turn: the agent was told of
//! it, and reads the same thing with `read_shared` meanwhile.
//!
//! **A copy taken back** is removed from the workspace it was written into, that directory and nothing else,
//! by a command that is issued in the transaction that takes it back and not waited for; one that was never
//! written lets its pins go.
//!
//! **A download** is one file written with `fs.write` under the thread's directory, where the agent said, and
//! only where nothing is there: a file that is there with the same bytes is left alone and counts as written,
//! one with other bytes is not replaced. A download that takes longer than [`DOWNLOAD_WAIT`] is said to be on
//! its way; asking again finds it written.

use crate::commands::{self, Outcome, Outgoing};
use crate::copies::Files;
use crate::delivery::{FILES_STEP, reason_of};
use crate::engine::{Engine, Inner};
use crate::error::EngineError;
use crate::ids;
use crate::manifest::{
    DEFAULT_FILE_MODE, Entry, EntryKind, Header, MANIFEST_VERSION, Manifest, build,
};
use crate::sessions::StartSnapshot;
use crate::sharing::{FileWritten, ParcelFault, Parcels};
use async_trait::async_trait;
use bytes::Bytes;
use orch_blob_contract::store::{read_all, stream_of};
use orch_db::Conn;
use orch_db::queries::environments::thread_engine as environments;
use orch_db::queries::shares::{self as share_rows, CopyParcel, CopyRow};
use orch_db::queries::{storage, threads};
use orch_link_contract::commands::{
    ExpectedHash, FsMaterialize, FsRemove, FsStat, FsWrite, MaterializePurpose, SerialKey,
};
use orch_types::{Hash, Id, Time, new_id};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How long a download is waited for before the agent is told it is on its way: long enough for a file of a few
/// megabytes on a slow link, short enough that a tool call does not hang on a large one.
pub const DOWNLOAD_WAIT: Duration = Duration::from_secs(20);

/// How often the wait for the writing of a copy looks up from it, to see whether the process is stopping.
const RECHECK_EVERY: Duration = Duration::from_secs(2);

/// The code a machine answers a write with when the file is not the version that was expected.
const CONFLICT: &str = "conflict";

/// A copy that waits for its thread's next turn.
const PENDING: &str = "pending";

/// A copy that was written into a workspace.
const DELIVERED: &str = "delivered";

/// What keeps the files of copies and writes downloads, for the engine it belongs to. It holds the engine
/// weakly, since the engine holds it.
pub struct WorkspaceParcels {
    engine: Weak<Inner>,
}

impl WorkspaceParcels {
    /// The keeper of `engine`'s copies, to be attached to it.
    #[must_use]
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: Arc::downgrade(&engine.inner),
        }
    }

    fn engine(&self) -> Result<Engine, ParcelFault> {
        self.engine
            .upgrade()
            .map(|inner| Engine { inner })
            .ok_or_else(|| ParcelFault("the engine has stopped".to_owned()))
    }
}

fn fault(error: EngineError) -> ParcelFault {
    ParcelFault(error.to_string())
}

#[async_trait]
impl Parcels for WorkspaceParcels {
    async fn keep(
        &self,
        conn: &mut Conn,
        _copy: Id,
        path: &str,
        files: &Files,
    ) -> Result<Option<Vec<u8>>, ParcelFault> {
        let engine = self.engine()?;
        let manifest = engine.keep_copy(conn, path, files).await.map_err(fault)?;
        Ok(Some(manifest.as_bytes().to_vec()))
    }

    async fn withdraw(&self, conn: &mut Conn, copy: &CopyRow) -> Result<(), ParcelFault> {
        self.engine()?
            .withdraw_copy(conn, copy)
            .await
            .map_err(fault)
    }

    async fn write_file(
        &self,
        conn: &mut Conn,
        thread: Id,
        path: &str,
        bytes: &[u8],
    ) -> Result<FileWritten, ParcelFault> {
        self.engine()?
            .write_download(conn, thread, path, bytes)
            .await
            .map_err(fault)
    }
}

/// `path` below the thread's directory `directory`, both relative to the workspace's root.
pub(crate) fn within(directory: &str, path: &str) -> String {
    let directory = directory.trim_matches('/');
    let path = path.trim_start_matches('/');
    if directory.is_empty() {
        path.to_owned()
    } else {
        format!("{directory}/{path}")
    }
}

impl Engine {
    /// Puts the files of a copy in the store, writes the list of them, and pins both until the copy is written
    /// into a workspace or taken back. The answer is the hash of the list.
    pub(crate) async fn keep_copy(
        &self,
        conn: &mut Conn,
        path: &str,
        files: &Files,
    ) -> Result<Hash, EngineError> {
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("this system has no store for files"))?;
        let now = self.inner.clock.now();
        let mut entries = Vec::with_capacity(files.files.len());
        let mut pinned = Vec::with_capacity(files.files.len() + 1);
        for file in &files.files {
            let hash = Hash::of(&file.bytes);
            let size = i64::try_from(file.bytes.len()).unwrap_or(i64::MAX);
            if store
                .size(&hash)
                .await
                .map_err(EngineError::unreadable)?
                .is_none()
            {
                store
                    .put(
                        &hash,
                        Some(size),
                        stream_of(vec![Bytes::from(file.bytes.clone())]),
                    )
                    .await
                    .map_err(EngineError::unreadable)?;
            }
            storage::register_blob(conn, &hash, size, now).await?;
            entries.push(Entry {
                path: file.path.clone(),
                kind: EntryKind::File,
                size,
                mtime_ns: now.timestamp_nanos_opt().unwrap_or(0),
                mode: DEFAULT_FILE_MODE,
                hash: Some(hash),
                link: None,
            });
            pinned.push(hash);
        }
        let written = build(
            &Header {
                v: MANIFEST_VERSION,
                root: path.trim_end_matches('/').to_owned(),
                generated_at: Time::new(now),
                generation: 0,
                case_sensitive: true,
            },
            entries,
        )
        .map_err(EngineError::unreadable)?;
        let manifest = self.put_manifest(conn, written.bytes).await?;
        pinned.push(manifest.hash);
        storage::pin_blobs(conn, &pinned).await?;
        Ok(manifest.hash)
    }

    /// The hashes a copy's list holds the pins of: the list itself and every file it names. A list the store
    /// no longer has gives the list's own hash alone.
    async fn listed_hashes(&self, manifest: &Hash) -> Result<Vec<Hash>, EngineError> {
        let mut hashes = vec![*manifest];
        let Some(store) = self.blobs() else {
            return Ok(hashes);
        };
        let read = match store.get(manifest, None).await {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(%error, %manifest, "the list of a copy's files could not be read; its files keep their pins");
                return Ok(hashes);
            }
        };
        let bytes = read_all(read.body).await.map_err(EngineError::unreadable)?;
        let listed = Manifest::parse(&bytes[..]).map_err(EngineError::unreadable)?;
        hashes.extend(listed.hashes());
        Ok(hashes)
    }

    /// Writes what was handed to the thread of a starting turn since its last one into its directory, before the
    /// files of the input: the copies shared with it, and the files of the messages other threads sent it that
    /// this turn tells it of. `None` when the process is stopping: the next one finds the same work.
    pub(crate) async fn deliver_shared(
        &self,
        snap: &StartSnapshot,
    ) -> Result<Option<()>, EngineError> {
        if self.deliver_copies(snap).await?.is_none() {
            return Ok(None);
        }
        self.deliver_notice_files(snap).await
    }

    /// The files of the messages that wait among the thread's notices, written where each message names them.
    /// A file that cannot be written is said in the log; the message still names where it was to be.
    async fn deliver_notice_files(&self, snap: &StartSnapshot) -> Result<Option<()>, EngineError> {
        let Some(notices) = snap.thread.pending_notices.as_array() else {
            return Ok(Some(()));
        };
        let directory = snap.directory();
        for notice in notices {
            let parts = crate::tools::thread_message::file_parts(
                notice.get("files").unwrap_or(&serde_json::Value::Null),
            );
            if parts.is_empty() {
                continue;
            }
            let Some(input) = notice
                .get("id")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| id.parse::<Id>().ok())
            else {
                continue;
            };
            self.set_step(snap.thread.node_id, FILES_STEP).await?;
            let Some(placement) = self
                .deliver_files(&crate::delivery::Delivering {
                    thread: snap.thread.node_id,
                    input,
                    content: &parts,
                    environment: &snap.environment,
                    directory: &directory,
                    sender: None,
                })
                .await?
            else {
                return Ok(None);
            };
            for (at, _) in parts.iter().enumerate() {
                if let Some(why) = placement.refusal_of(crate::delivery::Slot::File(at)) {
                    tracing::warn!(thread = %snap.thread.node_id, message = %input, %why, "a file of a message from another thread could not be written");
                }
            }
        }
        Ok(Some(()))
    }

    /// The copies shared with the thread of a starting turn that wait, written into its directory.
    async fn deliver_copies(&self, snap: &StartSnapshot) -> Result<Option<()>, EngineError> {
        let thread = snap.thread.node_id;
        let waiting = {
            let mut conn = self.inner.db.acquire().await?;
            share_rows::copies_waiting(&mut conn, thread).await?
        };
        if waiting.is_empty() {
            return Ok(Some(()));
        }
        // A copy waits for an environment on a machine; the turn goes on without it.
        let Some(machine) = snap.environment.machine_id else {
            return Ok(Some(()));
        };
        self.set_step(thread, FILES_STEP).await?;
        let directory = snap.directory();
        for copy in waiting {
            let Some(manifest) = copy.manifest else {
                // Nothing was kept to be written: the copy stands as written.
                let mut conn = self.inner.db.acquire().await?;
                share_rows::copy_delivered(
                    &mut conn,
                    copy.id,
                    snap.environment.id,
                    self.inner.clock.now(),
                )
                .await?;
                continue;
            };
            let target_root = format!(
                "{}/{}",
                directory.trim_end_matches('/'),
                copy.path.trim_matches('/')
            );
            let command_id = ids::derived(
                "fs.materialize.copy",
                &[
                    copy.id.as_bytes().as_slice(),
                    machine.as_bytes().as_slice(),
                    manifest.as_bytes().as_slice(),
                ],
            );
            let command = Outgoing::new(
                command_id,
                machine,
                SerialKey::Environment(snap.environment.id),
                &FsMaterialize {
                    work: None,
                    environment_id: snap.environment.id,
                    job_id: None,
                    manifest_blob: manifest,
                    target_root,
                    purpose: MaterializePurpose::Delivery,
                },
            )?;
            let Some(outcome) = self.run_here(command_id, command).await? else {
                return Ok(None);
            };
            match outcome {
                Outcome::Succeeded(_) => {
                    self.copy_written(&copy, &manifest, snap.environment.id)
                        .await?
                }
                Outcome::Failed(error) => {
                    tracing::warn!(copy = %copy.id, reason = %reason_of(&error), "a copy could not be written into the workspace; the next turn tries again");
                }
                Outcome::Expired | Outcome::Cancelled => {
                    tracing::warn!(copy = %copy.id, "the machine did not take a copy up; the next turn tries again");
                }
            }
        }
        Ok(Some(()))
    }

    /// Records a copy as written into `environment` and lets its pins go, once.
    async fn copy_written(
        &self,
        copy: &CopyParcel,
        manifest: &Hash,
        environment: Id,
    ) -> Result<(), EngineError> {
        let hashes = self.listed_hashes(manifest).await?;
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        if share_rows::copy_delivered(&mut tx, copy.id, environment, now).await? {
            storage::unpin_blobs(&mut tx, &hashes, now).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Issues a command and waits for its end here, looking up every few seconds to see whether the process is
    /// stopping. `None` when it is.
    async fn run_here(
        &self,
        command_id: Id,
        command: Outgoing,
    ) -> Result<Option<Outcome>, EngineError> {
        self.inner.commands.issue(command).await?;
        loop {
            match tokio::time::timeout(RECHECK_EVERY, self.inner.commands.wait(command_id)).await {
                Ok(outcome) => return Ok(Some(outcome?)),
                Err(_elapsed) => {
                    if self.is_stopped() {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Takes a copy out of the workspace it was written into, or lets go of what was kept for one that was never
    /// written. Called in the transaction that takes the copy back.
    pub(crate) async fn withdraw_copy(
        &self,
        conn: &mut Conn,
        copy: &CopyRow,
    ) -> Result<(), EngineError> {
        let Some(parcel) = share_rows::copy_parcel_of(conn, copy.id).await? else {
            return Ok(());
        };
        match parcel.state.as_str() {
            PENDING => {
                if let Some(manifest) = parcel.manifest {
                    let hashes = self.listed_hashes(&manifest).await?;
                    storage::unpin_blobs(conn, &hashes, self.inner.clock.now()).await?;
                }
            }
            DELIVERED => {
                if let Some(environment) = parcel.environment_id {
                    self.remove_copy(conn, copy.thread_id, &parcel, environment)
                        .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Asks the machine of `environment` to remove the directory a copy was written into, and that alone. The
    /// command is not waited for: the copy is taken back in the transaction that asked, and the directory goes
    /// when the machine takes the command up.
    async fn remove_copy(
        &self,
        conn: &mut Conn,
        thread: Id,
        parcel: &CopyParcel,
        environment: Id,
    ) -> Result<(), EngineError> {
        let Some(row) = threads::thread(conn, thread).await? else {
            return Ok(());
        };
        let Some(place) = environments::environment(conn, environment).await? else {
            return Ok(());
        };
        let Some(machine) = place.machine_id else {
            tracing::warn!(copy = %parcel.id, "a copy taken back lies in an environment that is on no machine; its directory stays");
            return Ok(());
        };
        let relative = parcel.path.trim_matches('/');
        if relative.is_empty() {
            return Ok(());
        }
        let command_id = ids::derived(
            "fs.remove.copy",
            &[
                parcel.id.as_bytes().as_slice(),
                machine.as_bytes().as_slice(),
            ],
        );
        let command = Outgoing::new(
            command_id,
            machine,
            SerialKey::Environment(environment),
            &FsRemove {
                environment_id: environment,
                path: within(&row.workspace_dir, relative),
                recursive: true,
            },
        )?;
        self.inner.commands.issue(command).await?;
        Ok(())
    }

    /// Writes the bytes an agent downloads at `path` under its thread's directory, where nothing is there yet.
    pub(crate) async fn write_download(
        &self,
        conn: &mut Conn,
        thread: Id,
        path: &str,
        bytes: &[u8],
    ) -> Result<FileWritten, EngineError> {
        let row = threads::thread(conn, thread)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread is gone"))?;
        let environment_id = row.environment_id.ok_or_else(|| {
            EngineError::unreadable("the thread has no environment to write a file into")
        })?;
        let environment = environments::environment(conn, environment_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread's environment is gone"))?;
        let machine = environment
            .machine_id
            .ok_or_else(|| EngineError::unreadable("the environment is not on a machine"))?;
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("this system has no store for files"))?;
        let hash = Hash::of(bytes);
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        if store
            .size(&hash)
            .await
            .map_err(EngineError::unreadable)?
            .is_none()
        {
            store
                .put(
                    &hash,
                    Some(size),
                    stream_of(vec![Bytes::copy_from_slice(bytes)]),
                )
                .await
                .map_err(EngineError::unreadable)?;
        }
        storage::register_blob(conn, &hash, size, self.inner.clock.now()).await?;
        let target = within(&row.workspace_dir, path);
        // Same-call retries keep one native command; another source/ref checks the actual target again.
        let command_id = ids::derived(
            "fs.write.download",
            &[
                thread.as_bytes().as_slice(),
                target.as_bytes(),
                hash.as_bytes().as_slice(),
            ],
        );
        let command = Outgoing::new(
            command_id,
            machine,
            SerialKey::Environment(environment.id),
            &FsWrite {
                work: None,
                environment_id: environment.id,
                path: target.clone(),
                blob: hash,
                mode: Some(DEFAULT_FILE_MODE),
                expected_hash: Some(ExpectedHash::Absent),
            },
        )?;
        let command = crate::tools::source::issue(self, command).await?;
        let command_id = command.id;
        let hash = serde_json::from_value(command.body["blob"].clone())?;
        let outcome =
            match tokio::time::timeout(DOWNLOAD_WAIT, self.inner.commands.wait(command_id)).await {
                Ok(outcome) => outcome?,
                Err(_elapsed) => return Ok(FileWritten::Downloading { received: 0 }),
            };
        match outcome {
            Outcome::Succeeded(_) => Ok(FileWritten::Ready),
            Outcome::Failed(error) if error.code == CONFLICT => {
                self.same_file_there(machine, environment.id, &target, &hash)
                    .await
            }
            Outcome::Failed(error) => Err(EngineError::unreadable(reason_of(&error))),
            Outcome::Expired | Outcome::Cancelled => Err(EngineError::unreadable(
                "the machine did not take the file up",
            )),
        }
    }

    /// Whether the file that stood in the way of a download holds the same bytes: then the download is there.
    pub(crate) async fn same_file_there(
        &self,
        machine: Id,
        environment: Id,
        target: &str,
        hash: &Hash,
    ) -> Result<FileWritten, EngineError> {
        let command = Outgoing::new(
            new_id(),
            machine,
            SerialKey::None,
            &FsStat {
                environment_id: environment,
                mount: None,
                path: target.to_owned(),
            },
        )?;
        let there = match commands::run(&*self.inner.commands, command).await? {
            Outcome::Succeeded(stat) => stat
                .get("hash")
                .cloned()
                .and_then(|value| serde_json::from_value::<Hash>(value).ok()),
            _ => None,
        };
        Ok(if there.as_ref() == Some(hash) {
            FileWritten::Ready
        } else {
            FileWritten::Differs
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_lies_below_the_threads_directory_and_the_root_has_none() {
        assert_eq!(
            within("web/api", ".orch/shared/x"),
            "web/api/.orch/shared/x"
        );
        assert_eq!(within("/web/api/", "/notes.md"), "web/api/notes.md");
        assert_eq!(within("", ".orch/shared/x"), ".orch/shared/x");
    }
}
