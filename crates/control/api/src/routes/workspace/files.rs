//! The files of an environment's workspace: listed live or from the last list of the tree, described, brought to the store and read from it in ranges, written on the version the writer saw, made, renamed, copied and deleted.
//!
//! **Reading is two steps.** A fetch says whether that exact version of a file (its size and its time
//! of last change) is in the store, and otherwise asks the machine to upload it and says that it is
//! on its way, or why it cannot be had; then the content is read by its hash, which has to belong to
//! that path in this environment. A machine that is away is answered from what was kept: the list of
//! its tree, and the content of the files that were read before.
//!
//! **One writer at a time, by version.** A write names the hash of the file the writer saw, or says
//! that the file must not exist yet, and writes only if the file is still that; if it is another, the
//! write is refused with the file as it stands and nothing is written. Writes need the environment's
//! own switch and the machine to be there. Each write, rename, copy and deletion is one audit row that
//! names the path and never the content.

use super::door::{self, Door};
use super::listing;
use super::live;
use super::pages;
use super::paths;
use super::problems::{Doing, Failure};
use crate::dispatch::CommandOutcome;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::workspace::json_reply;
use crate::sign_in::origin;
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use chrono::Duration as ChronoDuration;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::common::{ErrorCode, MAX_BODY_BYTES};
use orch_api_contract::workspace::{
    DirEntryKind, DirListing, EXPECTED_HASH_NONE, EnvironmentFileContentGet,
    EnvironmentFileContentGetQuery, EnvironmentFileContentPut, EnvironmentFileContentPutQuery,
    EnvironmentFileCopy, EnvironmentFileDelete, EnvironmentFileDeleteQuery, EnvironmentFileFetch,
    EnvironmentFileFetchQuery, EnvironmentFileList, EnvironmentFileListQuery, EnvironmentFileMkdir,
    EnvironmentFilePreviewGet, EnvironmentFileRename, EnvironmentFileStat,
    EnvironmentFileStatQuery, FETCH_WAIT_MAX_SECONDS, FetchState, FetchStatus, FetchUnavailable,
    FileFromToBody, FileListSource, FilePathBody, FileStat,
};
use orch_api_contract::workspace::{
    EnvironmentUploadFinish, EnvironmentUploadFinishBody, EnvironmentUploadWrite,
    EnvironmentUploadWriteBody, EnvironmentUploadWriteResult, WorkspaceUploadOutcome,
};
use orch_blob_contract::range::{BlobMeta, ByteRange, unsatisfied_content_range};
use orch_blob_contract::store::{BlobError, BlobRead};
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::storage::{self, CachedFile, NANOS_PER_MILLI};
use orch_link_contract::commands::{
    CommandKind, ExpectedHash, FsCopy, FsFetch, FsFetchResult, FsList, FsMkdir, FsRemove, FsRename,
    FsStat, FsWrite, SerialKey,
};
use orch_types::{Hash, Id, Json, Time};
use std::time::Duration;

/// How often a fetch is asked again, in a single request, after the machine said that the file
/// changed while it was read, or the command ran out: the new version is what the next try reads.
const FETCH_ATTEMPTS: u32 = 3;
/// Two names fit a compact history line; the complete list stays in its structured record.
const HISTORY_UPLOAD_NAMES: usize = 2;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(EnvironmentFileList, list)
        .on(EnvironmentFileStat, stat)
        .on_raw(EnvironmentFilePreviewGet, preview_get)
        .on(EnvironmentFileFetch, fetch)
        .on_raw(EnvironmentFileContentGet, content_get)
        .on_raw(EnvironmentFileContentPut, content_put)
        .on(EnvironmentUploadWrite, upload_write)
        .on(EnvironmentUploadWriteResult, upload_write_result)
        .on(EnvironmentUploadFinish, upload_finish)
        .on(EnvironmentFileMkdir, mkdir)
        .on(EnvironmentFileRename, rename)
        .on(EnvironmentFileCopy, copy)
        .on(EnvironmentFileDelete, delete);
}

// ---------------------------------------------------------------- listing

/// A directory, live when its machine is there and from the last list of the tree otherwise.
async fn list(ctx: Ctx, query: EnvironmentFileListQuery, (): ()) -> ApiResult<DirListing> {
    let limit = query
        .limit
        .unwrap_or(orch_link_contract::commands::FS_LIST_MAX_ENTRIES)
        .min(orch_link_contract::commands::FS_LIST_MAX_ENTRIES);
    if limit == 0 {
        return Err(ApiError::invalid("limit", "must be at least 1"));
    }
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = paths::clean("path", &query.path, true)?;
    let mount = paths::mount("mount", query.mount.as_deref())?;
    let machine = live::machine_of(&ctx, &mut conn, &door.env).await?;
    let asked = query.source.unwrap_or(FileListSource::Auto);
    let source = pages::resolve(&door.env, machine, asked, mount.is_some()).map_err(|error| {
        if query.cursor.is_some() {
            pages::stale()
        } else {
            error
        }
    })?;
    let binding = pages::Binding {
        environment: door.env.id,
        generation: door.env.generation,
        machine: if source == pages::Source::Live {
            machine
        } else {
            None
        },
        path: path.clone(),
        mount: mount.clone(),
        source: source.clone(),
    };
    let cursor = binding.cursor(query.cursor.as_deref(), ctx.app.clock.time())?;
    // Native reads and blob loads hold neither a transaction nor this pooled connection.
    drop(conn);
    let (entries, truncated, metadata) = match source {
        pages::Source::Live => {
            let native_cursor = cursor
                .as_ref()
                .map(|cursor| match &cursor.position {
                    pages::Position::Native { cursor } => Ok(cursor.clone()),
                    _ => Err(pages::stale()),
                })
                .transpose()?;
            let mut listed = live::run(
                &ctx,
                machine.ok_or_else(live::offline)?,
                SerialKey::None,
                &FsList {
                    environment_id: door.env.id,
                    mount: mount.clone(),
                    path: path.clone(),
                    watch_id: None,
                    watch_instance: None,
                    page: Some(orch_link_contract::commands::FsListPage {
                        cursor: native_cursor,
                        limit: Some(limit),
                        generation: Some(i64::from(door.env.generation)),
                    }),
                },
            )
            .await
            .map_err(|failure| {
                if cursor.is_some()
                    && matches!(
                        failure.code(),
                        Some("conflict" | "changed" | "not_found" | "ignored")
                    )
                {
                    pages::stale()
                } else {
                    failure.problem(Doing::Reading)
                }
            })?;
            if listed.entries.len() > orch_link_contract::commands::FS_LIST_MAX_ENTRIES as usize
                || serde_json::to_vec(&listed)
                    .map_err(ApiError::internal)?
                    .len()
                    > 2 * 1024 * 1024
            {
                return Err(ApiError::new(
                    ErrorCode::RequestTooLarge,
                    "The connector's directory page exceeds its row or byte bound.",
                ));
            }
            if listed.page.is_some()
                && (listed.entries.len() > limit as usize
                    || listed.truncated
                        != listed
                            .page
                            .as_ref()
                            .is_some_and(|page| page.next_cursor.is_some()))
            {
                return Err(ApiError::new(
                    ErrorCode::EnvironmentUnavailable,
                    "The connector answered an inconsistent directory page.",
                ));
            }
            // An old peer can give a partial first window but cannot invent a continuation.
            if listed.page.is_none() && listed.entries.len() > limit as usize {
                listed.entries.truncate(limit as usize);
                listed.truncated = true;
            }
            let metadata = match listed.page {
                Some(page) => Some(binding.native(page, ctx.app.clock.time())?),
                None if cursor.is_some() => {
                    return Err(ApiError::new(
                        ErrorCode::CapabilityUnsupported,
                        "This connector does not support directory continuation.",
                    ));
                }
                None => None,
            };
            if let (Some(cursor), Some(metadata)) = (&cursor, &metadata)
                && (cursor.revision != metadata.revision
                    || cursor.as_of != metadata.as_of
                    || cursor.observed_from != metadata.observed_from)
            {
                return Err(pages::stale());
            }
            (listed.entries, listed.truncated, metadata)
        }
        pages::Source::Manifest { blob } => {
            let tree = ctx
                .app
                .cache
                .manifest_of(&ctx.app.blobs, door.env.id, &blob)
                .await
                .map_err(ApiError::internal)?;
            let revision = format!("manifest:{blob}");
            if cursor
                .as_ref()
                .is_some_and(|cursor| cursor.revision != revision)
            {
                return Err(pages::stale());
            }
            let mut window = pages::Window::new(cursor.as_ref(), limit)?;
            pages::manifest(&tree, &path, &mut window)?;
            let (entries, next) = window.finish();
            let as_of = Time::new(door.env.manifest_at.ok_or_else(pages::stale)?);
            let metadata = binding.metadata(revision, next, as_of, ctx.app.clock.time())?;
            (entries, metadata.next_cursor.is_some(), Some(metadata))
        }
        pages::Source::Opened => {
            let (entries, metadata) = opened_page(&ctx, &binding, cursor.as_ref(), limit).await?;
            (entries, metadata.next_cursor.is_some(), Some(metadata))
        }
    };
    let mut reply = DirListing {
        path,
        mount,
        source: if binding.source == pages::Source::Live {
            orch_api_contract::forks::FilesSource::Live
        } else {
            orch_api_contract::forks::FilesSource::Cached
        },
        as_of: metadata
            .as_ref()
            .map_or(Time::new(ctx.now), |metadata| metadata.as_of),
        entries,
        truncated,
        observed_from: metadata
            .as_ref()
            .and_then(|metadata| metadata.observed_from),
        revision: metadata.as_ref().map(|metadata| metadata.revision.clone()),
        next_cursor: metadata
            .as_ref()
            .and_then(|metadata| metadata.next_cursor.clone()),
        expires_at: metadata.as_ref().map(|metadata| metadata.expires_at),
    };
    // The existing guard rechecks the exact bearer, current time/authority and files.view.
    // No machine/blob/timer/second connection is awaited inside this final read transaction.
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let fresh =
        orch_db::queries::environments::workspace::environment(&mut tx, binding.environment)
            .await?
            .ok_or_else(ApiError::not_found)?;
    if let Some(thread) = query.thread {
        door::beside(&mut tx, guard.principal(), fresh.id, thread).await?;
    }
    let current_machine = live::machine_of(&ctx, &mut tx, &fresh).await?;
    let current_source = pages::resolve(&fresh, current_machine, asked, reply.mount.is_some())
        .map_err(|error| {
            if query.cursor.is_some() {
                pages::stale()
            } else {
                error
            }
        })?;
    let current = pages::Binding {
        environment: fresh.id,
        generation: fresh.generation,
        machine: if current_source == pages::Source::Live {
            current_machine
        } else {
            None
        },
        path: reply.path.clone(),
        mount: reply.mount.clone(),
        source: current_source,
    };
    if !binding.agrees(&current)? {
        return Err(pages::stale());
    }
    listing::mark_cached(
        &mut tx,
        fresh.id,
        reply.mount.as_deref(),
        &mut reply.entries,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if serde_json::to_vec(&reply)
        .map_err(ApiError::internal)?
        .len()
        > 2 * 1024 * 1024
    {
        return Err(ApiError::new(
            ErrorCode::RequestTooLarge,
            "The directory page and its path exceed the page size. Request fewer rows.",
        ));
    }
    Ok(reply)
}

/// Offline live-only mode lists only retained copies/ancestors, with bounded metadata reads.
async fn opened_page(
    ctx: &Ctx,
    binding: &pages::Binding,
    cursor: Option<&pages::Cursor>,
    limit: u32,
) -> ApiResult<(Vec<orch_api_contract::workspace::DirEntry>, pages::Metadata)> {
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let mut after: Option<String> = None;
    let mut digest = Hash::hasher();
    let mut window = pages::Window::new(cursor, limit)?;
    let prefix = if binding.path.is_empty() {
        String::new()
    } else {
        format!("{}/", binding.path)
    };
    let mut previous: Option<(String, bool)> = None;
    let mut as_of = None;
    loop {
        let rows = storage::cached_latest_page(
            &mut tx,
            binding.environment,
            &binding.path,
            after.as_deref(),
            500,
        )
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            as_of = Some(
                as_of.map_or(row.cached_at, |at: chrono::DateTime<chrono::Utc>| {
                    at.max(row.cached_at)
                }),
            );
            let bytes =
                serde_json::to_vec(&(&row.path, row.size, row.mtime_ns, row.hash, row.cached_at))
                    .map_err(ApiError::internal)?;
            digest.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(&bytes);
            let rest = row.path.strip_prefix(&prefix).ok_or_else(pages::stale)?;
            let (name, directory) = rest
                .split_once('/')
                .map_or((rest, false), |(name, _)| (name, true));
            if name.is_empty()
                || previous
                    .as_ref()
                    .is_some_and(|(last, kind)| last == name && *kind == directory)
            {
                continue;
            }
            previous = Some((name.to_owned(), directory));
            window.push(orch_api_contract::workspace::DirEntry {
                name: name.to_owned(),
                path: format!("{prefix}{name}"),
                kind: if directory {
                    DirEntryKind::Dir
                } else {
                    DirEntryKind::File
                },
                size: if directory { 0 } else { row.size },
                mtime: Time::new(chrono::DateTime::from_timestamp_nanos(row.mtime_ns)),
                mode: if directory { 0o755 } else { 0o644 },
                link_target: None,
                cached: !directory,
            })?;
        }
        after = rows.last().map(|row| row.path.clone());
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let revision = format!("opened:{}", Hash::finish(&digest));
    if cursor.is_some_and(|cursor| cursor.revision != revision) {
        return Err(pages::stale());
    }
    let (entries, next) = window.finish();
    let now = ctx.app.clock.time();
    let metadata = binding.metadata(revision, next, as_of.map_or(now, Time::new), now)?;
    Ok((entries, metadata))
}

// ---------------------------------------------------------------- describing

/// One file or directory.
async fn stat(ctx: Ctx, query: EnvironmentFileStatQuery, (): ()) -> ApiResult<FileStat> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = paths::clean("path", &query.path, true)?;
    let mount = paths::mount("mount", query.mount.as_deref())?;
    let machine = live::machine_of(&ctx, &mut conn, &door.env).await?;
    if let Some(machine) = machine {
        let mut found = live_stat(&ctx, &door, machine, &path, mount.as_deref())
            .await
            .map_err(|failure| failure.problem(Doing::Reading))?;
        listing::mark_cached(
            &mut conn,
            door.env.id,
            mount.as_deref(),
            std::slice::from_mut(&mut found.entry),
        )
        .await?;
        return Ok(found);
    }
    if mount.is_some() {
        return Err(live::offline());
    }
    offline_stat(&ctx, &door, &path).await
}

async fn live_stat(
    ctx: &Ctx,
    door: &Door,
    machine: Id,
    path: &str,
    mount: Option<&str>,
) -> Result<FileStat, Failure> {
    let found = live::run(
        ctx,
        machine,
        SerialKey::None,
        &FsStat {
            environment_id: door.env.id,
            mount: mount.map(str::to_owned),
            path: path.to_owned(),
        },
    )
    .await?;
    Ok(found)
}

/// A visible thumbnail asks once; a large original is never brought just to draw a list or card.
async fn preview_get(ctx: Ctx, query: EnvironmentFileStatQuery, _: Body) -> ApiResult<Response> {
    let found = stat(ctx.clone(), query.clone(), ()).await?;
    if found.entry.kind != DirEntryKind::File {
        return Err(ApiError::not_found());
    }
    let limit = crate::previews::source_limit(&ctx).await?;
    if found.entry.size > limit {
        return crate::previews::unavailable(
            &ctx,
            found.entry.size,
            &found.media_type,
            found.is_text,
            true,
        )
        .await;
    }
    let path = paths::clean("path", &query.path, false)?;
    let mount = paths::mount("mount", query.mount.as_deref())?;
    let key = paths::cache_key(mount.as_deref(), &path);
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let cached = storage::cached_version(
        &mut conn,
        door.env.id,
        &key,
        found.entry.size,
        found.entry.mtime.get().timestamp_millis(),
    )
    .await?;
    let machine = live::machine_of(&ctx, &mut conn, &door.env).await?;
    drop(conn);
    let hash = if let Some(hash) = cached.or(found.hash)
        && ctx
            .app
            .blobs
            .store()
            .exists(&hash)
            .await
            .map_err(ApiError::internal)?
    {
        hash
    } else if let Some(machine) = machine {
        let fetched: FsFetchResult = live::run(
            &ctx,
            machine,
            SerialKey::None,
            &FsFetch {
                environment_id: door.env.id,
                mount,
                path: path.clone(),
                expect: None,
                max_bytes: Some(limit),
            },
        )
        .await
        .map_err(|failure| failure.problem(Doing::Reading))?;
        if fetched.size != found.entry.size
            || fetched.mtime_ns.div_euclid(NANOS_PER_MILLI)
                != found.entry.mtime.get().timestamp_millis()
            || found.hash.is_some_and(|hash| hash != fetched.hash)
        {
            return crate::previews::unavailable(
                &ctx,
                found.entry.size,
                &found.media_type,
                found.is_text,
                false,
            )
            .await;
        }
        let mut conn = ctx.app.db.acquire().await?;
        ctx.app
            .blobs
            .register(
                &mut conn,
                BlobMeta {
                    hash: fetched.hash,
                    size: fetched.size,
                },
                ctx.now,
            )
            .await
            .map_err(ApiError::internal)?;
        storage::record_cached(
            &mut conn,
            door.env.id,
            &CachedFile {
                path: key,
                size: fetched.size,
                mtime_ns: fetched.mtime_ns,
                hash: fetched.hash,
                cached_at: ctx.now,
            },
        )
        .await?;
        fetched.hash
    } else {
        return crate::previews::unavailable(
            &ctx,
            found.entry.size,
            &found.media_type,
            found.is_text,
            false,
        )
        .await;
    };
    crate::previews::serve(&ctx, &hash, &found.entry.name, &found.media_type).await
}

/// What the last list of the tree says of a path.
async fn offline_stat(ctx: &Ctx, door: &Door, path: &str) -> ApiResult<FileStat> {
    let env = &door.env;
    if !listing::indexed(env) {
        return Err(live::offline());
    }
    let Some(manifest) = env.manifest_blob else {
        return Err(live::offline());
    };
    let tree = ctx
        .app
        .cache
        .manifest_of(&ctx.app.blobs, env.id, &manifest)
        .await
        .map_err(ApiError::internal)?;
    if path.is_empty() {
        let root = orch_api_contract::workspace::DirEntry {
            name: String::new(),
            path: String::new(),
            kind: DirEntryKind::Dir,
            size: 0,
            mtime: Time::new(env.manifest_at.unwrap_or(ctx.now)),
            mode: 0o755,
            link_target: None,
            cached: false,
        };
        return Ok(listing::file_stat(root, None));
    }
    match tree.get(path) {
        Some(entry) => Ok(listing::file_stat(listing::dir_entry(entry), entry.hash)),
        None if tree.has_below(path) => {
            let made = listing::from_manifest(&tree, paths::parent_of(path))
                .and_then(|siblings| siblings.into_iter().find(|each| each.path == path))
                .ok_or_else(ApiError::not_found)?;
            Ok(listing::file_stat(made, None))
        }
        None => Err(ApiError::not_found()),
    }
}

// ---------------------------------------------------------------- bringing a file to the store

fn unavailable(reason: FetchUnavailable) -> FetchState {
    FetchState {
        state: FetchStatus::Unavailable,
        hash: None,
        size: None,
        received: None,
        reason: Some(reason),
    }
}

fn ready(hash: Hash, size: i64) -> FetchState {
    FetchState {
        state: FetchStatus::Ready,
        hash: Some(hash),
        size: Some(size),
        received: None,
        reason: None,
    }
}

/// What the machine, or the list of the tree, says a path is now.
enum Seen {
    /// A file of this version, whose content the list knew the hash of.
    File {
        size: i64,
        mtime_ms: i64,
        hash: Option<Hash>,
    },
    /// Something that is no file.
    NotAFile,
    /// Nothing, or nothing that may be read.
    Missing(FetchUnavailable),
}

/// The identifier of the command that brings one version of a file to the store: the same for the
/// same version, so that asking again waits for the command that is under way, and another for each
/// try of a version that failed.
fn fetch_id(environment: Id, key: &str, size: i64, mtime_ms: i64, attempt: u32) -> Id {
    let mut hasher = Hash::hasher();
    hasher.update(b"orch-api-command\0fs.fetch\0");
    hasher.update(environment.as_bytes());
    hasher.update(&(key.len() as u64).to_le_bytes());
    hasher.update(key.as_bytes());
    hasher.update(&size.to_le_bytes());
    hasher.update(&mtime_ms.to_le_bytes());
    hasher.update(&attempt.to_le_bytes());
    let digest = Hash::finish(&hasher);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    // Version 8 of the identifier standard, which is for ones made by hashing.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

/// Asks for one version of a file to be in the store, and says where that stands.
async fn fetch(
    ctx: Ctx,
    query: EnvironmentFileFetchQuery,
    body: FilePathBody,
) -> ApiResult<FetchState> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = paths::clean("path", &body.path, false)?;
    let mount = paths::mount("mount", query.mount.as_deref())?;
    let wait = Duration::from_secs(u64::from(
        query.wait.unwrap_or(0).min(FETCH_WAIT_MAX_SECONDS),
    ));
    let env = door.env.clone();
    let machine = live::machine_of(&ctx, &mut conn, &env).await?;
    let key = paths::cache_key(mount.as_deref(), &path);

    // 1. The version there is now: the machine's word, or the last list's.
    let seen = match machine {
        Some(machine) => match live_stat(&ctx, &door, machine, &path, mount.as_deref()).await {
            Ok(stat) if stat.entry.kind == DirEntryKind::File => Seen::File {
                size: stat.entry.size,
                mtime_ms: stat.entry.mtime.get().timestamp_millis(),
                hash: stat.hash,
            },
            Ok(_) => Seen::NotAFile,
            Err(failure) => match failure.code() {
                Some("ignored") => Seen::Missing(FetchUnavailable::Ignored),
                Some("not_found") => Seen::Missing(FetchUnavailable::NotFound),
                _ => return Err(failure.problem(Doing::Reading)),
            },
        },
        None if mount.is_some() => return Err(live::offline()),
        None => kept_version(&ctx, &env, &path).await?,
    };
    let (size, mtime_ms, listed_hash) = match seen {
        Seen::File {
            size,
            mtime_ms,
            hash,
        } => (size, mtime_ms, hash),
        Seen::NotAFile | Seen::Missing(FetchUnavailable::NotFound) => {
            return Ok(unavailable(FetchUnavailable::NotFound));
        }
        Seen::Missing(reason) => return Ok(unavailable(reason)),
    };
    let store = ctx.app.blobs.store();

    // 2. The content of that very version is in the store.
    if let Some(hash) = storage::cached_version(&mut conn, env.id, &key, size, mtime_ms).await?
        && store.exists(&hash).await.map_err(ApiError::internal)?
    {
        storage::touch_blob(&mut conn, &hash, ctx.now).await?;
        return Ok(ready(hash, size));
    }
    // The list of the tree knew the hash, and the store has that content.
    if machine.is_none()
        && let Some(hash) = listed_hash
        && store.exists(&hash).await.map_err(ApiError::internal)?
    {
        return Ok(ready(hash, size));
    }

    // 3. Offline, nothing more can be had.
    let Some(machine) = machine else {
        return Ok(unavailable(FetchUnavailable::OfflineNotCached));
    };
    // 4. A file larger than a blob may be is listed and cannot be read.
    let max_bytes = crate::uploads::max_blob_bytes(&ctx.app).await?;
    if size > max_bytes {
        return Ok(unavailable(FetchUnavailable::TooLarge));
    }

    // 5. The machine is asked to upload it, once for each version.
    let dispatcher = ctx.app.link.dispatcher();
    let deadline = Time::new(
        ctx.now + ChronoDuration::seconds(i64::from(CommandKind::FsFetch.deadline_secs())),
    );
    let mut last = FetchState {
        state: FetchStatus::Fetching,
        hash: None,
        size: Some(size),
        received: None,
        reason: None,
    };
    for _ in 0..FETCH_ATTEMPTS {
        let id = live::id_to_ask(&dispatcher, |attempt| {
            fetch_id(env.id, &key, size, mtime_ms, attempt)
        })
        .await?;
        if dispatcher
            .status(id)
            .await
            .map_err(ApiError::internal)?
            .is_none()
        {
            dispatcher
                .issue_as(
                    id,
                    machine,
                    &SerialKey::None,
                    deadline,
                    &FsFetch {
                        environment_id: env.id,
                        mount: mount.clone(),
                        path: path.clone(),
                        // The time of last change is told to the millisecond, which is not what the
                        // machine compares; it uploads the version it finds and says which.
                        expect: None,
                        max_bytes: Some(max_bytes),
                    },
                )
                .await
                .map_err(ApiError::internal)?;
        }
        let outcome = if wait.is_zero() {
            let over = dispatcher
                .status(id)
                .await
                .map_err(ApiError::internal)?
                .is_some_and(|row| row.is_over());
            if over {
                Some(dispatcher.wait(id).await.map_err(ApiError::internal)?)
            } else {
                None
            }
        } else {
            match tokio::time::timeout(wait, dispatcher.wait(id)).await {
                Ok(outcome) => Some(outcome.map_err(ApiError::internal)?),
                Err(_) => None,
            }
        };
        let received = storage::received_of_machine(
            &mut conn,
            machine,
            size,
            ctx.now - ChronoDuration::minutes(5),
        )
        .await?;
        last.received = received;
        match outcome {
            None => return Ok(last),
            Some(CommandOutcome::Succeeded(result)) => {
                let result: FsFetchResult =
                    serde_json::from_value(result).map_err(ApiError::internal)?;
                record_fetched(&ctx, &mut conn, env.id, &key, &result).await?;
                return Ok(ready(result.hash, result.size));
            }
            Some(CommandOutcome::Failed(error)) => match error.code.as_str() {
                "too_large" => return Ok(unavailable(FetchUnavailable::TooLarge)),
                "ignored" => return Ok(unavailable(FetchUnavailable::Ignored)),
                "not_found" => return Ok(unavailable(FetchUnavailable::NotFound)),
                // The file changed while it was read: the next try reads the new one.
                "changed" => {}
                _ if error.retryable => {}
                _ => return Err(Failure::Machine(error).problem(Doing::Reading)),
            },
            Some(CommandOutcome::Expired | CommandOutcome::Cancelled) => {}
        }
    }
    Ok(last)
}

/// What the last list of the tree says a path is, for a machine that is away.
async fn kept_version(
    ctx: &Ctx,
    env: &orch_db::queries::environments::workspace::EnvironmentRow,
    path: &str,
) -> ApiResult<Seen> {
    let (true, Some(manifest)) = (listing::indexed(env), env.manifest_blob) else {
        // There is no list: only a file that was read before can be had, and the cache says so.
        return Ok(Seen::Missing(FetchUnavailable::OfflineNotCached));
    };
    let tree = ctx
        .app
        .cache
        .manifest_of(&ctx.app.blobs, env.id, &manifest)
        .await
        .map_err(ApiError::internal)?;
    Ok(match tree.get(path) {
        Some(entry) if entry.kind == crate::cache::EntryKind::File => Seen::File {
            size: entry.size,
            mtime_ms: listing::millis_of(entry.mtime_ns),
            hash: entry.hash,
        },
        Some(_) => Seen::NotAFile,
        None if tree.has_below(path) => Seen::NotAFile,
        None => Seen::Missing(FetchUnavailable::NotFound),
    })
}

/// Records that the content of a version of a file is in the store.
async fn record_fetched(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    environment: Id,
    key: &str,
    fetched: &FsFetchResult,
) -> ApiResult<()> {
    storage::register_blob(conn, &fetched.hash, fetched.size, ctx.now).await?;
    storage::record_cached(
        conn,
        environment,
        &CachedFile {
            path: key.to_owned(),
            size: fetched.size,
            mtime_ns: fetched.mtime_ns,
            hash: fetched.hash,
            cached_at: ctx.now,
        },
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- reading the content

/// How a name is written in a `Content-Disposition` header: the plain form for a reader that knows
/// no other, and the encoded one for the rest.
fn disposition(kind: &str, name: &str) -> String {
    let plain: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("{kind}; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

/// The answer that serves a blob: the content, or the range of it that was asked for, with its hash
/// as its tag. Active content (a page, a vector image) is never shown inline as itself.
pub fn serve(
    read: BlobRead,
    asked_range: bool,
    request: &HeaderMap,
    hash: &Hash,
    name: &str,
    media_type: &str,
    download: bool,
) -> Response {
    let etag = format!("\"{hash}\"");
    let matches = request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|each| each.trim() == etag));
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    headers.insert(
        header::VARY,
        HeaderValue::from_static("Authorization, Cookie"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    if matches && !asked_range {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    let download = download || !crate::media::shows_inline(media_type);
    let shown = if download {
        crate::media::UNKNOWN
    } else {
        media_type
    };
    if let Ok(value) = HeaderValue::from_str(shown) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&disposition(
        if download { "attachment" } else { "inline" },
        name,
    )) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&read.range.len.to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    let partial = asked_range && !read.range.is_whole(read.meta.size);
    if partial && let Ok(value) = HeaderValue::from_str(&read.range.content_range(read.meta.size)) {
        headers.insert(header::CONTENT_RANGE, value);
    }
    let status = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let body = Body::from_stream(futures::StreamExt::map(read.body, |piece| {
        piece.map_err(std::io::Error::other)
    }));
    (status, headers, body).into_response()
}

/// The answer to a range that holds no byte of the content.
pub fn unsatisfiable(size: i64) -> Response {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    if let Ok(value) = HeaderValue::from_str(&unsatisfied_content_range(size)) {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    response
}

/// The range a request asks for, if it asks for one this serves.
#[must_use]
pub fn range_of(headers: &HeaderMap) -> Option<ByteRange> {
    headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(ByteRange::from_header)
}

/// Reads a blob for a request: the whole or the range asked for, the problems of the store being the
/// problems a client acts on.
pub async fn read_for(
    ctx: &Ctx,
    hash: &Hash,
    name: &str,
    media_type: &str,
    download: bool,
) -> ApiResult<Response> {
    let range = range_of(&ctx.headers);
    match ctx.app.blobs.read(hash, range).await {
        Ok(read) => Ok(serve(
            read,
            range.is_some(),
            &ctx.headers,
            hash,
            name,
            media_type,
            download,
        )),
        Err(BlobError::NotFound(_)) => Err(ApiError::not_found()),
        Err(BlobError::RangeNotSatisfiable { size }) => Ok(unsatisfiable(size)),
        Err(error) => Err(ApiError::internal(error)),
    }
}

/// The bytes of one version of a file, found by their hash.
async fn content_get(
    ctx: Ctx,
    query: EnvironmentFileContentGetQuery,
    _: Body,
) -> ApiResult<Response> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = paths::clean("path", &query.path, false)?;
    let mount = paths::mount("mount", query.mount.as_deref())?;
    let key = paths::cache_key(mount.as_deref(), &path);
    // The hash has to belong to that path in this environment: it was cached for it, or the list of
    // the tree says that path has that content.
    let mut belongs = storage::hash_belongs(&mut conn, door.env.id, &key, &query.hash).await?;
    if !belongs
        && mount.is_none()
        && listing::indexed(&door.env)
        && let Some(manifest) = door.env.manifest_blob
        && let Ok(tree) = ctx
            .app
            .cache
            .manifest_of(&ctx.app.blobs, door.env.id, &manifest)
            .await
    {
        belongs = tree.get(&path).and_then(|entry| entry.hash) == Some(query.hash);
    }
    if !belongs {
        return Err(ApiError::not_found());
    }
    storage::touch_blob(&mut conn, &query.hash, ctx.now).await?;
    drop(conn);
    let name = paths::name_of(&path);
    let media = crate::media::media_type(name, &[]);
    if query.preview == Some(true) {
        return crate::previews::serve(&ctx, &query.hash, name, media).await;
    }
    read_for(
        &ctx,
        &query.hash,
        name,
        media,
        query.download.unwrap_or(false),
    )
    .await
}

// ---------------------------------------------------------------- writing

/// What the caller wrote the file on: the hash they loaded, or that there was none.
fn expected_of(text: Option<&str>) -> ApiResult<Option<ExpectedHash>> {
    match text {
        None => Ok(None),
        Some(EXPECTED_HASH_NONE) => Ok(Some(ExpectedHash::Absent)),
        Some(other) => other
            .parse::<Hash>()
            .map(|hash| Some(ExpectedHash::Is(hash)))
            .map_err(|_| {
                ApiError::invalid("expected_hash", "this is the hash of a version, or `none`")
            }),
    }
}

/// Writes the audit row of a change of the workspace, in the caller's transaction.
async fn audited(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    door: &Door,
    action: AuditAction,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        action.as_str(),
        "environment",
        Some(door.env.id),
        summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = Some(door.env.home_node_id);
    entry.org_id = Some(door.env.org_id);
    entry.environment_id = Some(door.env.id);
    entry.details = details;
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}

/// What a person changing files needs before the machine is asked: the right to write through their
/// door, the environment's own switch, and a machine that answers.
async fn writer(ctx: &Ctx, conn: &mut orch_db::Conn, thread: Option<Id>) -> ApiResult<(Door, Id)> {
    let door = door::open(ctx, conn, thread).await?;
    door.may_write()?;
    let machine = live::require_live(ctx, conn, &door.env).await?;
    Ok((door, machine))
}

/// The summary of an act of the caller, for the history of the environment.
fn said(ctx: &Ctx, what: &str) -> ApiResult<String> {
    Ok(format!("{} {what}", crate::routes::nodes::actor_name(ctx)?))
}

/// Writes a file from the body or from an upload, if the file is still the version the writer saw.
async fn content_put(
    ctx: Ctx,
    query: EnvironmentFileContentPutQuery,
    body: Body,
) -> ApiResult<Response> {
    let mut conn = ctx.app.db.acquire().await?;
    let (door, machine) = writer(&ctx, &mut conn, query.thread).await?;
    let path = paths::clean("path", &query.path, false)?;
    let expected = expected_of(query.expected_hash.as_deref())?;
    drop(conn);
    if let Some(hash) = query.blob {
        let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
        crate::attachments::require(&mut tx, guard.principal(), &hash).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
    }
    let meta = match query.blob {
        Some(hash) => {
            let size = ctx
                .app
                .blobs
                .store()
                .size(&hash)
                .await
                .map_err(ApiError::internal)?
                .ok_or_else(ApiError::not_found)?;
            BlobMeta { hash, size }
        }
        None => ctx
            .app
            .blobs
            .store_stream(
                body.into_data_stream(),
                i64::try_from(MAX_BODY_BYTES).unwrap_or(i64::MAX),
            )
            .await
            .map_err(crate::uploads::stored_badly)?,
    };
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    if query.blob.is_some() {
        crate::attachments::require(&mut tx, guard.principal(), &meta.hash).await?;
    }
    ctx.app.blobs.register(&mut tx, meta, ctx.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let written = live::run(
        &ctx,
        machine,
        SerialKey::Environment(door.env.id),
        &FsWrite {
            work: None,
            environment_id: door.env.id,
            path: path.clone(),
            blob: meta.hash,
            mode: None,
            expected_hash: expected,
        },
    )
    .await;
    let stat = match written {
        Ok(found) => found,
        Err(Failure::Machine(error)) if error.code == "conflict" => {
            let mut problem = ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the file is not the version that was loaded; nothing was written",
            );
            if let Some(current) = error
                .details
                .as_ref()
                .and_then(|details| details.get("current"))
            {
                problem = problem.with("current", current);
            }
            return Err(problem);
        }
        Err(failure) => return Err(failure.problem(Doing::Writing)),
    };
    let mut tx = ctx.app.db.begin().await?;
    let hash = stat.hash.unwrap_or(meta.hash);
    storage::record_cached(
        &mut tx,
        door.env.id,
        &CachedFile {
            path: path.clone(),
            size: stat.entry.size,
            mtime_ns: stat
                .entry
                .mtime
                .get()
                .timestamp_millis()
                .saturating_mul(NANOS_PER_MILLI),
            hash,
            cached_at: ctx.now,
        },
    )
    .await?;
    audited(
        &ctx,
        &mut tx,
        &door,
        orch_db::audit::action!(AuditAction::WorkspaceFileWritten),
        &said(&ctx, &format!("wrote {path}"))?,
        serde_json::json!({ "path": path, "size": stat.entry.size, "hash": hash,
            "upload_batch": query.upload_batch, "replaced": query.expected_hash.is_none() }),
    )
    .await?;
    orch_db::queries::environments::thread_engine::touch_activity(&mut tx, door.env.id, ctx.now)
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    json_reply(StatusCode::OK, &stat)
}

/// Admits one uploaded file before dispatch, retaining its content until the job's machine stops.
async fn upload_write(
    ctx: Ctx,
    query: orch_api_contract::environment::ThroughThreadQuery,
    body: EnvironmentUploadWriteBody,
) -> ApiResult<orch_api_contract::workspace::Job> {
    use orch_db::queries::jobs::{self, NewJob, kind};
    use orch_engine::jobs::workspace_upload::{Admission, PHASES};
    let id = ctx.id("upload_id")?;
    let path = paths::clean("path", &body.path, false)?;
    let expected = expected_of(body.expected_hash.as_deref())?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    orch_db::queries::chats::lock_named(&mut tx, &format!("workspace.upload:{id}")).await?;
    if let Some(row) = jobs::job(&mut tx, id).await? {
        if row.kind != kind::DELIVERY
            || row.created_by != Some(principal.user())
            || row.environment_id != Some(ctx.id("id")?)
            || row.plan.get("workspace_upload").is_none()
        {
            return Err(ApiError::not_found());
        }
        let admitted: Admission = serde_json::from_value(row.plan["workspace_upload"].clone())
            .map_err(ApiError::internal)?;
        if admitted.write.path != path
            || admitted.write.blob != body.blob
            || admitted.write.expected_hash != expected
            || admitted.batch != body.upload_batch
        {
            return Err(ApiError::new(
                ErrorCode::ConflictExists,
                "this upload write already names different content",
            ));
        }
        let shown = crate::routes::environments::jobs::view(&mut tx, &row).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(shown);
    }
    let (door, machine) = writer(&ctx, &mut tx, query.thread).await?;
    crate::attachments::require(&mut tx, &principal, &body.blob).await?;
    let size = ctx
        .app
        .blobs
        .store()
        .size(&body.blob)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    ctx.app
        .blobs
        .register(
            &mut tx,
            BlobMeta {
                hash: body.blob,
                size,
            },
            ctx.now,
        )
        .await?;
    if ctx.app.blobs.pin(&mut tx, &[body.blob]).await? != 0 {
        return Err(ApiError::internal(
            "the upload content could not be retained",
        ));
    }
    let admitted = Admission {
        machine,
        generation: door.env.generation,
        write: FsWrite {
            work: None,
            environment_id: door.env.id,
            path: path.clone(),
            blob: body.blob,
            mode: None,
            expected_hash: expected,
        },
        size,
        home: door.env.home_node_id,
        org: door.env.org_id,
        by: principal.user(),
        origin: origin(&ctx),
        summary: said(&ctx, &format!("wrote {path}"))?,
        batch: body.upload_batch,
    };
    let mut progress = orch_engine::jobs::runner::initial_progress(&PHASES);
    progress.files_total = Some(1);
    progress.bytes_total = Some(size);
    let row = jobs::insert(
        &mut tx,
        &NewJob {
            id,
            kind: kind::DELIVERY,
            node_id: query.thread,
            environment_id: Some(door.env.id),
            provider_id: None,
            plan: &serde_json::json!({"workspace_upload":admitted}),
            progress: &serde_json::to_value(progress).map_err(ApiError::internal)?,
            created_by: Some(principal.user()),
        },
        ctx.now,
    )
    .await?;
    let shown = crate::routes::environments::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}

/// The file's actual result remains readable without issuing another filesystem command.
async fn upload_write_result(
    ctx: Ctx,
    query: orch_api_contract::environment::ThroughThreadQuery,
    (): (),
) -> ApiResult<FileStat> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let row = orch_db::queries::jobs::job(&mut conn, ctx.id("upload_id")?)
        .await?
        .filter(|row| {
            row.kind == orch_db::queries::jobs::kind::DELIVERY
                && row.environment_id == Some(door.env.id)
                && row.plan.get("workspace_upload").is_some()
        })
        .ok_or_else(ApiError::not_found)?;
    if row.is_open() {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "the uploaded file is still being written or stopped",
        ));
    }
    if let Some(written) = row
        .result
        .as_ref()
        .filter(|value| !value.is_null())
        .or_else(|| row.plan.get("written").filter(|value| !value.is_null()))
    {
        return serde_json::from_value(written.clone()).map_err(ApiError::internal);
    }
    if let Some(error) = row.error.as_ref().filter(|_| row.state == "failed") {
        return Err(
            Failure::Machine(orch_link_contract::commands::CommandError {
                code: error["code"].as_str().unwrap_or("internal").into(),
                detail: error["message"]
                    .as_str()
                    .unwrap_or("the upload could not be written")
                    .into(),
                retryable: false,
                details: None,
            })
            .problem(Doing::Writing),
        );
    }
    Err(ApiError::new(
        ErrorCode::PlanBlocked,
        "the upload stopped without a confirmed file write",
    ))
}

/// History groups a browser batch; its audit still holds each guarded write and its actual bytes.
async fn upload_finish(
    ctx: Ctx,
    query: orch_api_contract::environment::ThroughThreadQuery,
    body: EnvironmentUploadFinishBody,
) -> ApiResult<()> {
    let batch = ctx.id("upload_id")?;
    let folder = paths::clean("folder", &body.folder, true)?;
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let door = door::open(&ctx, &mut tx, query.thread).await?;
    // This records retained receipts, not a machine write; it must also work after writes were switched off.
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
        .bind(door.env.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    let done: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE environment_id=$1 AND actor_id=$2 AND action='workspace.file_written' AND details->'upload'->>'id'=$3)")
        .bind(door.env.id).bind(guard.user().id()).bind(batch.to_string()).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
    if done {
        return Ok(());
    }
    let writes: Vec<Json> = sqlx::query_scalar("SELECT details FROM audit_log WHERE environment_id=$1 AND actor_id=$2 AND action='workspace.file_written' AND details->>'upload_batch'=$3 ORDER BY id")
        .bind(door.env.id).bind(guard.user().id()).bind(batch.to_string()).fetch_all(&mut *tx).await.map_err(orch_db::DbError::from)?;
    let mut files = Vec::new();
    let mut total = Some(0_i64);
    let mut replaced = 0_i64;
    for write in &writes {
        let path = write
            .get("path")
            .and_then(Json::as_str)
            .ok_or_else(|| ApiError::internal("an upload write has no path"))?;
        let size = write
            .get("size")
            .and_then(Json::as_i64)
            .ok_or_else(|| ApiError::internal("an upload write has no size"))?;
        let was_replaced = write
            .get("replaced")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        if was_replaced {
            replaced += 1;
        }
        total = total.map(|bytes| bytes.saturating_add(size));
        files.push(serde_json::json!({"name":paths::name_of(path),"path":path,"size":size,"received":size,"outcome":"complete","replaced":was_replaced}));
    }
    // What was written, and the receipts of the attempts that were not, are each read once for the whole upload.
    let written_hashes: std::collections::HashSet<&str> = writes
        .iter()
        .filter_map(|write| write.get("hash").and_then(Json::as_str))
        .collect();
    let unwritten = |attempt: &&orch_api_contract::workspace::WorkspaceUploadAttempt| {
        !(attempt_written(attempt, &written_hashes) && attempt_complete(attempt))
    };
    let sessions: Vec<Id> = body
        .files
        .iter()
        .filter(unwritten)
        .filter_map(|attempt| attempt.upload_id)
        .collect();
    let receipts: std::collections::HashMap<Id, (String, i64, i64)> =
        sqlx::query_as::<_, (Id, String, i64, i64)>(
            "SELECT id,name,size,received FROM upload_sessions WHERE id=ANY($1) AND user_id=$2",
        )
        .bind(&sessions)
        .bind(guard.user().id())
        .fetch_all(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?
        .into_iter()
        .map(|(id, name, size, received)| (id, (name, size, received)))
        .collect();
    let blobs: Vec<Vec<u8>> = body
        .files
        .iter()
        .filter(unwritten)
        .filter_map(|attempt| attempt.blob.map(|blob| blob.as_bytes().to_vec()))
        .collect();
    let completed: std::collections::HashMap<Vec<u8>, i64> = sqlx::query_as::<_, (Vec<u8>, i64)>(
        "SELECT u.hash,b.size FROM completed_uploads u JOIN blobs b ON b.hash=u.hash WHERE u.user_id=$1 AND u.hash=ANY($2)",
    )
    .bind(guard.user().id())
    .bind(&blobs)
    .fetch_all(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?
    .into_iter()
    .collect();
    for attempt in &body.files {
        let name = paths::clean("files.name", &attempt.name, false)?;
        if attempt.size < 0 {
            return Err(ApiError::invalid(
                "files.size",
                "a file size is not negative",
            ));
        }
        if attempt_written(attempt, &written_hashes) && attempt_complete(attempt) {
            continue;
        }
        let mut received = None;
        if let Some(session) = attempt.upload_id {
            let receipt = receipts.get(&session).cloned();
            if let Some((stored_name, size, count)) = receipt {
                if stored_name != crate::uploads::clean_name("files.name", paths::name_of(&name))?
                    || size != attempt.size
                {
                    return Err(ApiError::invalid(
                        "files.upload_id",
                        "the upload receipt is for another file",
                    ));
                }
                received = Some(count);
            }
        }
        if received.is_none()
            && let Some(blob) = attempt.blob
        {
            received = completed.get(blob.as_bytes().as_slice()).copied();
        }
        total = total
            .zip(received)
            .map(|(bytes, count)| bytes.saturating_add(count));
        let outcome = if attempt.outcome == WorkspaceUploadOutcome::Complete {
            WorkspaceUploadOutcome::Failed
        } else {
            attempt.outcome
        };
        files.push(serde_json::json!({"name":name,"path":Json::Null,"size":attempt.size,"received":received,"outcome":outcome,"replaced":false}));
    }
    let outcome = if body.outcome == WorkspaceUploadOutcome::Failed
        || files.iter().any(|file| file["outcome"] == "failed")
    {
        "failed"
    } else if files.iter().any(|file| file["outcome"] == "cancelled")
        || body.outcome == WorkspaceUploadOutcome::Cancelled
    {
        "cancelled"
    } else {
        "complete"
    };
    let names = files
        .iter()
        .take(HISTORY_UPLOAD_NAMES)
        .filter_map(|file| file.get("name").and_then(Json::as_str))
        .collect::<Vec<_>>()
        .join(", ");
    let rest = files.len().saturating_sub(HISTORY_UPLOAD_NAMES);
    let action = match outcome {
        "failed" => "upload failed",
        "cancelled" => "upload cancelled",
        _ => "uploaded",
    };
    let what = format!(
        "{action} {names}{} to {}",
        if rest == 0 {
            String::new()
        } else {
            format!(" and {rest} more")
        },
        if folder.is_empty() { "/" } else { &folder }
    );
    audited(&ctx, &mut tx, &door, orch_db::audit::action!(AuditAction::WorkspaceFileWritten), &said(&ctx, &what)?,
        serde_json::json!({"upload":{"id":batch,"environment_id":door.env.id,"folder":folder,"files":files,"bytes":total,"replaced":replaced,"outcome":outcome}})).await?;
    // This changes only History's grouping; every individual audit fact and its content stays available.
    sqlx::query("UPDATE audit_log SET history=false WHERE environment_id=$1 AND actor_id=$2 AND action='workspace.file_written' AND details->>'upload_batch'=$3")
        .bind(door.env.id).bind(guard.user().id()).bind(batch.to_string()).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Whether the bytes of an upload attempt were written as one of the upload's files.
fn attempt_written(
    attempt: &orch_api_contract::workspace::WorkspaceUploadAttempt,
    written: &std::collections::HashSet<&str>,
) -> bool {
    attempt
        .blob
        .is_some_and(|blob| written.contains(blob.to_string().as_str()))
}

/// Whether the person's side says an upload attempt finished.
fn attempt_complete(attempt: &orch_api_contract::workspace::WorkspaceUploadAttempt) -> bool {
    attempt.outcome == WorkspaceUploadOutcome::Complete
}

/// Makes a directory.
async fn mkdir(
    ctx: Ctx,
    query: orch_api_contract::environment::ThroughThreadQuery,
    body: FilePathBody,
) -> ApiResult<FileStat> {
    let mut conn = ctx.app.db.acquire().await?;
    let (door, machine) = writer(&ctx, &mut conn, query.thread).await?;
    drop(conn);
    let path = paths::clean("path", &body.path, false)?;
    let made = live::run(
        &ctx,
        machine,
        SerialKey::Environment(door.env.id),
        &FsMkdir {
            environment_id: door.env.id,
            path,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Making))?;
    Ok(made)
}

/// The two paths of a rename or a copy.
fn two_paths(body: &FileFromToBody) -> ApiResult<(String, String)> {
    let from = paths::clean("from", &body.from, false)?;
    let to = paths::clean("to", &body.to, false)?;
    if from == to {
        return Err(ApiError::invalid("to", "this is where it is already"));
    }
    if to.starts_with(&format!("{from}/")) {
        return Err(ApiError::invalid(
            "to",
            "a directory is not put inside itself",
        ));
    }
    Ok((from, to))
}

/// Renames a file or a directory, which is also how one is moved.
async fn rename(
    ctx: Ctx,
    query: orch_api_contract::environment::ThroughThreadQuery,
    body: FileFromToBody,
) -> ApiResult<FileStat> {
    let (from, to) = two_paths(&body)?;
    let mut conn = ctx.app.db.acquire().await?;
    let (door, machine) = writer(&ctx, &mut conn, query.thread).await?;
    drop(conn);
    let moved = live::run(
        &ctx,
        machine,
        SerialKey::Environment(door.env.id),
        &FsRename {
            environment_id: door.env.id,
            from: from.clone(),
            to: to.clone(),
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Renaming))?;
    let stat = moved;
    let mut tx = ctx.app.db.begin().await?;
    storage::forget_cached_under(&mut tx, door.env.id, &from).await?;
    audited(
        &ctx,
        &mut tx,
        &door,
        orch_db::audit::action!(AuditAction::WorkspaceFileRenamed),
        &said(&ctx, &format!("renamed {from} to {to}"))?,
        serde_json::json!({ "from": from, "to": to, "size": stat.entry.size, "hash": stat.hash }),
    )
    .await?;
    orch_db::queries::environments::thread_engine::touch_activity(&mut tx, door.env.id, ctx.now)
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(stat)
}

/// Copies a file, or a directory with what is in it.
async fn copy(
    ctx: Ctx,
    query: orch_api_contract::environment::ThroughThreadQuery,
    body: FileFromToBody,
) -> ApiResult<FileStat> {
    let (from, to) = two_paths(&body)?;
    let mut conn = ctx.app.db.acquire().await?;
    let (door, machine) = writer(&ctx, &mut conn, query.thread).await?;
    drop(conn);
    let copied = live::run(
        &ctx,
        machine,
        SerialKey::Environment(door.env.id),
        &FsCopy {
            environment_id: door.env.id,
            from: from.clone(),
            to: to.clone(),
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Copying))?;
    let stat = copied;
    let mut tx = ctx.app.db.begin().await?;
    audited(
        &ctx,
        &mut tx,
        &door,
        orch_db::audit::action!(AuditAction::WorkspaceFileCopied),
        &said(&ctx, &format!("copied {from} to {to}"))?,
        serde_json::json!({ "from": from, "to": to, "size": stat.entry.size, "hash": stat.hash }),
    )
    .await?;
    orch_db::queries::environments::thread_engine::touch_activity(&mut tx, door.env.id, ctx.now)
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(stat)
}

/// Deletes a file, or a directory with what is in it.
async fn delete(ctx: Ctx, query: EnvironmentFileDeleteQuery, (): ()) -> ApiResult<()> {
    let path = paths::clean("path", &query.path, false)?;
    let mut conn = ctx.app.db.acquire().await?;
    let (door, machine) = writer(&ctx, &mut conn, query.thread).await?;
    drop(conn);
    live::run(
        &ctx,
        machine,
        SerialKey::Environment(door.env.id),
        &FsRemove {
            environment_id: door.env.id,
            path: path.clone(),
            recursive: query.recursive.unwrap_or(false),
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Removing))?;
    let mut tx = ctx.app.db.begin().await?;
    storage::forget_cached_under(&mut tx, door.env.id, &path).await?;
    audited(
        &ctx,
        &mut tx,
        &door,
        orch_db::audit::action!(AuditAction::WorkspaceFileDeleted),
        &said(&ctx, &format!("deleted {path}"))?,
        serde_json::json!({ "path": path, "recursive": query.recursive.unwrap_or(false) }),
    )
    .await?;
    orch_db::queries::environments::thread_engine::touch_activity(&mut tx, door.env.id, ctx.now)
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_has_one_identifier_for_its_fetch_and_another_for_each_new_try() {
        let environment = orch_types::new_id();
        let first = fetch_id(environment, "src/main.rs", 120, 1_000, 0);
        assert_eq!(first, fetch_id(environment, "src/main.rs", 120, 1_000, 0));
        assert_ne!(first, fetch_id(environment, "src/main.rs", 120, 1_000, 1));
        assert_ne!(first, fetch_id(environment, "src/main.rs", 121, 1_000, 0));
        assert_ne!(first, fetch_id(environment, "src/main.rs", 120, 1_001, 0));
        assert_ne!(first, fetch_id(environment, "src/lib.rs", 120, 1_000, 0));
    }

    #[test]
    fn a_name_with_more_than_ascii_is_offered_to_a_download_in_both_forms() {
        let header = disposition("attachment", "répertoire \"x\".txt");
        assert!(header.starts_with("attachment; filename=\""));
        assert!(header.contains("filename*=UTF-8''r%C3%A9pertoire%20%22x%22.txt"));
        assert!(!header[..header.find("filename*").unwrap_or(0)].contains("\"x\""));
    }

    #[test]
    fn what_a_write_was_made_on_is_a_hash_or_the_word_none() {
        assert!(matches!(expected_of(None), Ok(None)));
        assert!(matches!(
            expected_of(Some("none")),
            Ok(Some(ExpectedHash::Absent))
        ));
        let hash = Hash::of(b"x");
        let text = hash.to_string();
        assert!(
            matches!(expected_of(Some(&text)), Ok(Some(ExpectedHash::Is(found))) if found == hash)
        );
        assert!(expected_of(Some("latest")).is_err());
    }
}
