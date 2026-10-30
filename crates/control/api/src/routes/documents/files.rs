//! The files a document holds: put in from a body or from a finished upload, given back to whoever reads the document, and what an archive among them contains.
//!
//! Who may put a file in or read one is the document's page, asked before the handler runs
//! (`doc.edit`, `doc.view`), so a file follows the document: when the page changes, so does who
//! reads its files, and a file is found only among the files of the document whose identifier is in
//! the path. What a document may hold is the settings of the node it lies beside: one file at most
//! `docs.file_max_bytes`, all of them `docs.files_max_bytes`, and a refusal names the limit and the
//! node that sets it. A file is a blob of the store, pinned for as long as the document holds it;
//! a file put in under a name the document has already replaces that one only when asked to, and
//! keeps its identifier, so that the blocks that name it show the new bytes.
//!
//! Pictures, films and sounds are shown inline, with ranges; everything else is a download, under
//! the type it was kept with. An archive is looked into where it lies in the store: its listing one
//! directory at a time and one of its entries, up to the size of a preview.

use super::build::{Cast, Wanted};
use super::limits::{self, DocSettings, Limit};
use super::{live, live_locked};
use crate::archives::{self, ArchiveError};
use crate::attachments::{self, Refused};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::workspace::{files as workspace_files, json_reply};
use axum::body::Body;
use axum::response::Response;
use http::StatusCode;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentFile, DocumentFileArchiveEntryGet, DocumentFileArchiveEntryGetQuery,
    DocumentFileArchiveList, DocumentFileArchiveListQuery, DocumentFileBlobBody,
    DocumentFileChange, DocumentFileChangeKind, DocumentFileCreate, DocumentFileCreateQuery,
    DocumentFileGet, DocumentFileGetQuery,
};
use orch_api_contract::workspace::{
    ARCHIVE_LIST_LIMIT_DEFAULT, ARCHIVE_LIST_LIMIT_MAX, ArchiveListing,
};
use orch_authz::Policy;
use orch_blob_contract::range::{BlobMeta, ResolvedRange};
use orch_blob_contract::store::{BlobRead, stream_of};
use orch_db::DbError;
use orch_db::queries::documents::{self as rows, HeldFile, NewFile};
use orch_db::queries::storage;
use orch_documents::versions::Writer;
use orch_types::{Hash, Id, Time, new_id};

/// The kinds of file a document shows where it is read: pictures, films and sounds. Everything
/// else it offers as a download.
const SHOWN_INLINE: [&str; 3] = ["image", "video", "audio"];

/// Binds this module's requests.
pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_raw(DocumentFileCreate, create)
        .on_raw(DocumentFileGet, get)
        .on(DocumentFileArchiveList, archive_list)
        .on_raw(DocumentFileArchiveEntryGet, archive_entry);
}

/// How a request that names a finished upload is read.
fn reference(bytes: &[u8]) -> Option<(Hash, String)> {
    serde_json::from_slice::<DocumentFileBlobBody>(bytes)
        .ok()
        .map(|body| (body.blob, body.name))
}

/// The file `file` of the document `document`, if the document holds it.
async fn held(ctx: &Ctx, document: Id, file: Id) -> ApiResult<HeldFile> {
    let mut conn = ctx.app.db.acquire().await?;
    live(&mut conn, document).await?;
    rows::file(&mut conn, document, file)
        .await?
        .ok_or_else(ApiError::not_found)
}

// ---------------------------------------------------------------- putting a file in

/// Puts a file into a document, from a body that has the file and the header that names it, or from
/// the hash of an upload that was ended.
async fn create(mut ctx: Ctx, query: DocumentFileCreateQuery, body: Body) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    let policy = Policy::default();
    // What the place allows is read before a byte is, and nothing is held while the body arrives.
    let allowed = {
        let mut conn = ctx.app.db.acquire().await?;
        let row = live(&mut conn, id).await?;
        DocSettings::at(&mut conn, row.node_id).await?
    };
    let taken = match attachments::take(&ctx, body, allowed.file_max_bytes.value, reference).await {
        Ok(taken) => taken,
        Err(Refused::Other(error)) => return Err(error),
        Err(Refused::TooLarge {
            limit,
            by_request: true,
        }) => return Err(attachments::over_a_request(limit)),
        Err(Refused::TooLarge { .. }) => {
            let mut conn = ctx.app.db.acquire().await?;
            let principal = authorization::principal(&ctx, &mut conn).await?;
            return Err(limits::too_large(
                &mut conn,
                &principal,
                &allowed,
                Limit::FileBytes,
                policy,
            )
            .await);
        }
    };
    let described = attachments::describe(&ctx, &taken.meta, &taken.name).await?;

    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let row = live_locked(&mut tx, id).await?;
    let settings = DocSettings::at(&mut tx, row.node_id).await?;
    let same_name = rows::file_named(&mut tx, id, &taken.name).await?;
    let replaced = match same_name {
        Some(found) if query.overwrite.unwrap_or(false) => Some(found),
        Some(found) => {
            return Err(ApiError::new(
                ErrorCode::ConflictExists,
                "the document has a file of that name; ask to overwrite it to replace it",
            )
            .with("existing_id", found.file.id));
        }
        None => None,
    };
    if taken.meta.size > settings.file_max_bytes.value {
        return Err(
            limits::too_large(&mut tx, &principal, &settings, Limit::FileBytes, policy).await,
        );
    }
    let others = row.files_bytes - replaced.as_ref().map_or(0, |found| found.file.size);
    if others.saturating_add(taken.meta.size) > settings.files_max_bytes.value {
        return Err(
            limits::too_large(&mut tx, &principal, &settings, Limit::FilesBytes, policy).await,
        );
    }
    attachments::retain(&ctx, &mut tx, &principal, &taken).await?;
    // The new bytes are pinned before the old are let go: the same bytes put in again stay pinned.
    ctx.app.blobs.pin(&mut tx, &[taken.meta.hash]).await?;
    let file = NewFile {
        id: replaced.as_ref().map_or_else(new_id, |found| found.file.id),
        document: id,
        blob: taken.meta.hash,
        name: &taken.name,
        media_type: &described.media_type,
        size: taken.meta.size,
        is_text: described.is_text,
        added_by: Some(principal.user()),
        added_by_thread: None,
        at: ctx.now,
    };
    if let Some(old) = &replaced {
        rows::replace_file(&mut tx, &file).await?;
        ctx.app.blobs.unpin(&mut tx, &[old.blob], ctx.now).await?;
    } else {
        rows::add_file(&mut tx, &file).await?;
    }
    rows::history::record_file(
        &mut tx,
        id,
        &Writer::User(principal.user()),
        DocumentFileChange {
            name: taken.name.clone(),
            kind: if replaced.is_some() {
                DocumentFileChangeKind::Replaced
            } else {
                DocumentFileChangeKind::Added
            },
        },
        settings.version_rules(ctx.app.config.documents.version_window_secs),
        ctx.now,
    )
    .await?;
    // The document is stamped without a change to its text, so that every open copy lists the file.
    rows::touch(&mut tx, id).await?;
    let mut wanted = Wanted::default();
    wanted.user(principal.user());
    let cast = Cast::load(&mut tx, &principal, wanted, policy).await?;
    tx.commit().await.map_err(DbError::from)?;
    json_reply(
        StatusCode::OK,
        &DocumentFile {
            id: file.id,
            name: taken.name.clone(),
            media_type: described.media_type.clone(),
            size: taken.meta.size,
            is_text: described.is_text,
            added_by: cast.author(Some(principal.user()), None, None),
            added_at: Time::new(ctx.now),
        },
    )
}

// ---------------------------------------------------------------- reading a file

/// Serves a file of a document: pictures, films and sounds inline, with ranges, and anything else
/// as a download, under the type it was kept with.
async fn get(ctx: Ctx, query: DocumentFileGetQuery, _: Body) -> ApiResult<Response> {
    let (id, file) = (ctx.id("id")?, ctx.id("file_id")?);
    let held = held(&ctx, id, file).await?;
    {
        let mut conn = ctx.app.db.acquire().await?;
        storage::touch_blob(&mut conn, &held.blob, ctx.now).await?;
    }
    if query.preview == Some(true) {
        return crate::previews::serve(&ctx, &held.blob, &held.file.name, &held.file.media_type)
            .await;
    }
    attachments::serve_kept(
        &ctx,
        &held.blob,
        &held.file.name,
        &held.file.media_type,
        &SHOWN_INLINE,
        query.download.unwrap_or(false),
    )
    .await
}

// ---------------------------------------------------------------- archives

/// What went wrong in looking into an archive, as the problem a client acts on.
fn archive_problem(error: ArchiveError) -> ApiError {
    match &error {
        ArchiveError::NotFound(_) => ApiError::not_found(),
        ArchiveError::TooLarge { limit, .. } => {
            ApiError::new(ErrorCode::RequestTooLarge, error.to_string()).with("limit", limit)
        }
        ArchiveError::Failed(cause) => ApiError::internal(cause),
        ArchiveError::NotAnArchive
        | ArchiveError::Unreadable(_)
        | ArchiveError::NotAFile(_)
        | ArchiveError::Encrypted(_)
        | ArchiveError::Timeout => {
            ApiError::new(ErrorCode::CapabilityUnsupported, error.to_string())
        }
    }
}

/// The blob a file is, as the archive reader asks for it.
fn meta_of(held: &HeldFile) -> BlobMeta {
    BlobMeta {
        hash: held.blob,
        size: held.file.size,
    }
}

/// What an archive that the document holds contains, one directory at a time.
async fn archive_list(
    ctx: Ctx,
    query: DocumentFileArchiveListQuery,
    (): (),
) -> ApiResult<ArchiveListing> {
    let (id, file) = (ctx.id("id")?, ctx.id("file_id")?);
    let held = held(&ctx, id, file).await?;
    let limit = match query.limit {
        None => ARCHIVE_LIST_LIMIT_DEFAULT,
        Some(0) => return Err(ApiError::invalid("limit", "must be at least 1")),
        Some(limit) => limit.min(ARCHIVE_LIST_LIMIT_MAX),
    };
    let inside = query
        .inside
        .as_deref()
        .map(|inside| inside.trim_matches('/'))
        .filter(|inside| !inside.is_empty());
    archives::list(
        &ctx.app.blobs,
        meta_of(&held),
        inside,
        usize::try_from(limit).unwrap_or(usize::MAX),
        archives::READ_TIME,
    )
    .await
    .map_err(archive_problem)
}

/// One entry of an archive that the document holds, for a viewer: at most as much as a preview shows.
async fn archive_entry(
    ctx: Ctx,
    query: DocumentFileArchiveEntryGetQuery,
    _: Body,
) -> ApiResult<Response> {
    let (id, file) = (ctx.id("id")?, ctx.id("file_id")?);
    let entry = query.entry.trim_matches('/').to_owned();
    if entry.is_empty() || entry.split('/').any(|part| part == ".." || part.is_empty()) {
        return Err(ApiError::invalid(
            "entry",
            "this is the name of a file inside the archive",
        ));
    }
    let held = held(&ctx, id, file).await?;
    let bytes = archives::read_entry(
        &ctx.app.blobs,
        meta_of(&held),
        &entry,
        ctx.app.config.limits.preview_bytes,
        archives::READ_TIME,
    )
    .await
    .map_err(archive_problem)?;
    let name = entry.rsplit('/').next().unwrap_or(&entry);
    let head = &bytes[..bytes.len().min(crate::media::SNIFF_BYTES)];
    let media = crate::media::media_type(name, head);
    let hash = Hash::of(&bytes);
    let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let read = BlobRead {
        meta: BlobMeta { hash, size },
        range: ResolvedRange::whole(size),
        body: stream_of(vec![bytes::Bytes::from(bytes)]),
    };
    // The entry is a few bytes that were unpacked for this answer alone: it is served whole.
    Ok(workspace_files::serve(
        read,
        false,
        &ctx.headers,
        &hash,
        name,
        media,
        false,
    ))
}
