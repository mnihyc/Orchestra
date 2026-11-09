//! A thread's directory kept as of a moment: taken as a job, listed, read file by file, downloaded as one stream, renamed and deleted; and who may read and cancel the jobs of the workspace.
//!
//! A snapshot belongs to the thread it was taken through and goes where that thread goes: whoever
//! may read the files of the environment the thread runs in now, `files.view` there, may read it (and
//! whoever may write them, `files.manage`, takes, renames and deletes snapshots). It is taken by a
//! job that the engine carries out with the machine ([`orch_engine::jobs::snapshot`]), so the
//! request answers at once with the job, whose result names the snapshot when it ends. What a snapshot
//! holds is read from its list in the store, and every file it names is pinned there for as long as
//! the snapshot exists: deleting it lets them go. Offline, a snapshot is still read whole, since
//! it needs no machine.

use super::files;

use super::listing;

use super::live;

use super::paths;

use super::snapshot_read;

use crate::cache::Entry;

use crate::cache::EntryKind;

use crate::cache::Manifest;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::resources::view::addresses;

use crate::routes::resources::view::parse;

use crate::sign_in::origin;

use axum::body::Body;

use axum::response::Response;

use bytes::Bytes;

use futures::StreamExt;

use http::HeaderValue;

use http::StatusCode;

use http::header;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::environment::EnvironmentRef;

use orch_api_contract::workspace::DirListing;

use orch_api_contract::workspace::EnvironmentFilesExport;

use orch_api_contract::workspace::EnvironmentSnapshotList;

use orch_api_contract::workspace::Job;

use orch_api_contract::workspace::Snapshot;

use orch_api_contract::workspace::SnapshotArchiveFormat;

use orch_api_contract::workspace::SnapshotArchiveGet;

use orch_api_contract::workspace::SnapshotArchiveGetQuery;

use orch_api_contract::workspace::SnapshotDelete;

use orch_api_contract::workspace::SnapshotFileContentGet;

use orch_api_contract::workspace::SnapshotFileList;

use orch_api_contract::workspace::SnapshotGet;

use orch_api_contract::workspace::SnapshotPathQuery;

use orch_api_contract::workspace::SnapshotPreflight;

use orch_api_contract::workspace::SnapshotSelection;

use orch_api_contract::workspace::SnapshotSelectionRefusal;

use orch_api_contract::workspace::SnapshotState;

use orch_api_contract::workspace::SnapshotUpdate;

use orch_api_contract::workspace::ThreadSnapshotCreate;

use orch_api_contract::workspace::ThreadSnapshotCreateBody;

use orch_api_contract::workspace::ThreadSnapshotCreateSelection;

use orch_api_contract::workspace::ThreadSnapshotList;

use orch_api_contract::workspace::ThreadSnapshotPreflight;

use orch_api_contract::workspace::ThreadSnapshotPreflightBody;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_blob_contract::store::BlobError;

use orch_blob_contract::store::BlobStore;

use orch_blob_contract::store::ByteStream;

use orch_db::audit;

use orch_db::audit::Actor;

use orch_db::audit::Entry as AuditEntry;

use orch_db::queries::environments::workspace as environment_rows;

use orch_db::queries::storage;

use orch_db::queries::storage::SnapshotRow;

use orch_engine::jobs::snapshot::Asked;

use orch_types::Id;

use orch_types::Time;

use std::collections::HashMap;

use std::io::Read;

use std::io::Write;

use std::sync::Arc;
/// How long a label of a snapshot may be: as long as a name of a thread.
const LABEL_MAX_CHARS: usize = 120;

/// How much of an archive is collected before it is sent on: the size of a piece of the link.
const ARCHIVE_PIECE_BYTES: usize = 64 * 1024;

/// How many pieces of an archive wait for a reader that is slower than the writer.
const ARCHIVE_QUEUE_PIECES: usize = 8;

/// How hard an archive is compressed: zstd's default, which is quick and gets most of the gain.
const ARCHIVE_COMPRESSION_LEVEL: i32 = 3;

/// The permission bits of a directory that a list implies.
const DIRECTORY_MODE: u32 = 0o755;

/// The seconds in a day, for the default label.
const LABEL_TIME_FORMAT: &str = "%Y-%m-%d %H:%M UTC";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(
            ThreadSnapshotCreate,
            creation::binding::<ThreadSnapshotCreateBody, true>(),
        )
        .creation(EnvironmentFilesExport, creation::binding::<(), false>())
        .on_with_operation(
            EnvironmentFilesExport,
            crate::key_operations::native::effects::prepare::<creation::Export>,
            creation::export,
        );
    registry
        .on_snapshot_read_with_optional_operation(
            EnvironmentSnapshotList,
            crate::key_operations::native::selected::<environment_list::Snapshots>,
            crate::key_operations::native::selected_read::<environment_list::Snapshots>,
        )
        .on_snapshot_read_with_optional_operation(
            ThreadSnapshotList,
            crate::key_operations::native::selected::<read::ThreadSnapshots>,
            crate::key_operations::native::selected_read::<read::ThreadSnapshots>,
        )
        .on_with_initial_operation(
            ThreadSnapshotCreate,
            crate::key_operations::native::effects::initial::<creation::Create>,
            crate::key_operations::native::effects::prepare::<creation::Create>,
            creation::create,
        )
        .on_snapshot_body_read_with_optional_initial_operation(
            ThreadSnapshotPreflight,
            crate::key_operations::native::effects::initial::<preflight::Measure>,
            crate::key_operations::native::selected_body::<
                preflight::Measure,
                (),
                ThreadSnapshotPreflightBody,
            >,
            crate::key_operations::native::selected_body_read::<
                preflight::Measure,
                (),
                ThreadSnapshotPreflightBody,
            >,
        )
        .on_with_operation(SnapshotGet, snapshot_read::prepare, get)
        .on_with_operation(SnapshotFileList, snapshot_read::prepare, file_list)
        .on_raw_read_with_operation(SnapshotFileContentGet, snapshot_read::prepare, file_content)
        .on_raw_read_with_operation(SnapshotArchiveGet, snapshot_read::prepare, archive)
        .on_with_operation(
            SnapshotUpdate,
            crate::key_operations::native::effects::prepare::<update::Update>,
            update::run,
        )
        .on_with_initial_operation(
            SnapshotDelete,
            crate::key_operations::native::effects::initial::<deletion::Delete>,
            crate::key_operations::native::effects::prepare::<deletion::Delete>,
            deletion::run,
        );
}

// ---------------------------------------------------------------- views

fn state_of(row: &SnapshotRow, now: Time) -> ApiResult<SnapshotState> {
    if row.storage == "environment"
        && row.state == "complete"
        && row.expires_at.is_some_and(|expires| expires <= now.get())
    {
        return Ok(SnapshotState::Expired);
    }
    parse(&row.state)
}

/// Snapshots as the API shows them, for any number of them in the same few statements.
pub(crate) async fn views(
    conn: &mut orch_db::Conn,
    rows: Vec<SnapshotRow>,
    now: Time,
) -> ApiResult<Vec<Snapshot>> {
    let environments: Vec<Id> = rows.iter().map(|row| row.environment_id).collect();
    let mut found = HashMap::new();
    for id in &environments {
        if found.contains_key(id) {
            continue;
        }
        if let Some(row) = environment_rows::environment(conn, *id).await? {
            found.insert(*id, row);
        }
    }
    let exposed = crate::routes::ports::exposures(conn, &environments, now).await?;
    let addresses = addresses(conn, &environments).await?;
    let mut people: Vec<Id> = rows.iter().filter_map(|row| row.created_by).collect();
    people.extend(found.values().map(|row| row.owner_id));
    let people = refusals::user_refs(conn, &people).await?;
    rows.into_iter()
        .map(|row| {
            let environment = found
                .get(&row.environment_id)
                .ok_or_else(|| ApiError::internal("the environment of a snapshot is not there"))?;
            let selection: SnapshotSelection = serde_json::from_value(row.selection.clone())
                .unwrap_or(SnapshotSelection {
                    include: Vec::new(),
                    exclude: Vec::new(),
                });
            Ok(Snapshot {
                admin_access: None,
                id: row.id,
                thread_id: row.thread_id,
                environment: EnvironmentRef {
                    activity: None,
                    ports_exposed: exposed.contains(&environment.id),
                    id: environment.id,
                    name: environment.name.clone(),
                    owner: people.get(&environment.owner_id).cloned().ok_or_else(|| {
                        ApiError::internal("the owner of an environment is not there")
                    })?,
                    address: addresses.get(&environment.id).cloned().unwrap_or_default(),
                    placement: parse(&environment.placement)?,
                    status: parse(&environment.status)?,
                },
                label: row.label.clone(),
                kind: parse(&row.kind)?,
                storage: parse(&row.storage)?,
                repository_head: row.repository_head.clone(),
                directory: row.directory.clone(),
                expires_at: row.expires_at.map(Time::new),
                unavailable_reason: row.unavailable_reason.clone(),
                state: state_of(&row, now)?,
                selection,
                file_count: row.file_count,
                total_bytes: row.total_bytes,
                missing_count: row.missing_count,
                env_generation: i64::from(row.env_generation),
                created_by: row.created_by.and_then(|id| people.get(&id).cloned()),
                created_at: Time::new(row.created_at),
                completed_at: row.completed_at.map(Time::new),
            })
        })
        .collect()
}

async fn view(conn: &mut orch_db::Conn, row: SnapshotRow, now: Time) -> ApiResult<Snapshot> {
    views(conn, vec![row], now)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("a snapshot was read and is not shown"))
}

/// Requires `permission` on the environment the thread `thread` runs in now: a snapshot goes where
/// its thread goes, and the files of an environment are given on its page. A thread that runs in no
/// environment has none to hold anything on, and its snapshots are nobody's to read.
async fn require_where_it_runs(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    thread: Id,
    permission: Permission,
) -> ApiResult<()> {
    let environment = environment_rows::thread_place(conn, thread)
        .await?
        .and_then(|place| place.environment_id)
        .ok_or_else(ApiError::not_found)?;
    authorization::require_resource(
        conn,
        principal,
        ResourceKind::Environment,
        environment,
        permission,
        Policy::default(),
    )
    .await
}

// ---------------------------------------------------------------- the snapshots of a thread

/// A cursor over a thread's snapshots: where the last of the page before was.
fn cursor_of(row: &SnapshotRow) -> String {
    format!("{}.{}", row.created_at.timestamp_micros(), row.id)
}

fn after_of(cursor: &str) -> ApiResult<(chrono::DateTime<chrono::Utc>, Id)> {
    let bad = || ApiError::invalid("cursor", "this is not a place in the list");
    let (micros, id) = cursor.split_once('.').ok_or_else(bad)?;
    let at = chrono::DateTime::from_timestamp_micros(micros.parse().map_err(|_| bad())?)
        .ok_or_else(bad)?;
    Ok((at, id.parse().map_err(|_| bad())?))
}

/// A label for a snapshot that was not given one.
fn label_of(ctx: &Ctx, given: Option<&str>) -> ApiResult<String> {
    match given.map(str::trim) {
        Some(label) if !label.is_empty() => {
            if label.chars().count() > LABEL_MAX_CHARS || label.chars().any(char::is_control) {
                return Err(ApiError::invalid(
                    "label",
                    &format!("a label has at most {LABEL_MAX_CHARS} characters"),
                ));
            }
            Ok(label.to_owned())
        }
        _ => Ok(format!("Snapshot {}", ctx.now.format(LABEL_TIME_FORMAT))),
    }
}

/// The native current source shared by preflight and creation.
async fn snapshot_environment(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
) -> ApiResult<(environment_rows::EnvironmentRow, String)> {
    let place = environment_rows::thread_place(conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let environment = match place.environment_id {
        Some(id) => environment_rows::environment(conn, id).await?,
        None => None,
    }
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::EnvironmentNoneUsable,
            "this thread runs in no environment",
        )
    })?;
    Ok((environment, place.workspace_dir))
}

async fn require_snapshot(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    environment: Id,
) -> ApiResult<()> {
    authorization::require_resource(
        conn,
        principal,
        ResourceKind::Environment,
        environment,
        Permission::FilesManage,
        Policy::default(),
    )
    .await
}

/// The original native validation and physical selection, without dispatch or a job.
async fn snapshot_paths(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    environment: &environment_rows::EnvironmentRow,
    directory: &str,
    selection: Option<&ThreadSnapshotCreateSelection>,
) -> ApiResult<(Id, orch_link_contract::commands::Selection)> {
    let (include, exclude) = match selection {
        Some(selection) => {
            let clean = |list: &[String], field: &str| -> ApiResult<Vec<String>> {
                list.iter()
                    .map(|each| {
                        let pattern = paths::clean(field, each, true)?;
                        orch_link_contract::commands::validate_selection_pattern(&pattern)
                            .map_err(|reason| ApiError::invalid(field, reason))?;
                        Ok(pattern)
                    })
                    .collect()
            };
            if selection.include.is_empty() {
                return Err(ApiError::invalid(
                    "selection.include",
                    "a selection names a path",
                ));
            }
            (
                clean(&selection.include, "selection.include")?,
                clean(
                    selection.exclude.as_deref().unwrap_or(&[]),
                    "selection.exclude",
                )?,
            )
        }
        None => {
            if !listing::indexed(environment) {
                return Err(ApiError::new(
                    ErrorCode::WorkspaceNotIndexed,
                    "the tree is too large to index; a snapshot of it names the paths it takes",
                ));
            }
            (vec![directory.to_owned()], Vec::new())
        }
    };
    let machine = live::machine_of(ctx, conn, environment)
        .await?
        .ok_or_else(live::offline)?;
    Ok((
        machine,
        orch_link_contract::commands::Selection { include, exclude },
    ))
}

fn measurement_view(
    selection: &orch_link_contract::commands::Selection,
    measured: orch_link_contract::commands::FsMeasureResult,
) -> ApiResult<SnapshotPreflight> {
    let refusal: Option<SnapshotSelectionRefusal> =
        serde_json::from_value(serde_json::to_value(measured.refusal).map_err(ApiError::internal)?)
            .map_err(ApiError::internal)?;
    let reason = refusal.map(|reason| {
        match reason {
            SnapshotSelectionRefusal::EntryLimit => {
                "The selection has more than 250,000 entries; choose narrower paths."
            }
            SnapshotSelectionRefusal::ScanTime => {
                "Measuring the selection exceeded ten seconds; choose narrower paths."
            }
            SnapshotSelectionRefusal::Unreadable => {
                "Some selected entries could not be read; their size cannot be confirmed."
            }
            SnapshotSelectionRefusal::SelectionRequired => {
                "This workspace is listed live; choose explicit paths before taking a snapshot."
            }
        }
        .to_owned()
    });
    let estimate = orch_engine::jobs::estimates::estimate(
        &orch_engine::jobs::estimates::Work {
            bytes_up: measured.total_bytes,
            files: measured.file_count,
            ..Default::default()
        },
        &orch_engine::jobs::estimates::Rates::assumed(),
    );
    Ok(SnapshotPreflight {
        selection: SnapshotSelection {
            include: selection.include.clone(),
            exclude: selection.exclude.clone(),
        },
        entry_count: measured.entry_count,
        file_count: measured.file_count,
        directory_count: measured.directory_count,
        total_bytes: measured.total_bytes,
        largest_directories: serde_json::from_value(
            serde_json::to_value(measured.largest_directories).map_err(ApiError::internal)?,
        )
        .map_err(ApiError::internal)?,
        allowed: measured.complete && refusal.is_none(),
        refusal,
        reason,
        max_entries: i64::try_from(orch_link_contract::commands::SNAPSHOT_MAX_ENTRIES)
            .unwrap_or(i64::MAX),
        scan_seconds: orch_link_contract::commands::SNAPSHOT_SCAN_SECONDS,
        estimate,
    })
}

/// One snapshot.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Snapshot> {
    // Recovery is factual preparation; retain only the final authenticated response tuple.
    let (source, accepted) = snapshot_read::open(&ctx, false).await?;
    let row = source.row;
    let recovery = row.kind == "turn"
        && row.state == "unavailable"
        && row.unavailable_reason.as_deref() == Some("store_unavailable");
    if recovery && let Err(error) = ctx.app.engine.recover_recording(row.id).await {
        tracing::warn!(%error, snapshot=%row.id, "the retained recording could not be revalidated");
    }
    let (reply, read) = crate::key_operations::snapshot_read::run(ctx.clone(), move |ctx| {
        let accepted = accepted.clone();
        async move {
            let mut tx = ctx.app.db.begin_snapshot().await?;
            let (source, principal) = if recovery {
                accepted.refreshed_in(&mut tx).await?
            } else {
                accepted.current_in(&mut tx).await?
            };
            let read = snapshot_read::Read::new(&ctx, &source, &principal)?;
            let inspection = source
                .row
                .thread_id
                .is_none()
                .then(|| {
                    orch_engine::archive::custody_inspection(&principal, source.row.created_by)
                })
                .flatten();
            let mut reply = view(&mut tx, source.row, Time::new(ctx.app.clock.now())).await?;
            reply.admin_access = inspection;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            read.publish()?;
            Ok((reply, read))
        }
    })
    .await?;
    read.check().await?;
    Ok(reply)
}

// ---------------------------------------------------------------- what a snapshot holds

async fn recorded_read(
    ctx: &Ctx,
    row: &SnapshotRow,
    path: Option<String>,
    export: bool,
    read: &snapshot_read::Read,
) -> ApiResult<orch_link_contract::source::FsRecordReadResult> {
    use crate::thread_engine::DispatchCommands;
    use orch_engine::archive::snapshot_authority::SnapshotReadAuthority;
    use orch_engine::commands::{CommandAuthority, Commands, Outgoing};
    use orch_link_contract::source::FsRecordRead;
    if row.expires_at.is_some_and(|end| end <= ctx.app.clock.now())
        || row.state != storage::snapshot_state::COMPLETE
    {
        return Err(ApiError::new(
            ErrorCode::WorkspaceRestoreUnavailable,
            "the complete recorded state is unavailable",
        )
        .with("snapshot_id", row.id)
        .with(
            "reason",
            row.unavailable_reason
                .as_deref()
                .unwrap_or("expired_or_incomplete"),
        ));
    }
    let accepted = read.clone();
    let command = crate::key_operations::snapshot_read::run(ctx.clone(), move |ctx| {
        let (accepted, path) = (accepted.clone(), path.clone());
        async move {
            let mut tx = ctx.app.db.begin_snapshot().await?;
            let (source, principal) = accepted.new_command_in(&mut tx).await?;
            let environment = environment_rows::environment(&mut tx, source.row.environment_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let machine = live::require_live(&ctx, &mut tx, &environment).await?;
            let command = Outgoing::new(
                orch_types::new_id(),
                machine,
                orch_link_contract::commands::SerialKey::None,
                &FsRecordRead {
                    environment_id: source.row.environment_id,
                    snapshot_id: source.row.id,
                    path,
                    export,
                },
            )
            .map_err(ApiError::internal)?;
            let origin = SnapshotReadAuthority::capture(
                principal.caller(),
                &source.row,
                machine,
                environment.generation,
                &environment.workspace_root,
                &command,
            )
            .map_err(snapshot_read::engine_error)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            accepted.publish()?;
            Ok(command.with_authority(CommandAuthority::snapshot_read(origin)))
        }
    })
    .await?;
    let dispatcher = ctx.app.link.dispatcher();
    let id = command.id;
    let unavailable = || {
        ApiError::new(
            ErrorCode::WorkspaceRestoreUnavailable,
            "the recorded state could not be read completely",
        )
        .with("snapshot_id", row.id)
    };
    DispatchCommands::new(dispatcher.clone())
        .issue(command)
        .await
        .map_err(|_| unavailable())?;
    let result = dispatcher
        .wait_for::<FsRecordRead>(id)
        .await
        .map_err(|_| unavailable())?;
    let mut conn = ctx.app.db.acquire().await?;
    // This exact Accepted command's upload/result is a fact. Its registration survives revocation;
    // public readback below still requires current authority and the same Snapshot association.
    storage::register_blob(&mut conn, &result.blob, result.size, ctx.app.clock.now()).await?;
    drop(conn);
    read.check().await?;
    Ok(result)
}

/// The list of a snapshot, if it has one.
async fn tree_of(ctx: &Ctx, row: &SnapshotRow) -> ApiResult<Arc<Manifest>> {
    let manifest = row.manifest_blob.ok_or_else(ApiError::not_found)?;
    ctx.app
        .cache
        .manifest_of(&ctx.app.blobs, row.id, &manifest)
        .await
        .map_err(ApiError::internal)
}

/// A directory of a snapshot.
async fn file_list(ctx: Ctx, query: SnapshotPathQuery, (): ()) -> ApiResult<DirListing> {
    let (source, read) = snapshot_read::open(&ctx, true).await?;
    let row = source.row;
    let path = paths::clean("path", &query.path, true)?;
    if row.storage == "environment" {
        let checked = recorded_read(&ctx, &row, None, false, &read).await?;
        if Some(checked.blob) != row.manifest_blob {
            return Err(ApiError::new(
                ErrorCode::WorkspaceRestoreUnavailable,
                "the recorded manifest changed",
            )
            .with("reason", "store_changed"));
        }
    }
    let tree = tree_of(&ctx, &row).await;
    read.check().await?;
    let tree = tree?;
    let mut entries = listing::from_manifest(&tree, &path).ok_or_else(ApiError::not_found)?;
    // What a snapshot lists with a hash is in the store, pinned.
    for entry in &mut entries {
        entry.cached = row.storage == "blobs"
            && tree
                .get(&entry.path)
                .is_some_and(|found| found.kind == EntryKind::File && found.hash.is_some());
    }
    listing::sort(&mut entries);
    let truncated = listing::cut(&mut entries);
    read.check().await?;
    Ok(DirListing {
        path,
        mount: None,
        source: orch_api_contract::forks::FilesSource::Cached,
        as_of: Time::new(row.completed_at.unwrap_or(row.created_at)),
        observed_from: None,
        entries,
        truncated,
        revision: None,
        next_cursor: None,
        expires_at: None,
    })
}

/// The bytes of one file of a snapshot.
async fn file_content(ctx: Ctx, query: SnapshotPathQuery, _: Body) -> ApiResult<Response> {
    let (source, read) = snapshot_read::open(&ctx, true).await?;
    let row = source.row;
    let path = paths::clean("path", &query.path, false)?;
    let tree = tree_of(&ctx, &row).await;
    read.check().await?;
    let tree = tree?;
    let hash = tree
        .get(&path)
        .filter(|entry| entry.kind == EntryKind::File)
        .and_then(|entry| entry.hash)
        .ok_or_else(ApiError::not_found)?;
    if row.storage == "environment" {
        let pulled = recorded_read(&ctx, &row, Some(path.clone()), false, &read).await?;
        if pulled.blob != hash {
            return Err(ApiError::new(
                ErrorCode::WorkspaceRestoreUnavailable,
                "the recorded file does not match the saved manifest",
            )
            .with("reason", "store_changed"));
        }
    }
    let name = paths::name_of(&path);
    read.check().await?;
    let response = files::read_for(
        &ctx,
        &hash,
        name,
        crate::media::media_type(name, &[]),
        false,
    )
    .await;
    read.check().await?;
    read.response(response?).await
}

// ---------------------------------------------------------------- as one stream

/// A writer whose bytes go to a reader of a response, a piece at a time; one that is gone ends the
/// writing.
struct Sink {
    pieces: tokio::sync::mpsc::Sender<std::io::Result<Bytes>>,
    held: Vec<u8>,
}

impl Sink {
    fn send_held(&mut self) -> std::io::Result<()> {
        if self.held.is_empty() {
            return Ok(());
        }
        let piece = Bytes::from(std::mem::take(&mut self.held));
        self.pieces
            .blocking_send(Ok(piece))
            .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }
}

impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.held.extend_from_slice(data);
        if self.held.len() >= ARCHIVE_PIECE_BYTES {
            self.send_held()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send_held()
    }
}

/// The content of a blob as a reader that a thread of its own can use.
struct BlobReader {
    handle: tokio::runtime::Handle,
    body: ByteStream,
    held: Bytes,
}

impl Read for BlobReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.held.is_empty() {
            match self.handle.block_on(self.body.next()) {
                Some(Ok(piece)) => self.held = piece,
                Some(Err(error)) => return Err(std::io::Error::other(error)),
                None => return Ok(0),
            }
        }
        let n = out.len().min(self.held.len());
        out[..n].copy_from_slice(&self.held[..n]);
        self.held = self.held.slice(n..);
        Ok(n)
    }
}

/// What goes into an archive of a snapshot.
struct Item {
    path: String,
    entry: Entry,
}

fn open_blob(
    handle: &tokio::runtime::Handle,
    store: &Arc<dyn BlobStore>,
    hash: &orch_types::Hash,
    authority: &snapshot_read::Read,
) -> std::io::Result<(BlobReader, i64)> {
    handle.block_on(authority.check()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the Snapshot read is no longer authorized",
        )
    })?;
    let read = handle
        .block_on(store.get(hash, None))
        .map_err(|error| match error {
            BlobError::NotFound(_) => std::io::Error::from(std::io::ErrorKind::NotFound),
            other => std::io::Error::other(other),
        })?;
    let size = read.meta.size;
    Ok((
        BlobReader {
            handle: handle.clone(),
            body: read.body,
            held: Bytes::new(),
        },
        size,
    ))
}

/// Writes the files as a tar compressed with Zstandard.
fn write_tar_zst(
    handle: &tokio::runtime::Handle,
    store: &Arc<dyn BlobStore>,
    items: &[Item],
    out: Sink,
    authority: &snapshot_read::Read,
) -> std::io::Result<()> {
    let encoder = zstd::stream::write::Encoder::new(out, ARCHIVE_COMPRESSION_LEVEL)?;
    let mut builder = tar::Builder::new(encoder);
    for item in items {
        let mut header = tar::Header::new_gnu();
        header.set_mode(item.entry.mode & 0o7777);
        header.set_mtime(u64::try_from(item.entry.mtime_ns / 1_000_000_000).unwrap_or(0));
        match item.entry.kind {
            EntryKind::Dir => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder.append_data(&mut header, format!("{}/", item.path), std::io::empty())?;
            }
            EntryKind::File => {
                // A file listed without a hash could not be taken: it is in no archive.
                let Some(hash) = item.entry.hash else {
                    continue;
                };
                let (reader, size) = open_blob(handle, store, &hash, authority)?;
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(u64::try_from(size).unwrap_or(0));
                builder.append_data(&mut header, &item.path, reader)?;
            }
            EntryKind::Link => {
                if let Some(target) = &item.entry.link {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    builder.append_link(&mut header, &item.path, target)?;
                }
            }
            EntryKind::Other => {}
        }
    }
    let encoder = builder.into_inner()?;
    let mut sink = encoder.finish()?;
    sink.flush()
}

/// Writes the files as a zip, which every desktop opens.
fn write_zip(
    handle: &tokio::runtime::Handle,
    store: &Arc<dyn BlobStore>,
    items: &[Item],
    out: Sink,
    authority: &snapshot_read::Read,
) -> std::io::Result<()> {
    let mut zip = zip::ZipWriter::new_stream(out);
    let plain = zip::write::SimpleFileOptions::default();
    for item in items {
        match item.entry.kind {
            EntryKind::Dir => {
                zip.add_directory(
                    format!("{}/", item.path),
                    plain.unix_permissions(DIRECTORY_MODE),
                )
                .map_err(std::io::Error::other)?;
            }
            EntryKind::File => {
                let Some(hash) = item.entry.hash else {
                    continue;
                };
                let (mut reader, size) = open_blob(handle, store, &hash, authority)?;
                let options = plain
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(item.entry.mode & 0o777)
                    .large_file(size > i64::from(u32::MAX));
                zip.start_file(&item.path, options)
                    .map_err(std::io::Error::other)?;
                std::io::copy(&mut reader, &mut zip)?;
            }
            EntryKind::Link | EntryKind::Other => {}
        }
    }
    let mut sink = zip.finish().map_err(std::io::Error::other)?;
    sink.flush()
}

/// Serves a snapshot as one stream: a tar compressed with Zstandard, or a zip.
async fn archive(ctx: Ctx, query: SnapshotArchiveGetQuery, _: Body) -> ApiResult<Response> {
    let (source, authority) = snapshot_read::open(&ctx, true).await?;
    let row = source.row;
    if row.storage == "environment" {
        let read = recorded_read(&ctx, &row, None, true, &authority).await?;
        if Some(read.blob) != row.manifest_blob {
            return Err(ApiError::new(
                ErrorCode::WorkspaceRestoreUnavailable,
                "the recorded export does not match the saved manifest",
            )
            .with("reason", "store_changed"));
        }
    }
    let tree = tree_of(&ctx, &row).await;
    authority.check().await?;
    let tree = tree?;
    let format = query.format.unwrap_or(SnapshotArchiveFormat::TarZst);
    // Everything goes below a directory named after the snapshot, so that unpacking one makes one.
    let top = crate::media::safe_name(&row.label);
    let mut items = vec![Item {
        path: top.clone(),
        entry: Entry {
            path: top.clone(),
            kind: EntryKind::Dir,
            size: 0,
            mtime_ns: row.created_at.timestamp_nanos_opt().unwrap_or(0),
            mode: DIRECTORY_MODE,
            hash: None,
            link: None,
        },
    }];
    items.extend(tree.entries().iter().map(|entry| Item {
        path: format!("{top}/{}", entry.path),
        entry: entry.clone(),
    }));
    let (pieces, received) = tokio::sync::mpsc::channel(ARCHIVE_QUEUE_PIECES);
    let (store, handle) = (ctx.app.blobs.store(), tokio::runtime::Handle::current());
    let sink = Sink {
        pieces: pieces.clone(),
        held: Vec::new(),
    };
    let writer_authority = authority.clone();
    tokio::task::spawn_blocking(move || {
        let written = match format {
            SnapshotArchiveFormat::TarZst => {
                write_tar_zst(&handle, &store, &items, sink, &writer_authority)
            }
            SnapshotArchiveFormat::Zip => {
                write_zip(&handle, &store, &items, sink, &writer_authority)
            }
        };
        if let Err(error) = written {
            // Say that the stream broke off, so that a reader does not take what it has for the whole.
            let _ = pieces.blocking_send(Err(error));
        }
    });
    let body = Body::from_stream(futures::stream::unfold(
        received,
        |mut received| async move { received.recv().await.map(|piece| (piece, received)) },
    ));
    let (extension, media) = match format {
        SnapshotArchiveFormat::TarZst => ("tar.zst", "application/zstd"),
        SnapshotArchiveFormat::Zip => ("zip", "application/zip"),
    };
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(media));
    if let Ok(value) = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}.{extension}\"",
        top.replace('"', "_")
    )) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    authority.response(response).await
}

#[path = "snapshots/creation.rs"]
pub mod creation;

#[path = "snapshots/deletion.rs"]
pub mod deletion;

#[path = "snapshots/environment_list.rs"]
pub mod environment_list;

#[path = "snapshots/preflight.rs"]
pub mod preflight;

#[path = "snapshots/read.rs"]
pub mod read;

#[path = "snapshots/subject.rs"]
pub mod subject;

#[path = "snapshots/update.rs"]
pub mod update;
