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
use crate::cache::{Entry, EntryKind, Manifest};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::resources::view::{addresses, parse};
use crate::sign_in::origin;
use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use futures::StreamExt;
use http::{HeaderValue, StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::environment::EnvironmentRef;
use orch_api_contract::workspace::{
    DirListing, EnvironmentFilesExport, Job, Snapshot, SnapshotArchiveFormat, SnapshotArchiveGet,
    SnapshotArchiveGetQuery, SnapshotDelete, SnapshotFileContentGet, SnapshotFileList, SnapshotGet,
    SnapshotPathQuery, SnapshotPreflight, SnapshotSelection, SnapshotSelectionRefusal,
    SnapshotState, SnapshotUpdate, SnapshotUpdateBody, ThreadSnapshotCreate,
    ThreadSnapshotCreateBody, ThreadSnapshotCreateSelection, ThreadSnapshotList,
    ThreadSnapshotPreflight, ThreadSnapshotPreflightBody,
};
use orch_authz::{Act, Policy, Principal};
use orch_blob_contract::store::{BlobError, BlobStore, ByteStream};
use orch_db::audit::{self, Actor, Entry as AuditEntry};
use orch_db::queries::environments::workspace as environment_rows;
use orch_db::queries::storage::{self, SnapshotRow};
use orch_engine::jobs::snapshot::Asked;
use orch_types::{Id, Time};
use std::collections::HashMap;
use std::io::{Read, Write};
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
    registry.on(EnvironmentFilesExport, export_environment);
    registry
        .on(ThreadSnapshotList, list)
        .on(ThreadSnapshotCreate, create)
        .on(ThreadSnapshotPreflight, preflight)
        .on(SnapshotGet, get)
        .on(SnapshotFileList, file_list)
        .on_raw(SnapshotFileContentGet, file_content)
        .on_raw(SnapshotArchiveGet, archive)
        .on(SnapshotUpdate, update)
        .on(SnapshotDelete, delete);
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

/// The snapshot `id`, and the caller holding `permission` on the environment the thread it was taken
/// through runs in now.
async fn own(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    permission: Permission,
) -> ApiResult<(SnapshotRow, Principal)> {
    let id = ctx.id("id")?;
    let row = storage::snapshot(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let principal = authorization::principal(ctx, conn).await?;
    if row.thread_id.is_none() {
        if row.created_by != Some(principal.user()) {
            return Err(ApiError::not_found());
        }
    } else if row.storage == "environment" {
        authorization::require_resource(
            conn,
            &principal,
            ResourceKind::Environment,
            row.environment_id,
            permission,
            Policy::default(),
        )
        .await?;
    } else {
        let thread = row.thread_id.ok_or_else(ApiError::not_found)?;
        require_where_it_runs(conn, &principal, thread, permission).await?;
    }
    Ok((row, principal))
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

/// The snapshots taken through a thread, newest first, for whoever may read the files of the
/// environment it runs in.
async fn list(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<Page<Snapshot>> {
    let thread = ctx.id("id")?;
    let limit = query.limit();
    let after = query.cursor.as_deref().map(after_of).transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let current = environment_rows::thread_place(&mut conn, thread)
        .await?
        .and_then(|place| place.environment_id);
    let mut permission = HashMap::new();
    let mut recorded = orch_db::queries::source::snapshot_environments(&mut conn, thread).await?;
    if let Some(environment) = current {
        recorded.push(environment);
    }
    recorded.sort();
    recorded.dedup();
    for environment in recorded {
        let object = orch_access_contract::decisions::ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: environment,
        };
        let allowed = match orch_authz::resources::check(
            &mut conn,
            &principal,
            object,
            Permission::FilesView,
            Policy::default(),
        )
        .await
        {
            Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => true,
            Ok(_) | Err(orch_authz::Error::NotFound(_) | orch_authz::Error::Suspended { .. }) => {
                false
            }
            Err(error) => return Err(ApiError::internal(error)),
        };
        permission.insert(environment, allowed);
    }
    let manual = current.is_some_and(|environment| permission.get(&environment) == Some(&true));
    let recorded: Vec<Id> = permission
        .into_iter()
        .filter_map(|(id, allowed)| allowed.then_some(id))
        .collect();
    if !manual && recorded.is_empty() {
        require_where_it_runs(&mut conn, &principal, thread, Permission::FilesView).await?;
    }
    let mut found = storage::visible_snapshots_of_thread(
        &mut conn,
        thread,
        after,
        i64::from(limit) + 1,
        manual,
        &recorded,
    )
    .await?;
    let more = found.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next_cursor = found.last().filter(|_| more).map(cursor_of);
    Ok(Page {
        items: views(&mut conn, found, principal.now()).await?,
        next_cursor,
    })
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

/// Resolves and authorizes exactly the paths both preflight and creation measure.
async fn prepare_snapshot(
    ctx: &Ctx,
    selection: Option<&ThreadSnapshotCreateSelection>,
) -> ApiResult<(
    environment_rows::EnvironmentRow,
    Id,
    orch_link_contract::commands::Selection,
)> {
    let thread = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let place = environment_rows::thread_place(&mut conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let environment = match place.environment_id {
        Some(id) => environment_rows::environment(&mut conn, id).await?,
        None => None,
    }
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::EnvironmentNoneUsable,
            "this thread runs in no environment",
        )
    })?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    authorization::require_resource(
        &mut conn,
        &principal,
        ResourceKind::Environment,
        environment.id,
        Permission::FilesManage,
        Policy::default(),
    )
    .await?;
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
            if !listing::indexed(&environment) {
                return Err(ApiError::new(
                    ErrorCode::WorkspaceNotIndexed,
                    "the tree is too large to index; a snapshot of it names the paths it takes",
                ));
            }
            (vec![place.workspace_dir.clone()], Vec::new())
        }
    };
    let machine = live::machine_of(ctx, &mut conn, &environment)
        .await?
        .ok_or_else(live::offline)?;
    drop(conn);
    // The machine takes the snapshot of an environment it has been told to attach.
    Ok((
        environment,
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

async fn measure_snapshot(
    ctx: &Ctx,
    environment: Id,
    machine: Id,
    selection: &orch_link_contract::commands::Selection,
) -> ApiResult<SnapshotPreflight> {
    let measured = live::run(
        ctx,
        machine,
        orch_link_contract::commands::SerialKey::Environment(environment),
        &orch_link_contract::commands::FsMeasure {
            environment_id: environment,
            selection: selection.clone(),
        },
    )
    .await
    .map_err(|failure| failure.problem(super::problems::Doing::Reading))?;
    measurement_view(selection, measured)
}

/// Measures without writing a snapshot, audit row or job.
async fn preflight(
    ctx: Ctx,
    (): (),
    body: ThreadSnapshotPreflightBody,
) -> ApiResult<SnapshotPreflight> {
    let (environment, machine, selection) = prepare_snapshot(&ctx, body.selection.as_ref()).await?;
    measure_snapshot(&ctx, environment.id, machine, &selection).await
}

/// Rechecks the selection before the transaction that starts the snapshot job.
async fn create(ctx: Ctx, (): (), body: ThreadSnapshotCreateBody) -> ApiResult<Job> {
    let thread = ctx.id("id")?;
    let label = label_of(&ctx, body.label.as_deref())?;
    let (environment, machine, selection) = prepare_snapshot(&ctx, body.selection.as_ref()).await?;
    let measured = measure_snapshot(&ctx, environment.id, machine, &selection).await?;
    if !measured.allowed {
        return Err(ApiError::new(
            ErrorCode::WorkspaceNotIndexed,
            measured
                .reason
                .as_deref()
                .unwrap_or("the selection could not be measured"),
        )
        .with("preflight", measured));
    }
    let orch_link_contract::commands::Selection { include, exclude } = selection;
    let by = ctx.user()?.id();
    let mut tx = ctx.app.db.begin().await?;
    let begun = ctx
        .app
        .engine
        .begin_snapshot(
            &mut tx,
            &Asked {
                thread: Some(thread),
                environment: environment.id,
                generation: environment.generation,
                label: label.clone(),
                include: include.clone(),
                exclude,
                created_by: Some(by),
            },
        )
        .await
        .map_err(ApiError::internal)?;
    let summary = format!(
        "{} began a snapshot of {}",
        crate::routes::nodes::actor_name(&ctx)?,
        include.join(", ")
    );
    let mut entry = AuditEntry::new(
        Actor::User(by),
        orch_db::audit::action!(AuditAction::WorkspaceSnapshotCreated).as_str(),
        "snapshot",
        Some(begun.snapshot),
        &summary,
    );
    entry.history = AuditAction::WorkspaceSnapshotCreated.is_history_line();
    entry.node_id = Some(thread);
    entry.org_id = Some(environment.org_id);
    entry.environment_id = Some(environment.id);
    entry.details =
        serde_json::json!({ "snapshot": begun.snapshot, "label": label, "include": include });
    audit::append(&mut tx, &entry, &origin(&ctx), ctx.now).await?;
    let job = crate::routes::environments::jobs::view(&mut tx, &begun.job).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(job)
}

/// One snapshot.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Snapshot> {
    let mut conn = ctx.app.db.acquire().await?;
    let (row, _) = own(&ctx, &mut conn, Permission::FilesView).await?;
    if row.kind == "turn"
        && row.state == "unavailable"
        && row.unavailable_reason.as_deref() == Some("store_unavailable")
    {
        drop(conn);
        if let Err(error) = ctx.app.engine.recover_recording(row.id).await {
            tracing::warn!(%error, snapshot=%row.id, "the retained recording could not be revalidated");
        }
        conn = ctx.app.db.acquire().await?;
        // Recheck the reader after external content work, even if recovery found no original.
        let (current, _) = own(&ctx, &mut conn, Permission::FilesView).await?;
        return view(&mut conn, current, Time::new(ctx.app.clock.now())).await;
    }
    view(&mut conn, row, Time::new(ctx.app.clock.now())).await
}

/// Renames a snapshot.
async fn update(ctx: Ctx, (): (), body: SnapshotUpdateBody) -> ApiResult<Snapshot> {
    let label = label_of(&ctx, Some(&body.label)).and_then(|label| {
        if body.label.trim().is_empty() {
            Err(ApiError::invalid("label", "a snapshot is called something"))
        } else {
            Ok(label)
        }
    })?;
    let mut conn = ctx.app.db.acquire().await?;
    let (row, _) = own(&ctx, &mut conn, Permission::FilesManage).await?;
    storage::relabel_snapshot(&mut conn, row.id, &label).await?;
    let renamed = storage::snapshot(&mut conn, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    view(&mut conn, renamed, Time::new(ctx.app.clock.now())).await
}

/// Deletes a snapshot, and lets go of the files it held.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    let (row, _) = own(&ctx, &mut conn, Permission::FilesManage).await?;
    if row.state == storage::snapshot_state::RUNNING {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "the snapshot is being taken; cancel its job first",
        ));
    }
    // What the snapshot pinned is read from its list before the list goes.
    let listed: Vec<orch_types::Hash> = match row.manifest_blob.filter(|_| row.storage == "blobs") {
        Some(manifest) => ctx
            .app
            .cache
            .manifest_of(&ctx.app.blobs, row.id, &manifest)
            .await
            .map(|tree| {
                tree.hashes()
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default(),
        None => Vec::new(),
    };
    drop(conn);
    let mut tx = ctx.app.db.begin().await?;
    let tracked = storage::snapshot_pins(&mut tx, row.id).await?;
    let removed = if row.storage == "environment" {
        orch_db::queries::source::expire_recorded(&mut tx, row.id, ctx.now).await?
    } else {
        storage::delete_snapshot(&mut tx, row.id).await?
    };
    let Some(manifest) = removed else {
        return Err(ApiError::not_found());
    };
    let mut released = tracked;
    if released.is_empty() {
        released = listed;
        released.extend(manifest);
    }
    ctx.app.blobs.unpin(&mut tx, &released, ctx.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

// ---------------------------------------------------------------- what a snapshot holds

async fn recorded_read(
    ctx: &Ctx,
    row: &SnapshotRow,
    path: Option<String>,
    export: bool,
) -> ApiResult<orch_link_contract::source::FsRecordReadResult> {
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
    let mut conn = ctx.app.db.acquire().await?;
    let environment = environment_rows::environment(&mut conn, row.environment_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let machine = live::require_live(ctx, &mut conn, &environment).await?;
    drop(conn);
    let result = live::run(
        ctx,
        machine,
        orch_link_contract::commands::SerialKey::None,
        &orch_link_contract::source::FsRecordRead {
            environment_id: row.environment_id,
            snapshot_id: row.id,
            path,
            export,
        },
    )
    .await
    .map_err(|_| {
        ApiError::new(
            ErrorCode::WorkspaceRestoreUnavailable,
            "the recorded state could not be read completely",
        )
        .with("snapshot_id", row.id)
    })?;
    let mut conn = ctx.app.db.acquire().await?;
    // A slow transfer cannot return file data under the permission from before a move or revoke.
    let _ = own(ctx, &mut conn, Permission::FilesView).await?;
    storage::register_blob(&mut conn, &result.blob, result.size, ctx.app.clock.now()).await?;
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
    let mut conn = ctx.app.db.acquire().await?;
    let (row, _) = own(&ctx, &mut conn, Permission::FilesView).await?;
    let path = paths::clean("path", &query.path, true)?;
    drop(conn);
    if row.storage == "environment" {
        let checked = recorded_read(&ctx, &row, None, false).await?;
        if Some(checked.blob) != row.manifest_blob {
            return Err(ApiError::new(
                ErrorCode::WorkspaceRestoreUnavailable,
                "the recorded manifest changed",
            )
            .with("reason", "store_changed"));
        }
    }
    let tree = tree_of(&ctx, &row).await?;
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
    let mut conn = ctx.app.db.acquire().await?;
    let (row, _) = own(&ctx, &mut conn, Permission::FilesView).await?;
    drop(conn);
    let path = paths::clean("path", &query.path, false)?;
    let tree = tree_of(&ctx, &row).await?;
    let hash = tree
        .get(&path)
        .filter(|entry| entry.kind == EntryKind::File)
        .and_then(|entry| entry.hash)
        .ok_or_else(ApiError::not_found)?;
    if row.storage == "environment" {
        let read = recorded_read(&ctx, &row, Some(path.clone()), false).await?;
        if read.blob != hash {
            return Err(ApiError::new(
                ErrorCode::WorkspaceRestoreUnavailable,
                "the recorded file does not match the saved manifest",
            )
            .with("reason", "store_changed"));
        }
    }
    let name = paths::name_of(&path);
    files::read_for(
        &ctx,
        &hash,
        name,
        crate::media::media_type(name, &[]),
        false,
    )
    .await
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
) -> std::io::Result<(BlobReader, i64)> {
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
                let (reader, size) = open_blob(handle, store, &hash)?;
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
                let (mut reader, size) = open_blob(handle, store, &hash)?;
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
    let mut conn = ctx.app.db.acquire().await?;
    let (row, _) = own(&ctx, &mut conn, Permission::FilesView).await?;
    drop(conn);
    if row.storage == "environment" {
        let read = recorded_read(&ctx, &row, None, true).await?;
        if Some(read.blob) != row.manifest_blob {
            return Err(ApiError::new(
                ErrorCode::WorkspaceRestoreUnavailable,
                "the recorded export does not match the saved manifest",
            )
            .with("reason", "store_changed"));
        }
    }
    let tree = tree_of(&ctx, &row).await?;
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
    tokio::task::spawn_blocking(move || {
        let written = match format {
            SnapshotArchiveFormat::TarZst => write_tar_zst(&handle, &store, &items, sink),
            SnapshotArchiveFormat::Zip => write_zip(&handle, &store, &items, sink),
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
    Ok(response)
}

// ---------------------------------------------------------------- jobs

/// Whether the caller holds something on the environment: any permission at all.
async fn holds_on_environment(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    environment: Id,
) -> ApiResult<bool> {
    let object = orch_access_contract::decisions::ObjectRef {
        kind: ResourceKind::Environment.into(),
        id: environment,
    };
    let effective = orch_authz::resources::effective(conn, principal, object, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    Ok(effective.owner || effective.administers || !effective.held.is_empty())
}

/// Captures a whole workspace without creating a hidden thread or holding a transaction for file work.
async fn export_environment(ctx: Ctx, (): (), (): ()) -> ApiResult<Job> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    authorization::require_resource(
        &mut tx,
        &principal,
        ResourceKind::Environment,
        id,
        Permission::EnvironmentManage,
        Policy::default(),
    )
    .await?;
    if !orch_db::queries::environments::temporary::try_lock(&mut tx, id).await? {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "The environment is changing or being removed. Try Export again when it finishes.",
        ));
    }
    let environment = orch_db::queries::environments::hosting::environment(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let begun = ctx
        .app
        .engine
        .begin_snapshot(
            &mut tx,
            &Asked {
                thread: None,
                environment: id,
                generation: environment.generation,
                label: environment.name.clone(),
                include: vec!["**".to_owned()],
                exclude: Vec::new(),
                created_by: Some(principal.user()),
            },
        )
        .await
        .map_err(ApiError::internal)?;
    orch_engine::temporary_environments::export_activity(&mut tx, id, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    let action = orch_db::audit::action!(AuditAction::WorkspaceSnapshotCreated);
    let summary = format!("Workspace export of {} was requested", environment.name);
    let mut entry = AuditEntry::new(
        Actor::User(principal.user()),
        action.as_str(),
        "snapshot",
        Some(begun.snapshot),
        &summary,
    );
    entry.environment_id = Some(id);
    entry.node_id = Some(environment.home_node_id);
    entry.org_id = Some(environment.org_id);
    entry.details =
        serde_json::json!({"snapshot":begun.snapshot,"environment":id,"whole_workspace":true});
    audit::append(&mut tx, &entry, &origin(&ctx), ctx.now).await?;
    let job = crate::routes::environments::jobs::view(&mut tx, &begun.job).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.engine.kick();
    Ok(job)
}

/// Whether the caller may read a snapshot job or a delivery job: whoever may view the thread it is
/// about, holds anything on its environment, or manages its provider. The requests for jobs are bound
/// once, with the environments; they ask the workspace about the jobs that are its own.
pub(crate) async fn may_read_job(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    row: &orch_db::queries::jobs::JobRow,
) -> ApiResult<()> {
    if row
        .plan
        .get("workspace_export")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        && row.created_by == Some(principal.user())
    {
        return Ok(());
    }
    let mut refusal: Option<ApiError> = None;
    if let Some(thread) = row.node_id {
        match authorization::require(
            conn,
            principal,
            thread,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => refusal = Some(error),
        }
    }
    if let Some(environment) = row.environment_id
        && holds_on_environment(conn, principal, environment).await?
    {
        return Ok(());
    }
    if let Some(provider) = row.provider_id
        && authorization::require_resource(
            conn,
            principal,
            ResourceKind::Provider,
            provider,
            Permission::ProviderManage,
            Policy::default(),
        )
        .await
        .is_ok()
    {
        return Ok(());
    }
    Err(refusal.unwrap_or_else(ApiError::not_found))
}

/// Whether the caller could have started a snapshot job or a delivery job, and so may ask for it to
/// be cancelled: whoever writes the files of the environment the thread it is about runs in.
pub(crate) async fn may_cancel_job(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    row: &orch_db::queries::jobs::JobRow,
) -> ApiResult<()> {
    if row
        .plan
        .get("workspace_export")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        && row.created_by == Some(principal.user())
    {
        return Ok(());
    }
    let Some(thread) = row.node_id else {
        return Err(ApiError::not_found());
    };
    require_where_it_runs(conn, principal, thread, Permission::FilesManage).await
}
