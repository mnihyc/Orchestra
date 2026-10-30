//! Files an agent attaches: read by its environment's machine, verified in the blob store, and limited as people's uploads are.

use crate::commands::{self, Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::parcels::within;
use crate::tools::definitions::{Code, Refusal};
use orch_api_contract::messages::for_failure;
use orch_api_contract::workspace::{DirEntryKind, FileStat};
use orch_db::queries::{environments::thread_engine as environments, storage, threads};
use orch_link_contract::commands::{ErrorCode, FsFetch, FsFetchResult, FsStat, SerialKey};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, new_id};

/// A path accepted by the workspace file operations, in characters.
const PATH_MAX_CHARS: usize = 4_096;
/// Unknown file types are downloads, as with an ordinary attachment.
const UNKNOWN_MEDIA_TYPE: &str = "application/octet-stream";

/// A whole file already held by the store; its destination pins it in its own write transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceFile {
    pub(crate) name: String,
    pub(crate) size: i64,
    pub(crate) blob: Hash,
    pub(crate) media_type: String,
    pub(crate) is_text: bool,
}

/// The same relative-path rules for documents, chats and messages to another thread.
pub(crate) fn relative_file(path: &str) -> bool {
    !path.is_empty()
        && path.chars().count() <= PATH_MAX_CHARS
        && !path.starts_with(['/', '\\'])
        && !path.chars().any(char::is_control)
        && !path.contains(['\\', ':'])
        && path.split('/').all(|part| part != "..")
        && path
            .rsplit('/')
            .next()
            .is_some_and(|name| !matches!(name.trim(), "" | "."))
}

/// The original workspace changed while its files were being read; none of the destination is written.
pub(crate) fn changed() -> Refusal {
    Refusal::new(Code::Changed, for_failure("workspace_file_changed"))
}

/// The sending thread must still name the directory from which the files were read.
pub(crate) fn same_workspace(before: &threads::ThreadRow, current: &threads::ThreadRow) -> bool {
    before.environment_id == current.environment_id && before.workspace_dir == current.workspace_dir
}

fn refused(path: &str, code: Code, words: &str) -> Refusal {
    Refusal::new(code, format!("{path:?}: {}", for_failure(words)))
}

/// The refusal of a path that names no file of the thread's workspace, the same wherever such a path is given;
/// none for a path that may name one.
pub(crate) fn path_refusal(path: &str) -> Option<Refusal> {
    if relative_file(path) {
        None
    } else {
        Some(refused(path, Code::Invalid, "workspace_file_path"))
    }
}

struct Reader {
    environment: Id,
    machine: Id,
    largest: i64,
}

impl Engine {
    /// Reads files for any agent writer, after its destination's write access was checked.
    /// No transaction or pooled connection is held while the machine or store is awaited.
    pub(crate) async fn read_workspace_files(
        &self,
        thread: &threads::ThreadRow,
        paths: &[String],
        largest: i64,
    ) -> Result<Result<Vec<WorkspaceFile>, Refusal>, EngineError> {
        if paths.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        for path in paths {
            if let Some(refusal) = path_refusal(path) {
                return Ok(Err(refusal));
            }
            if path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .trim()
                .chars()
                .count()
                > orch_chat::attachments::NAME_MAX_CHARS
            {
                return Ok(Err(refused(path, Code::Invalid, "workspace_file_name")));
            }
        }
        if thread.environment_id.is_none() {
            return Ok(Err(Refusal::new(
                Code::Closed,
                for_failure("workspace_file_environment"),
            )));
        }
        let (source, largest, online) = {
            let mut conn = self.inner.db.acquire().await?;
            let source = match thread.environment_id {
                Some(id) => environments::environment(&mut conn, id).await?,
                None => None,
            };
            let system =
                orch_settings::get::<i64>(&mut conn, Place::System, keys::STORAGE_MAX_BLOB_BYTES)
                    .await?
                    .ok_or_else(|| {
                        EngineError::unreadable("the upload size limit has no resolved value")
                    })?;
            let online = match source.as_ref().and_then(|source| source.machine_id) {
                Some(machine) => orch_db::queries::resources::machine(&mut conn, machine)
                    .await?
                    .is_some_and(|machine| {
                        machine.online && machine.status == "active" && machine.deleted_at.is_none()
                    }),
                None => false,
            };
            (source, largest.min(system), online)
        };
        let Some(source) = source
            .filter(|source| online && source.status == "running" && source.machine_id.is_some())
        else {
            return Ok(Err(Refusal::new(
                Code::Closed,
                for_failure("workspace_file_away"),
            )));
        };
        let Some(machine) = source.machine_id else {
            return Ok(Err(Refusal::new(
                Code::Closed,
                for_failure("workspace_file_away"),
            )));
        };
        let Some(store) = self.blobs() else {
            return Ok(Err(Refusal::new(
                Code::Closed,
                for_failure("workspace_file_store"),
            )));
        };
        let reader = Reader {
            environment: source.id,
            machine,
            largest,
        };
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let relative = within(&thread.workspace_dir, path);
            match self
                .read_workspace_file(&reader, path, &relative, &*store)
                .await?
            {
                Ok(file) => files.push(file),
                Err(refusal) => return Ok(Err(refusal)),
            }
        }
        // Reads and uploads above may take time. Only metadata work happens in this transaction.
        let mut tx = self.inner.db.begin().await?;
        super::source::check(&mut tx).await?;
        let current = threads::lock(&mut tx, thread.node_id).await?;
        let current_source = environments::environment(&mut tx, source.id).await?;
        if !current.is_some_and(|current| {
            current.environment_id == thread.environment_id
                && current.workspace_dir == thread.workspace_dir
        }) || !current_source.is_some_and(|current| {
            current.machine_id == Some(machine)
                && current.generation == source.generation
                && current.status == "running"
        }) {
            return Ok(Err(changed()));
        }
        let hashes: Vec<Hash> = files.iter().map(|file| file.blob).collect();
        orch_db::queries::uploads::lock_blobs(&mut tx, &hashes).await?;
        for file in &files {
            storage::register_blob(&mut tx, &file.blob, file.size, self.inner.clock.now()).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Ok(files))
    }

    async fn read_workspace_file(
        &self,
        reader: &Reader,
        given: &str,
        relative: &str,
        store: &dyn orch_blob_contract::store::BlobStore,
    ) -> Result<Result<WorkspaceFile, Refusal>, EngineError> {
        let stat = Outgoing::new(
            new_id(),
            reader.machine,
            SerialKey::None,
            &FsStat {
                environment_id: reader.environment,
                mount: None,
                path: relative.to_owned(),
            },
        )?;
        let found: FileStat = match self.workspace_file_reply(given, stat).await {
            Ok(value) => serde_json::from_value(value)?,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if !matches!(found.entry.kind, DirEntryKind::File | DirEntryKind::Symlink) {
            return Ok(Err(refused(given, Code::Invalid, "workspace_file_read")));
        }
        if found.entry.size < 0 || found.entry.size > reader.largest {
            return Ok(Err(refused(
                given,
                Code::Invalid,
                "workspace_file_too_large",
            )));
        }
        // The connector resolves symlinks inside the environment and uploads the entire opened file.
        let fetch = Outgoing::new(
            new_id(),
            reader.machine,
            SerialKey::None,
            &FsFetch {
                environment_id: reader.environment,
                mount: None,
                path: relative.to_owned(),
                expect: None,
                max_bytes: Some(reader.largest),
            },
        )?;
        let fetched: FsFetchResult = match self.workspace_file_reply(given, fetch).await {
            Ok(value) => serde_json::from_value(value)?,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if fetched.size < 0 || fetched.size > reader.largest {
            return Ok(Err(refused(
                given,
                Code::Invalid,
                "workspace_file_too_large",
            )));
        }
        // Keep the native bounded-sniff facts tied to the version that was actually uploaded.
        if found.entry.size != fetched.size
            || found.entry.mtime.get().timestamp_millis()
                != fetched.mtime_ns.div_euclid(storage::NANOS_PER_MILLI)
            || found.hash.is_some_and(|hash| hash != fetched.hash)
        {
            return Ok(Err(changed()));
        }
        if store
            .size(&fetched.hash)
            .await
            .map_err(EngineError::unreadable)?
            != Some(fetched.size)
        {
            return Ok(Err(refused(given, Code::Closed, "workspace_file_store")));
        }
        Ok(Ok(WorkspaceFile {
            name: given.rsplit('/').next().unwrap_or(given).trim().to_owned(),
            blob: fetched.hash,
            size: fetched.size,
            media_type: if found.media_type.trim().is_empty() {
                UNKNOWN_MEDIA_TYPE.to_owned()
            } else {
                found.media_type
            },
            is_text: found.is_text,
        }))
    }

    /// The environment and the machine that hold the thread's workspace, where the machine is there to be asked.
    pub(crate) async fn workspace_target(
        &self,
        thread: &threads::ThreadRow,
    ) -> Result<Result<(Id, Id), Refusal>, EngineError> {
        let Some(environment) = thread.environment_id else {
            return Ok(Err(Refusal::new(
                Code::Closed,
                for_failure("workspace_file_environment"),
            )));
        };
        let away = || Refusal::new(Code::Closed, for_failure("workspace_file_away"));
        let mut conn = self.inner.db.acquire().await?;
        let Some(source) = environments::environment(&mut conn, environment).await? else {
            return Ok(Err(away()));
        };
        let Some(machine) = source.machine_id else {
            return Ok(Err(away()));
        };
        let online = orch_db::queries::resources::machine(&mut conn, machine)
            .await?
            .is_some_and(|machine| {
                machine.online && machine.status == "active" && machine.deleted_at.is_none()
            });
        if !online || source.status != "running" {
            return Ok(Err(away()));
        }
        Ok(Ok((source.id, machine)))
    }

    /// What kind of thing a workspace path is, if it is there.
    pub(crate) async fn workspace_kind(
        &self,
        thread: &threads::ThreadRow,
        path: &str,
    ) -> Result<Result<DirEntryKind, Refusal>, EngineError> {
        let (environment, machine) = match self.workspace_target(thread).await? {
            Ok(target) => target,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let stat = Outgoing::new(
            new_id(),
            machine,
            SerialKey::None,
            &FsStat {
                environment_id: environment,
                mount: None,
                path: within(&thread.workspace_dir, path),
            },
        )?;
        let found: FileStat = match self.workspace_file_reply(path, stat).await {
            Ok(value) => serde_json::from_value(value)?,
            Err(refusal) => return Ok(Err(refusal)),
        };
        Ok(Ok(found.entry.kind))
    }

    /// The entries of a workspace directory, as the machine lists them: all of them, or a refusal when the directory
    /// holds more than a listing gives.
    pub(crate) async fn workspace_list(
        &self,
        thread: &threads::ThreadRow,
        path: &str,
    ) -> Result<Result<Vec<orch_api_contract::workspace::DirEntry>, Refusal>, EngineError> {
        let (environment, machine) = match self.workspace_target(thread).await? {
            Ok(target) => target,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let list = Outgoing::new(
            new_id(),
            machine,
            SerialKey::None,
            &orch_link_contract::commands::FsList {
                environment_id: environment,
                mount: None,
                path: within(&thread.workspace_dir, path),
                watch_id: None,
                watch_instance: None,
                page: None,
            },
        )?;
        let listed: orch_link_contract::commands::FsListResult =
            match self.workspace_file_reply(path, list).await {
                Ok(value) => serde_json::from_value(value)?,
                Err(refusal) => return Ok(Err(refusal)),
            };
        if listed.truncated {
            return Ok(Err(Refusal::invalid(format!(
                "{path:?} holds more entries than one listing gives. Import its subdirectories one at a time."
            ))));
        }
        Ok(Ok(listed.entries))
    }

    async fn workspace_file_reply(
        &self,
        path: &str,
        command: Outgoing,
    ) -> Result<orch_types::Json, Refusal> {
        match commands::run(&*self.inner.commands, command).await {
            Ok(Outcome::Succeeded(value)) => Ok(value),
            Ok(Outcome::Failed(error)) => Err(match error.known() {
                Some(ErrorCode::TooLarge) => {
                    refused(path, Code::Invalid, "workspace_file_too_large")
                }
                Some(ErrorCode::Changed) => changed(),
                _ => refused(path, Code::NotFound, "workspace_file_read"),
            }),
            Ok(Outcome::Expired | Outcome::Cancelled) | Err(_) => {
                Err(refused(path, Code::Closed, "workspace_file_away"))
            }
        }
    }
}
