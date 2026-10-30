//! What can be found out about a workspace without opening its files one by one: the latest change of each path that agents made, a search by name or inside the text, what an archive holds, and the state and differences of a repository.
//!
//! The names of files are searched in the last list of the tree and work while the machine is away;
//! everything else is asked of the machine, which has to be there. Looking into an archive reads its
//! names, sizes and dates where it lies and gives one entry of it for a viewer, up to the size a
//! preview shows; nothing is written into the workspace and nothing is run. A repository is read by
//! the environment's own `git`, read-only, and what a machine without one says is told as a state of
//! the answer and not as a failure.

use super::door;
use super::files;
use super::listing::{self, indexed};
use super::live;
use super::paths;
use super::problems::Doing;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::workspace::{
    ARCHIVE_LIST_LIMIT_DEFAULT, ARCHIVE_LIST_LIMIT_MAX, ArchiveListing, EnvironmentArchiveEntryGet,
    EnvironmentArchiveEntryGetQuery, EnvironmentArchiveList, EnvironmentArchiveListQuery,
    EnvironmentChange, EnvironmentChangeList, EnvironmentChangeListQuery,
    EnvironmentChangeListReply, EnvironmentFileSearch, EnvironmentFileSearchQuery,
    EnvironmentFileSearchReply, EnvironmentGitDiffGet, EnvironmentGitDiffGetQuery,
    EnvironmentGitStatusGet, EnvironmentGitStatusGetQuery, FILE_SEARCH_LIMIT_DEFAULT,
    FILE_SEARCH_LIMIT_MAX, FILE_SEARCH_NAMES_MAX, FILE_SEARCH_QUERY_MIN_CHARS, FileSearchContent,
    FileSearchIn, GIT_DIFF_MAX_BYTES, GitStatus, GitUnavailable, TRUNCATED_HEADER,
};
use orch_db::queries::environments::workspace as rows;
use orch_link_contract::commands::{
    FsArchiveList, FsArchiveRead, FsSearch, GitDiff, GitStatus as GitStatusCommand, SerialKey,
};
use orch_types::Time;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(EnvironmentChangeList, changes)
        .on(EnvironmentFileSearch, search)
        .on(EnvironmentArchiveList, archive_list)
        .on_raw(EnvironmentArchiveEntryGet, archive_entry)
        .on(EnvironmentGitStatusGet, git_status)
        .on_raw(EnvironmentGitDiffGet, git_diff);
}

// ---------------------------------------------------------------- what agents changed

/// The latest change to each path that agents made: those of the thread the caller came in by, or,
/// with `all` and for a caller who came in by managing the environment, those of every thread there.
async fn changes(
    ctx: Ctx,
    query: EnvironmentChangeListQuery,
    (): (),
) -> ApiResult<EnvironmentChangeListReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let thread = if query.all.unwrap_or(false) {
        None
    } else {
        door.thread
    };
    let found = rows::latest_changes(&mut conn, door.env.id, thread, query.turn_id).await?;
    let changes = found
        .into_iter()
        .map(|row| {
            Ok(EnvironmentChange {
                change: serde_json::from_value(row.change).map_err(ApiError::internal)?,
                item_id: row.item_id,
                thread_id: row.thread_id,
                at: Time::new(row.at),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(EnvironmentChangeListReply { changes })
}

// ---------------------------------------------------------------- searching

/// Searches the files of an environment by name, from the last list of the tree, or the lines of its
/// text files, in the environment as it is now.
async fn search(
    ctx: Ctx,
    query: EnvironmentFileSearchQuery,
    (): (),
) -> ApiResult<EnvironmentFileSearchReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let asked = query.q.trim();
    let min = usize::try_from(FILE_SEARCH_QUERY_MIN_CHARS).unwrap_or(usize::MAX);
    if asked.chars().count() < min {
        return Err(ApiError::invalid(
            "q",
            &format!("a search is for at least {FILE_SEARCH_QUERY_MIN_CHARS} characters"),
        ));
    }
    match query.scope.unwrap_or(FileSearchIn::Names) {
        FileSearchIn::Names => {
            if !indexed(&door.env) {
                return Err(ApiError::new(
                    ErrorCode::WorkspaceNotIndexed,
                    "the tree is too large to keep a list of; the names of files are not searched",
                ));
            }
            let machine_is_there = live::machine_of(&ctx, &mut conn, &door.env)
                .await?
                .is_some();
            let Some(manifest) = door.env.manifest_blob else {
                return Err(if machine_is_there {
                    ApiError::new(
                        ErrorCode::EnvironmentUnavailable,
                        "the machine has not uploaded a list of the tree yet",
                    )
                } else {
                    live::offline()
                });
            };
            let tree = ctx
                .app
                .cache
                .manifest_of(&ctx.app.blobs, door.env.id, &manifest)
                .await
                .map_err(ApiError::internal)?;
            let limit = usize::try_from(FILE_SEARCH_NAMES_MAX).unwrap_or(usize::MAX);
            let mut found: Vec<_> = tree
                .find(&asked.to_lowercase(), limit)
                .into_iter()
                .map(listing::dir_entry)
                .collect();
            listing::mark_cached(&mut conn, door.env.id, None, &mut found).await?;
            Ok(EnvironmentFileSearchReply::Names(found))
        }
        FileSearchIn::Content => {
            let limit = match query.limit {
                None => FILE_SEARCH_LIMIT_DEFAULT,
                Some(0) => return Err(ApiError::invalid("limit", "must be at least 1")),
                Some(limit) => limit.min(FILE_SEARCH_LIMIT_MAX),
            };
            // Where to look: the directory asked for, or the thread's own.
            let path = match &query.path {
                Some(path) => paths::clean("path", path, true)?,
                None => door.directory.clone().unwrap_or_default(),
            };
            let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
            drop(conn);
            let found = live::run(
                &ctx,
                machine,
                SerialKey::None,
                &FsSearch {
                    environment_id: door.env.id,
                    path,
                    query: asked.to_owned(),
                    regex: query.regex.unwrap_or(false),
                    case: query.case.unwrap_or(false),
                    glob: query.glob.clone().filter(|glob| !glob.trim().is_empty()),
                    limit,
                },
            )
            .await
            .map_err(|failure| failure.problem(Doing::Searching))?;
            Ok(EnvironmentFileSearchReply::Content(FileSearchContent {
                matches: found.matches,
                truncated: found.truncated,
                searched: found.searched,
            }))
        }
    }
}

// ---------------------------------------------------------------- archives

/// What an archive holds, read where it lies.
async fn archive_list(
    ctx: Ctx,
    query: EnvironmentArchiveListQuery,
    (): (),
) -> ApiResult<ArchiveListing> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = paths::clean("path", &query.path, false)?;
    let inside = match &query.inside {
        Some(inside) => {
            Some(inside.trim_matches('/').to_owned()).filter(|inside| !inside.is_empty())
        }
        None => None,
    };
    let limit = match query.limit {
        None => ARCHIVE_LIST_LIMIT_DEFAULT,
        Some(0) => return Err(ApiError::invalid("limit", "must be at least 1")),
        Some(limit) => limit.min(ARCHIVE_LIST_LIMIT_MAX),
    };
    let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
    drop(conn);
    let found = live::run(
        &ctx,
        machine,
        SerialKey::None,
        &FsArchiveList {
            environment_id: door.env.id,
            path,
            inside,
            limit,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Reading))?;
    Ok(found)
}

/// One entry of an archive, for a viewer: at most as much as a preview shows.
async fn archive_entry(
    ctx: Ctx,
    query: EnvironmentArchiveEntryGetQuery,
    _: Body,
) -> ApiResult<Response> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = paths::clean("path", &query.path, false)?;
    let entry = query.entry.trim_matches('/').to_owned();
    if entry.is_empty() || entry.split('/').any(|part| part == ".." || part.is_empty()) {
        return Err(ApiError::invalid(
            "entry",
            "this is the name of a file inside the archive",
        ));
    }
    let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
    let read = live::run(
        &ctx,
        machine,
        SerialKey::None,
        &FsArchiveRead {
            environment_id: door.env.id,
            path,
            entry: entry.clone(),
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Reading))?;
    let preview = i64::try_from(ctx.app.config.limits.preview_bytes).unwrap_or(i64::MAX);
    if read.blob.size > preview {
        return Err(ApiError::new(
            ErrorCode::RequestTooLarge,
            "that entry is larger than a preview shows",
        )
        .with("limit", preview));
    }
    orch_db::queries::storage::register_blob(&mut conn, &read.blob.hash, read.blob.size, ctx.now)
        .await?;
    drop(conn);
    let name = paths::name_of(&entry);
    let media = crate::media::media_type(name, &[]);
    files::read_for(&ctx, &read.blob.hash, name, media, false).await
}

// ---------------------------------------------------------------- repositories

fn no_repository(reason: GitUnavailable) -> GitStatus {
    GitStatus {
        available: false,
        reason: Some(reason),
        repository_dir: None,
        status_hash: None,
        remotes: Vec::new(),
        upstream: None,
        branch: None,
        head: None,
        ahead: None,
        behind: None,
        changes: Vec::new(),
    }
}

/// The state of the repository in the selected workspace directory.
async fn git_status(ctx: Ctx, query: EnvironmentGitStatusGetQuery, (): ()) -> ApiResult<GitStatus> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = query
        .path
        .as_deref()
        .map(|path| paths::clean("path", path, true))
        .transpose()?
        .or_else(|| door.directory.clone());
    let observation = orch_types::new_id();
    ctx.app
        .engine
        .begin_git_read(
            door.env.id,
            path.as_deref().unwrap_or(""),
            observation,
            Time::new(ctx.now),
        )
        .await
        .map_err(ApiError::internal)?;
    let Some(machine) = live::machine_of(&ctx, &mut conn, &door.env).await? else {
        let status = no_repository(GitUnavailable::Offline);
        drop(conn);
        ctx.app
            .engine
            .finish_git_read(
                door.env.id,
                path.as_deref().unwrap_or(""),
                observation,
                Time::new(ctx.now),
                &status,
            )
            .await
            .map_err(ApiError::internal)?;
        return Ok(status);
    };
    drop(conn);
    let found = live::run(
        &ctx,
        machine,
        SerialKey::None,
        &GitStatusCommand {
            environment_id: door.env.id,
            path: path.clone(),
            watch_id: None,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Repository))?;
    let status = found;
    ctx.app
        .engine
        .finish_git_read(
            door.env.id,
            path.as_deref().unwrap_or(""),
            observation,
            Time::new(ctx.now),
            &status,
        )
        .await
        .map_err(ApiError::internal)?;
    Ok(status)
}

/// The unified difference of the selected repository or path, as plain text; more than
/// two mebibytes is cut and says so in a header.
async fn git_diff(ctx: Ctx, query: EnvironmentGitDiffGetQuery, _: Body) -> ApiResult<Response> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let path = query
        .path
        .as_deref()
        .map(|path| paths::clean("path", path, true))
        .transpose()?
        .or_else(|| door.directory.clone());
    let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
    drop(conn);
    let found = live::run(
        &ctx,
        machine,
        SerialKey::None,
        &GitDiff {
            environment_id: door.env.id,
            path,
            staged: query.staged,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Repository))?;
    let limit = usize::try_from(GIT_DIFF_MAX_BYTES).unwrap_or(usize::MAX);
    let mut text = found.text;
    let mut truncated = found.truncated;
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        truncated = true;
    }
    let size = i64::try_from(text.len()).map_err(ApiError::internal)?;
    let etag = HeaderValue::from_str(&format!("\"{}\"", orch_types::Hash::of(text.as_bytes())))
        .map_err(ApiError::internal)?;
    let mut response = if let Some(range) = files::range_of(&ctx.headers) {
        match range.resolve(size) {
            Ok(range) => {
                let start = usize::try_from(range.start).map_err(ApiError::internal)?;
                let end = usize::try_from(range.end()).map_err(ApiError::internal)?;
                let mut response = (
                    StatusCode::PARTIAL_CONTENT,
                    text.into_bytes()[start..end].to_vec(),
                )
                    .into_response();
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&range.content_range(size))
                        .map_err(ApiError::internal)?,
                );
                response
            }
            Err(_) => files::unsatisfiable(size),
        }
    } else {
        (StatusCode::OK, text).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::ETAG, etag);
    if truncated {
        headers.insert(
            header::HeaderName::from_static(TRUNCATED_HEADER),
            HeaderValue::from_static("1"),
        );
    }
    Ok(response)
}
