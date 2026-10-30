//! Files that people put into chats and documents: taken into the store from a body or from a finished upload, told apart by name and first bytes, and given back under the name and the type they were kept with.
//!
//! A file arrives in one of two ways, as it does for a thread: its bytes are the body of the request
//! and its name is in the header, or it was sent in pieces as an upload that has ended and the body
//! names the hash of what arrived and a name. A body is stored as it comes, and one that is larger
//! than a request may carry, or than the place allows, leaves nothing; a finished upload is larger
//! than the place allows when the store says so. What a file is comes from its name and its first
//! bytes ([`crate::media`]); the type a sender claims is never taken for the type the file has.
//!
//! A file is given back by the request of the place that holds it, after that place has said that
//! the caller may read it ([`serve_kept`]). What a picture, a film or a sound is shown as in the page
//! that asked is a matter of the place: the file is sent inline only if the place says its kind is
//! one it shows, and as a download otherwise. Inert media keep their type so a film sent as a
//! download still plays where a page has it as a source. Active document types are octet-stream
//! downloads and are never sent as executable pages, whatever their place says.

use crate::blobs::BlobsError;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::routes::workspace::files;
use crate::uploads;
use axum::body::Body;
use axum::response::Response;
use http::{HeaderValue, StatusCode, header};
use orch_api_contract::common::{ErrorCode, MAX_BODY_BYTES, MAX_JSON_BYTES};
use orch_api_contract::threads::FILENAME_HEADER;
use orch_authz::Principal;
use orch_blob_contract::range::{BlobMeta, ByteRange};
use orch_blob_contract::store::read_all;
use orch_db::Conn;
use orch_db::queries::uploads as references;
use orch_types::Hash;

/// The most characters of a media type that is taken as written: what a header may hold of one.
const MEDIA_TYPE_MAX_CHARS: usize = 127;

/// A file the store holds because a request brought it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// The content and its size.
    pub meta: BlobMeta,
    /// The name the request gave it, fit to be kept.
    pub name: String,
    /// Whether the request supplied bytes rather than naming a saved reference.
    pub supplied_bytes: bool,
}

/// Why a file was not taken.
#[derive(Debug)]
pub enum Refused {
    /// It is larger than may be: `limit` is the figure that was met, and `by_request` says that
    /// it is the size a request carries (more goes in pieces) and not what the place allows.
    TooLarge {
        /// The most bytes that may be.
        limit: i64,
        /// Whether the limit is the one of a request.
        by_request: bool,
    },
    /// Anything else, as the problem a client acts on.
    Other(ApiError),
}

impl From<ApiError> for Refused {
    fn from(error: ApiError) -> Self {
        Self::Other(error)
    }
}

/// How a request that names a finished upload is read: the hash of what arrived and a name.
pub type Reference = fn(&[u8]) -> Option<(Hash, String)>;

/// The refusal of a body that is larger than a request carries.
#[must_use]
pub fn over_a_request(limit: i64) -> ApiError {
    ApiError::new(
        ErrorCode::RequestTooLarge,
        "a request carries at most that much; a larger file goes up in pieces",
    )
    .with("limit", limit)
}

/// Takes the file a request brings into the store: the body when the header gives a name, otherwise
/// the finished upload that the body names. `place_limit` is the most the place that is to hold it
/// allows. Nothing is recorded in the database; the caller does that in its own transaction.
pub async fn take(
    ctx: &Ctx,
    body: Body,
    place_limit: i64,
    reference: Reference,
) -> Result<Taken, Refused> {
    if let Some(name) = crate::routes::threads::uploads::filename_of(ctx) {
        let name = uploads::clean_name(FILENAME_HEADER, &name)?;
        let request_limit = i64::try_from(MAX_BODY_BYTES).unwrap_or(i64::MAX);
        let by_request = place_limit >= request_limit;
        let limit = place_limit.min(request_limit);
        return match ctx
            .app
            .blobs
            .store_stream(body.into_data_stream(), limit)
            .await
        {
            Ok(meta) => Ok(Taken {
                meta,
                name,
                supplied_bytes: true,
            }),
            Err(BlobsError::TooLarge { limit }) => Err(Refused::TooLarge { limit, by_request }),
            Err(other) => Err(uploads::stored_badly(other).into()),
        };
    }
    let bytes = axum::body::to_bytes(body, MAX_JSON_BYTES)
        .await
        .map_err(|_| ApiError::new(ErrorCode::RequestTooLarge, "the body is too large"))?;
    let (blob, name) = reference(&bytes).ok_or_else(|| {
        ApiError::invalid(
            "",
            "a file is sent as the body with its name in the header, or as the hash of an upload and a name",
        )
    })?;
    let name = uploads::clean_name("name", &name)?;
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    require(&mut tx, guard.principal(), &blob).await?;
    let size = ctx
        .app
        .blobs
        .store()
        .size(&blob)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    if size > place_limit {
        return Err(Refused::TooLarge {
            limit: place_limit,
            by_request: false,
        });
    }
    Ok(Taken {
        meta: BlobMeta { hash: blob, size },
        name,
        supplied_bytes: false,
    })
}

/// What a file is, as far as its name and its first bytes say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    /// Its media type.
    pub media_type: String,
    /// Whether its bytes are text, so that it can be read and searched where it lies.
    pub is_text: bool,
}

/// The first bytes of a blob that decide what it is.
async fn head_of(ctx: &Ctx, meta: &BlobMeta) -> ApiResult<Vec<u8>> {
    if meta.size <= 0 {
        return Ok(Vec::new());
    }
    let sniffed = i64::try_from(crate::media::SNIFF_BYTES).unwrap_or(i64::MAX);
    let read = ctx
        .app
        .blobs
        .read(
            &meta.hash,
            Some(ByteRange::Between {
                start: 0,
                end: meta.size.min(sniffed) - 1,
            }),
        )
        .await
        .map_err(ApiError::internal)?;
    let bytes = read_all(read.body).await.map_err(ApiError::internal)?;
    Ok(bytes.to_vec())
}

/// Says what the file `meta` called `name` is.
pub async fn describe(ctx: &Ctx, meta: &BlobMeta, name: &str) -> ApiResult<Described> {
    let head = head_of(ctx, meta).await?;
    Ok(Described {
        media_type: crate::media::media_type(name, &head).to_owned(),
        is_text: crate::media::is_text(&head),
    })
}

/// Retains the file and its authorized reference in the caller's guarded transaction.
pub async fn retain(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    taken: &Taken,
) -> ApiResult<()> {
    references::lock_blobs(conn, &[taken.meta.hash]).await?;
    if !taken.supplied_bytes {
        require(conn, principal, &taken.meta.hash).await?;
    }
    ctx.app
        .blobs
        .register(conn, taken.meta, principal.now().get())
        .await?;
    references::remember(
        conn,
        principal.user(),
        &taken.meta.hash,
        principal.now().get(),
    )
    .await?;
    Ok(())
}

/// Requires a caller's upload or a currently readable saved source before consulting stored bytes.
pub async fn require(conn: &mut Conn, principal: &Principal, hash: &Hash) -> ApiResult<()> {
    match orch_chat::following::require_blobs(conn, principal, &[*hash]).await {
        Ok(()) => Ok(()),
        Err(orch_chat::Error::NotFound) => Err(ApiError::not_found()),
        Err(error) => Err(ApiError::internal(error)),
    }
}

/// A media type as a header may carry it: a type and a subtype in lower case and nothing else
/// (no parameters), or, where the sender claimed anything else, the type the name says.
#[must_use]
pub fn plain_media_type(claimed: &str, name: &str) -> String {
    let claimed = claimed.trim().to_ascii_lowercase();
    let token = |text: &str| {
        !text.is_empty()
            && text.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '!' | '#' | '$' | '&' | '^' | '_' | '.' | '+' | '-')
            })
    };
    let fit = claimed.len() <= MEDIA_TYPE_MAX_CHARS
        && claimed
            .split_once('/')
            .is_some_and(|(kind, subtype)| token(kind) && token(subtype));
    if fit {
        claimed
    } else {
        crate::media::media_type(name, &[]).to_owned()
    }
}

/// Whether a file of this type is shown inline by a place that shows the kinds `inline`
/// (`"image"`, `"video"`, `"audio"`): never active content, whatever the place says.
#[must_use]
pub fn is_shown_inline(media_type: &str, inline: &[&str]) -> bool {
    let kind = media_type.split('/').next().unwrap_or_default();
    inline.contains(&kind) && crate::media::shows_inline(media_type)
}

/// Gives a file back by its hash: the whole of it or the range asked for, with its hash as its tag.
/// It is shown inline if `download` is not asked for and the place shows its kind, and is
/// otherwise offered as a download; inert media keep their type and active documents use octet-stream.
pub async fn serve_kept(
    ctx: &Ctx,
    hash: &Hash,
    name: &str,
    media_type: &str,
    inline: &[&str],
    download: bool,
) -> ApiResult<Response> {
    let shown = !download && is_shown_inline(media_type, inline);
    let mut response = files::read_for(ctx, hash, name, media_type, !shown).await?;
    if !shown
        && crate::media::shows_inline(media_type)
        && response.status() != StatusCode::NOT_MODIFIED
        && response.status() != StatusCode::RANGE_NOT_SATISFIABLE
        && let Ok(value) = HeaderValue::from_str(media_type)
    {
        // Inert media keep the type a player needs. Active document types remain octet-stream downloads.
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_type_is_kept_only_when_it_is_a_plain_type_and_subtype() {
        assert_eq!(
            plain_media_type("Application/PDF", "x.bin"),
            "application/pdf"
        );
        assert_eq!(
            plain_media_type("text/html; charset=utf-7", "notes.txt"),
            "text/plain"
        );
        assert_eq!(plain_media_type("", "photo.png"), "image/png");
        assert_eq!(plain_media_type("not a type", "data.csv"), "text/csv");
        assert_eq!(
            plain_media_type(&format!("a/{}", "b".repeat(200)), "n.zip"),
            "application/zip"
        );
    }

    #[test]
    fn only_the_kinds_a_place_shows_are_shown_and_never_active_content() {
        assert!(is_shown_inline("image/png", &["image"]));
        assert!(!is_shown_inline("video/mp4", &["image"]));
        assert!(is_shown_inline("video/mp4", &["image", "video", "audio"]));
        assert!(!is_shown_inline("image/svg+xml", &["image"]));
        assert!(!is_shown_inline(
            "application/pdf",
            &["image", "video", "audio"]
        ));
        assert!(!is_shown_inline("text/html", &["image", "video", "audio"]));
    }
}
