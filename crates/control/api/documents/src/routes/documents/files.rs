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

use super::limits;

use super::limits::DocSettings;

use super::limits::Limit;

use super::live;

use crate::archives;

use crate::archives::ArchiveError;

use crate::attachments;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::workspace::files as workspace_files;

use axum::body::Body;

use axum::response::Response;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::documents::DocumentFileArchiveEntryGet;

use orch_api_contract::documents::DocumentFileArchiveEntryGetQuery;

use orch_api_contract::documents::DocumentFileArchiveList;

use orch_api_contract::documents::DocumentFileArchiveListQuery;

use orch_api_contract::documents::DocumentFileBlobBody;

use orch_api_contract::documents::DocumentFileCreate;

use orch_api_contract::documents::DocumentFileGet;

use orch_api_contract::documents::DocumentFileGetQuery;

use orch_api_contract::workspace::ARCHIVE_LIST_LIMIT_DEFAULT;

use orch_api_contract::workspace::ARCHIVE_LIST_LIMIT_MAX;

use orch_api_contract::workspace::ArchiveListing;

use orch_blob_contract::range::BlobMeta;

use orch_blob_contract::range::ResolvedRange;

use orch_blob_contract::store::BlobRead;

use orch_blob_contract::store::stream_of;

use orch_db::queries::documents as rows;

use orch_db::queries::documents::HeldFile;

use orch_db::queries::storage;

use orch_types::Hash;

use orch_types::Id;

pub(super) use creation::projection::FileWitness;
/// The kinds of file a document shows where it is read: pictures, films and sounds. Everything
/// else it offers as a download.
const SHOWN_INLINE: [&str; 3] = ["image", "video", "audio"];

/// Binds this module's requests.
pub(super) fn mount(registry: &mut Registry) {
    registry.creation(DocumentFileCreate, creation::raw_creation());
    registry
        .on_raw_staged_operation::<DocumentFileCreate, creation::Files, _, _>(
            DocumentFileCreate,
            creation::create,
        )
        .on_raw_read_with_operation(DocumentFileGet, read_operation::prepare, get)
        .on_with_operation(
            DocumentFileArchiveList,
            read_operation::prepare,
            archive_list,
        )
        .on_raw_read_with_operation(
            DocumentFileArchiveEntryGet,
            read_operation::prepare,
            archive_entry,
        );
}

/// How a request that names a finished upload is read.
fn reference(bytes: &[u8]) -> Option<(Hash, String)> {
    serde_json::from_slice::<DocumentFileBlobBody>(bytes)
        .ok()
        .map(|body| (body.blob, body.name))
}

/// The file `file` of the document `document`, if the document holds it.
async fn held(ctx: &Ctx, document: Id, file: Id) -> ApiResult<HeldFile> {
    read_operation::current(ctx).await?;
    let mut conn = ctx.app.db.acquire().await?;
    live(&mut conn, document).await?;
    rows::file(&mut conn, document, file)
        .await?
        .ok_or_else(ApiError::not_found)
}

// ---------------------------------------------------------------- reading a file

/// Serves a file of a document: pictures, films and sounds inline, with ranges, and anything else
/// as a download, under the type it was kept with.
async fn get(ctx: Ctx, query: DocumentFileGetQuery, _: Body) -> ApiResult<Response> {
    let result = async {
        let (id, file) = (ctx.id("id")?, ctx.id("file_id")?);
        let held = held(&ctx, id, file).await?;
        {
            let mut conn = ctx.app.db.acquire().await?;
            storage::touch_blob(&mut conn, &held.blob, ctx.now).await?;
        }
        read_operation::current(&ctx).await?;
        if query.preview == Some(true) {
            return crate::previews::serve(
                &ctx,
                &held.blob,
                &held.file.name,
                &held.file.media_type,
            )
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
    .await;
    read_operation::response(ctx, result).await
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
    let result = async {
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
        read_operation::current(&ctx).await?;
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
    .await;
    read_operation::current(&ctx).await?;
    result
}

/// One entry of an archive that the document holds, for a viewer: at most as much as a preview shows.
async fn archive_entry(
    ctx: Ctx,
    query: DocumentFileArchiveEntryGetQuery,
    _: Body,
) -> ApiResult<Response> {
    let result = async {
        let (id, file) = (ctx.id("id")?, ctx.id("file_id")?);
        let entry = query.entry.trim_matches('/').to_owned();
        if entry.is_empty() || entry.split('/').any(|part| part == ".." || part.is_empty()) {
            return Err(ApiError::invalid(
                "entry",
                "this is the name of a file inside the archive",
            ));
        }
        let held = held(&ctx, id, file).await?;
        read_operation::current(&ctx).await?;
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
    .await;
    read_operation::response(ctx, result).await
}

#[path = "files/creation.rs"]
pub mod creation;

#[path = "files/read_operation.rs"]
pub mod read_operation;
