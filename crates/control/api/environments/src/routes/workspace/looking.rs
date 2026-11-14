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

use super::listing;

use super::listing::indexed;

use super::live;

use super::paths;

use super::problems::Doing;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use axum::body::Body;

use axum::response::IntoResponse;

use axum::response::Response;

use http::HeaderValue;

use http::StatusCode;

use http::header;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::workspace::ArchiveListing;

use orch_api_contract::workspace::EnvironmentArchiveEntryGet;

use orch_api_contract::workspace::EnvironmentArchiveEntryGetQuery;

use orch_api_contract::workspace::EnvironmentArchiveList;

use orch_api_contract::workspace::EnvironmentArchiveListQuery;

use orch_api_contract::workspace::EnvironmentChange;

use orch_api_contract::workspace::EnvironmentChangeList;

use orch_api_contract::workspace::EnvironmentChangeListQuery;

use orch_api_contract::workspace::EnvironmentChangeListReply;

use orch_api_contract::workspace::EnvironmentFileSearch;

use orch_api_contract::workspace::EnvironmentFileSearchQuery;

use orch_api_contract::workspace::EnvironmentFileSearchReply;

use orch_api_contract::workspace::EnvironmentGitDiffGet;

use orch_api_contract::workspace::EnvironmentGitDiffGetQuery;

use orch_api_contract::workspace::EnvironmentGitStatusGet;

use orch_api_contract::workspace::EnvironmentGitStatusGetQuery;

use orch_api_contract::workspace::FILE_SEARCH_QUERY_MIN_CHARS;

use orch_api_contract::workspace::FileSearchContent;

use orch_api_contract::workspace::FileSearchIn;

use orch_api_contract::workspace::GitStatus;

use orch_api_contract::workspace::GitUnavailable;

use orch_api_contract::workspace::TRUNCATED_HEADER;

use orch_db::queries::environments::workspace as rows;

use orch_link_contract::commands::FsArchiveList;

use orch_link_contract::commands::FsArchiveRead;

use orch_link_contract::commands::FsSearch;

use orch_link_contract::commands::GitDiff;

use orch_link_contract::commands::GitStatus as GitStatusCommand;

use orch_link_contract::commands::SerialKey;

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
    let found =
        rows::latest_changes(&mut conn, door.env.id, thread, query.turn_id, query.item_id).await?;
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
    let policy = orch_engine::environments::workspace_limits(&mut conn, door.env.id)
        .await
        .map_err(ApiError::internal)?;
    let limit = query
        .limit
        .unwrap_or(u32::try_from(policy.search_results).unwrap_or(u32::MAX));
    if limit == 0 {
        return Err(ApiError::invalid("limit", "must be at least 1"));
    }
    let scope = query.scope.unwrap_or(FileSearchIn::Names);
    if scope == FileSearchIn::Names
        && indexed(&door.env)
        && door.env.manifest_blob.is_some()
        && !query
            .cursor
            .as_deref()
            .is_some_and(|cursor| cursor.starts_with("live:"))
    {
        use base64::Engine as _;
        let codec = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let blob = door.env.manifest_blob.ok_or_else(super::pages::stale)?;
        let selection = serde_json::to_string(&(asked.to_lowercase(), &query.path, query.case))
            .map_err(ApiError::internal)?;
        let after = query
            .cursor
            .as_deref()
            .map(|text| {
                let text = text
                    .strip_prefix("cached:")
                    .ok_or_else(super::pages::stale)?;
                let bytes = codec.decode(text).map_err(|_| super::pages::stale())?;
                let cursor: CachedNames =
                    serde_json::from_slice(&bytes).map_err(|_| super::pages::stale())?;
                if cursor.environment != door.env.id
                    || cursor.generation != door.env.generation
                    || cursor.manifest != blob
                    || cursor.selection != selection
                {
                    return Err(super::pages::stale());
                }
                Ok(cursor.after)
            })
            .transpose()?;
        let tree = ctx
            .app
            .cache
            .manifest_of(&ctx.app.blobs, door.env.id, &blob)
            .await
            .map_err(ApiError::internal)?;
        let all = tree.entries();
        let start = after.as_ref().map_or(0, |after| {
            all.partition_point(|entry| entry.path.as_str() <= after.as_str())
        });
        let needle = if query.case == Some(true) {
            asked.to_owned()
        } else {
            asked.to_lowercase()
        };
        let mut found = Vec::new();
        let mut more = false;
        let mut searched = start;
        for entry in &all[start..] {
            searched += 1;
            if query.path.as_ref().is_some_and(|path| {
                !path.is_empty()
                    && entry.path != *path
                    && !entry.path.starts_with(&format!("{path}/"))
            }) {
                continue;
            }
            let matches = if query.case == Some(true) {
                entry.path.contains(&needle)
            } else {
                entry.path.to_lowercase().contains(&needle)
            };
            if !matches {
                continue;
            }
            if found.len() >= limit as usize {
                more = true;
                break;
            }
            found.push(listing::dir_entry(entry));
        }
        let next_cursor = if more {
            let after = found.last().ok_or_else(super::pages::stale)?.path.clone();
            Some(format!(
                "cached:{}",
                codec.encode(
                    serde_json::to_vec(&CachedNames {
                        environment: door.env.id,
                        generation: door.env.generation,
                        manifest: blob,
                        selection,
                        after,
                    })
                    .map_err(ApiError::internal)?
                )
            ))
        } else {
            None
        };
        listing::mark_cached(&mut conn, door.env.id, None, &mut found).await?;
        let as_of = Time::new(door.env.manifest_at.unwrap_or(ctx.now));
        return Ok(EnvironmentFileSearchReply::Names(
            orch_api_contract::workspace::FileSearchNames {
                entries: found,
                next_cursor,
                source: orch_api_contract::forks::FilesSource::Cached,
                as_of,
                observed_from: Some(as_of),
                searched: i64::try_from(searched).unwrap_or(i64::MAX),
            },
        ));
    }
    let path = match &query.path {
        Some(path) => paths::clean("path", path, true)?,
        None if scope == FileSearchIn::Content => door.directory.clone().unwrap_or_default(),
        None => String::new(),
    };
    let cursor = query
        .cursor
        .as_deref()
        .map(|cursor| {
            if scope == FileSearchIn::Names {
                cursor
                    .strip_prefix("live:")
                    .map(str::to_owned)
                    .ok_or_else(super::pages::stale)
            } else {
                Ok(cursor.to_owned())
            }
        })
        .transpose()?;
    let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
    drop(conn);
    let found = live::run(
        &ctx,
        machine,
        SerialKey::None,
        &FsSearch {
            environment_id: door.env.id,
            path,
            scope,
            cursor,
            query: asked.to_owned(),
            regex: query.regex.unwrap_or(false),
            case: query.case.unwrap_or(false),
            glob: query.glob.clone().filter(|glob| !glob.trim().is_empty()),
            limit,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Searching))?;
    match scope {
        FileSearchIn::Names => Ok(EnvironmentFileSearchReply::Names(
            orch_api_contract::workspace::FileSearchNames {
                entries: found.names.unwrap_or_default(),
                next_cursor: found.next_cursor.map(|cursor| format!("live:{cursor}")),
                source: orch_api_contract::forks::FilesSource::Live,
                as_of: found.as_of.unwrap_or(Time::new(ctx.now)),
                observed_from: found.observed_from,
                searched: found.searched,
            },
        )),
        FileSearchIn::Content => Ok(EnvironmentFileSearchReply::Content(FileSearchContent {
            matches: found.matches,
            truncated: found.truncated,
            searched: found.searched,
            next_cursor: found.next_cursor,
            observed_from: found.observed_from,
        })),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedNames {
    environment: orch_types::Id,
    generation: i32,
    manifest: orch_types::Hash,
    selection: String,
    after: String,
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
    let policy = orch_engine::environments::workspace_limits(&mut conn, door.env.id)
        .await
        .map_err(ApiError::internal)?;
    let limit = query
        .limit
        .unwrap_or(u32::try_from(policy.archive_results).unwrap_or(u32::MAX));
    if limit == 0 {
        return Err(ApiError::invalid("limit", "must be at least 1"));
    }
    let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
    drop(conn);
    let found = live::run(
        &ctx,
        machine,
        SerialKey::None,
        &FsArchiveList {
            cursor: query.cursor.clone(),
            query: query.q.clone().filter(|query| !query.is_empty()),
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
/// one page continues through the same native process, without rerunning its prefix.
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
            cursor: query.cursor.clone(),
            max_bytes: query.max_bytes,
            environment_id: door.env.id,
            path,
            staged: query.staged,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Repository))?;
    let next_cursor = found.next_cursor;
    let text = found.text;
    let truncated = found.truncated;
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
    if let Some(cursor) = next_cursor {
        headers.insert(
            http::header::HeaderName::from_static("x-next-cursor"),
            HeaderValue::from_str(&cursor).map_err(ApiError::internal)?,
        );
    }
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
