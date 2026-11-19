//! What machines upload to and download from the store of content over the internal listener: pieces that go on where they stopped, checked against their hash, and only for commands that are open.
//!
//! A machine moves content beside its session, to the same host, and its gateway hands each request
//! to the routes here with the machine it found the request to be from in a header only a gateway can
//! send. Nothing is moved for its own sake: every request names a command that is open for that
//! machine. An upload is taken only for a kind of command that uploads and only up to the largest
//! file the system stores; a download is served only for content that the command itself lists
//! (and, for a command that writes a tree down, the files its list names); at most four transfers
//! run for a machine at once. The two kinds of content that a machine makes on its own account, the
//! list of its tree and what a shell printed, name the nil command, which is taken from a machine
//! that has an environment and for uploads only.
//!
//! **A piece is the next piece.** An upload has a row (`blob_uploads`) that says how many bytes
//! have arrived, and a file where they wait. A piece that begins anywhere else is refused with the
//! number that has arrived, which is how a machine whose answer was lost finds out where to go on.
//! The last piece completes the upload: the file is put in the store, which checks it against the
//! hash and the size it was announced with, and an upload whose content is not what its hash says
//! is thrown away with a refusal the machine acts on.

use crate::certificates::LinkFault;
use crate::server::Peer;
use crate::state::App;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Extension, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use orch_blob_contract::range::{ByteRange, unsatisfied_content_range};
use orch_blob_contract::store::BlobError;
use orch_db::queries::resources::MachineRow;
use orch_db::queries::{link as commands, resources as machines, storage};
use orch_link_contract::commands::CommandKind;
use orch_link_contract::content::{
    BlobProgress, COMMAND_HEADER, COMPLETE_HEADER, ChunkRefusal, HASH_MISMATCH, MAX_CHUNK_BYTES,
    MAX_MISSING_HASHES, MissingReply, MissingRequest, PutQuery, RECEIVED_HEADER,
    STATUS_HASH_MISMATCH, STATUS_OFFSET_CONFLICT, accept_chunk, listed_hashes, serves_downloads,
    takes_uploads,
};
use orch_pki::certificates::ServiceName;
use orch_settings::Place;
use orch_types::{Hash, Id, Json};

/// The header in which a gateway gives the machine it found a request to be from.
pub const MACHINE_HEADER: &str = "x-orch-machine";

/// The most bytes of a question which content is missing: a thousand hashes of seventy characters,
/// and room.
const MISSING_BODY_BYTES: usize = 256 * 1024;

/// How large a file may be when no setting says: eight gibibytes, as shipped.
pub const DEFAULT_MAX_BLOB_BYTES: i64 = 8 * 1024 * 1024 * 1024;

/// The setting that says how large a file may be.
pub const MAX_BLOB_KEY: &str = "storage.max_blob_bytes";

/// The role a machine has to have for the content it makes on its own account.
const ENVIRONMENT_ROLE: &str = "environment";

/// The routes of the internal listener that serve content.
pub fn routes() -> Router<App> {
    Router::new()
        .route("/internal/link/blobs/missing", post(missing))
        .route(
            "/internal/link/blobs/{hash}",
            get(download).head(probe).put(upload),
        )
}

// ---------------------------------------------------------------- who asks, and what for

/// The command a request names.
enum Named {
    /// The nil command: content a machine makes on its own account.
    Standing,
    /// A command that is open for the machine.
    Open { kind: CommandKind, body: Json },
}

/// A request, as far as who made it and under which command.
struct Asker {
    machine: MachineRow,
    command: Named,
    command_id: Id,
}

/// What a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wants {
    /// To learn whether content is there and how much of it.
    Probe,
    /// To put content there.
    Upload,
    /// To take content from there.
    Download,
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

async fn asker(app: &App, peer: &Peer, headers: &HeaderMap) -> Result<Asker, LinkFault> {
    peer.require(ServiceName::Gateway)
        .map_err(|_| LinkFault::forbidden("only a gateway hands over content"))?;
    let machine: Id = header_text(headers, MACHINE_HEADER)
        .ok_or_else(|| LinkFault::new(401, "auth.required", "show who you are"))?
        .parse()
        .map_err(|_| LinkFault::invalid("the machine is not an identifier"))?;
    let command: Id = header_text(headers, COMMAND_HEADER)
        .ok_or_else(|| LinkFault::invalid("a request for content names its command"))?
        .parse()
        .map_err(|_| LinkFault::invalid("the command is not an identifier"))?;
    let mut conn = app
        .db
        .acquire()
        .await
        .map_err(|e| LinkFault::internal(&e))?;
    let machine = machines::machine(&mut conn, machine)
        .await
        .map_err(|e| LinkFault::internal(&e))?
        .filter(MachineRow::is_active)
        .ok_or_else(|| LinkFault::forbidden("the system does not let this machine in"))?;
    if command.is_nil() {
        return Ok(Asker {
            machine,
            command: Named::Standing,
            command_id: command,
        });
    }
    let row = commands::command(&mut conn, command)
        .await
        .map_err(|e| LinkFault::internal(&e))?
        .filter(|row| row.machine_id == machine.id)
        .ok_or_else(|| LinkFault::forbidden("that is no command of this machine"))?;
    if row.is_over() {
        return Err(LinkFault::new(410, "gone", "that command is over"));
    }
    let kind: CommandKind = row
        .kind
        .parse()
        .map_err(|_| LinkFault::forbidden("that command is of no kind this knows"))?;
    Ok(Asker {
        machine,
        command: Named::Open {
            kind,
            body: row.body,
        },
        command_id: command,
    })
}

/// Whether a kind of command uploads: the kinds the catalogue marks, and the one that fetches one
/// entry of an archive for a viewer, which the catalogue leaves unmarked and whose answer is a blob
/// that machine has to put up.
fn uploads(kind: CommandKind) -> bool {
    takes_uploads(kind) || kind == CommandKind::FsArchiveRead
}

impl Asker {
    /// Whether the command lets the machine do `wants`; content for a download is judged apart
    /// ([`Asker::lists`]).
    fn permits(&self, wants: Wants) -> bool {
        match (&self.command, wants) {
            (Named::Standing, Wants::Probe | Wants::Upload) => self
                .machine
                .roles
                .iter()
                .any(|role| role == ENVIRONMENT_ROLE),
            (Named::Standing, Wants::Download) => false,
            (Named::Open { kind, .. }, Wants::Upload) => uploads(*kind),
            (Named::Open { kind, .. }, Wants::Download) => serves_downloads(*kind),
            (Named::Open { kind, .. }, Wants::Probe) => uploads(*kind) || serves_downloads(*kind),
        }
    }

    /// Whether the command lists the content `hash`: in its body, or, for a command that writes a
    /// tree down, in the list of that tree.
    async fn lists(&self, app: &App, hash: &Hash) -> Result<bool, LinkFault> {
        let Named::Open { kind, body } = &self.command else {
            return Ok(false);
        };
        if listed_hashes(body).contains(hash) {
            return Ok(true);
        }
        if *kind != CommandKind::FsMaterialize {
            return Ok(false);
        }
        let Some(manifest) = body
            .get("manifest_blob")
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Hash>().ok())
        else {
            return Ok(false);
        };
        let files = app
            .blobs
            .files_of_manifest(&manifest, crate::cache::MANIFEST_MAX_STORED_BYTES)
            .await
            .map_err(|e| LinkFault::internal(&e))?;
        Ok(files.contains(hash))
    }
}

fn denied(wants: Wants) -> LinkFault {
    let detail = match wants {
        Wants::Probe => "that command does not move content",
        Wants::Upload => "that command takes no content",
        Wants::Download => "that command is not given this content",
    };
    LinkFault::forbidden(detail)
}

fn hash_of(text: &str) -> Result<Hash, LinkFault> {
    text.parse()
        .map_err(|_| LinkFault::invalid("that is no hash of content"))
}

/// The most bytes the system stores in one file.
async fn max_blob_bytes(app: &App) -> Result<i64, LinkFault> {
    let mut conn = app
        .db
        .acquire()
        .await
        .map_err(|e| LinkFault::internal(&e))?;
    Ok(
        orch_settings::get::<i64>(&mut conn, Place::System, MAX_BLOB_KEY)
            .await
            .ok()
            .flatten()
            .unwrap_or(DEFAULT_MAX_BLOB_BYTES),
    )
}

// ---------------------------------------------------------------- answers

fn progress_headers(progress: BlobProgress) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&progress.received.to_string()) {
        headers.insert(HeaderName::from_static(RECEIVED_HEADER), value);
    }
    headers.insert(
        HeaderName::from_static(COMPLETE_HEADER),
        HeaderValue::from_static(progress.complete_header()),
    );
    headers
}

fn progress_reply(status: StatusCode, progress: BlobProgress) -> Response {
    let body = serde_json::to_vec(&progress).unwrap_or_default();
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response.headers_mut().extend(progress_headers(progress));
    response
}

fn json_reply<T: serde::Serialize>(value: &T) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn refused_piece(refusal: ChunkRefusal) -> Response {
    match refusal {
        ChunkRefusal::Offset { received } => {
            let status =
                StatusCode::from_u16(STATUS_OFFSET_CONFLICT).unwrap_or(StatusCode::CONFLICT);
            progress_reply(
                status,
                BlobProgress {
                    received,
                    complete: false,
                },
            )
        }
        ChunkRefusal::ChunkTooLarge | ChunkRefusal::BlobTooLarge => {
            LinkFault::new(413, "request.too_large", refusal.to_string()).into_response()
        }
        ChunkRefusal::PastEnd => LinkFault::invalid(refusal.to_string()).into_response(),
    }
}

// ---------------------------------------------------------------- HEAD

async fn probe(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    Path(hash): Path<String>,
    headers: HeaderMap,
) -> Result<Response, LinkFault> {
    let asker = asker(&app, &peer, &headers).await?;
    let hash = hash_of(&hash)?;
    if !asker.permits(Wants::Probe) {
        return Err(denied(Wants::Probe));
    }
    if let Some(size) = app
        .blobs
        .store()
        .size(&hash)
        .await
        .map_err(|e| LinkFault::internal(&e))?
    {
        return Ok(progress_reply(
            StatusCode::OK,
            BlobProgress {
                received: size,
                complete: true,
            },
        ));
    }
    let mut conn = app
        .db
        .acquire()
        .await
        .map_err(|e| LinkFault::internal(&e))?;
    match storage::upload(&mut conn, &hash)
        .await
        .map_err(|e| LinkFault::internal(&e))?
    {
        Some(row) => Ok(progress_reply(
            StatusCode::OK,
            BlobProgress {
                received: row.received,
                complete: false,
            },
        )),
        None => Err(LinkFault::new(
            404,
            "not_found",
            "the system holds none of that",
        )),
    }
}

// ---------------------------------------------------------------- POST missing

async fn missing(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, LinkFault> {
    let asker = asker(&app, &peer, &headers).await?;
    if !asker.permits(Wants::Upload) {
        return Err(denied(Wants::Upload));
    }
    let bytes = to_bytes(body, MISSING_BODY_BYTES)
        .await
        .map_err(|_| LinkFault::new(413, "request.too_large", "that question is too long"))?;
    let asked: MissingRequest = serde_json::from_slice(&bytes)
        .map_err(|error| LinkFault::invalid(format!("the question cannot be read: {error}")))?;
    if asked.hashes.len() > MAX_MISSING_HASHES {
        return Err(LinkFault::invalid(format!(
            "at most {MAX_MISSING_HASHES} hashes are asked about at once"
        )));
    }
    let mut conn = app
        .db
        .acquire()
        .await
        .map_err(|e| LinkFault::internal(&e))?;
    let absent = app
        .blobs
        .missing(&mut conn, &asked.hashes, app.clock.now())
        .await
        .map_err(|e| LinkFault::internal(&e))?;
    Ok(json_reply(&MissingReply { missing: absent }))
}

// ---------------------------------------------------------------- GET

async fn download(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    Path(hash): Path<String>,
    headers: HeaderMap,
) -> Result<Response, LinkFault> {
    let asker = asker(&app, &peer, &headers).await?;
    let hash = hash_of(&hash)?;
    if !asker.permits(Wants::Download) || !asker.lists(&app, &hash).await? {
        return Err(denied(Wants::Download));
    }
    let transfer = app
        .blobs
        .begin_transfer(asker.machine.id)
        .map_err(|error| LinkFault::new(429, "rate_limited", error.to_string()))?;
    let range = header_text(&headers, header::RANGE.as_str()).and_then(ByteRange::from_header);
    let read = match app.blobs.read(&hash, range).await {
        Ok(read) => read,
        Err(BlobError::NotFound(_)) => {
            return Err(LinkFault::new(
                404,
                "not_found",
                "the system holds none of that",
            ));
        }
        Err(BlobError::RangeNotSatisfiable { size }) => {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            if let Ok(value) = HeaderValue::from_str(&unsatisfied_content_range(size)) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            return Ok(response);
        }
        Err(error) => return Err(LinkFault::internal(&error)),
    };
    if let Ok(mut conn) = app.db.acquire().await
        && let Err(error) = storage::touch_blob(&mut conn, &hash, app.clock.now()).await
    {
        tracing::debug!(%error, "the use of a blob could not be recorded");
    }
    let partial = range.is_some() && !read.range.is_whole(read.meta.size);
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(value) = HeaderValue::from_str(&read.range.len.to_string()) {
        response_headers.insert(header::CONTENT_LENGTH, value);
    }
    if partial && let Ok(value) = HeaderValue::from_str(&read.range.content_range(read.meta.size)) {
        response_headers.insert(header::CONTENT_RANGE, value);
    }
    // The transfer stays counted until the last byte has been handed on.
    let body = Body::from_stream(read.body.map(move |piece| {
        let _counted = &transfer;
        piece.map_err(std::io::Error::other)
    }));
    let status = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut response = (status, body).into_response();
    response.headers_mut().extend(response_headers);
    Ok(response)
}

// ---------------------------------------------------------------- PUT

/// Why a piece could not be taken, other than being refused by the rules of the protocol.
fn failed(error: &dyn std::fmt::Display) -> LinkFault {
    LinkFault::internal(error)
}

async fn upload(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    Path(hash): Path<String>,
    Query(query): Query<PutQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, LinkFault> {
    let asker = asker(&app, &peer, &headers).await?;
    let hash = hash_of(&hash)?;
    if !asker.permits(Wants::Upload) {
        return Err(denied(Wants::Upload));
    }
    let max = max_blob_bytes(&app).await?;
    let _transfer = app
        .blobs
        .begin_transfer(asker.machine.id)
        .map_err(|error| LinkFault::new(429, "rate_limited", error.to_string()))?;
    // Content the store holds already needs nothing more, whatever piece this is.
    if let Some(size) = app
        .blobs
        .store()
        .size(&hash)
        .await
        .map_err(|e| failed(&e))?
    {
        let mut conn = app.db.acquire().await.map_err(|e| failed(&e))?;
        app.blobs
            .register(
                &mut conn,
                orch_blob_contract::range::BlobMeta { hash, size },
                app.clock.now(),
            )
            .await
            .map_err(|e| failed(&e))?;
        return Ok(progress_reply(
            StatusCode::OK,
            BlobProgress {
                received: size,
                complete: true,
            },
        ));
    }
    let piece = to_bytes(
        body,
        usize::try_from(MAX_CHUNK_BYTES).unwrap_or(usize::MAX) + 1,
    )
    .await
    .map_err(|_| {
        LinkFault::new(
            413,
            "request.too_large",
            "a piece carries at most four mebibytes",
        )
    })?;
    let len = i64::try_from(piece.len()).unwrap_or(i64::MAX);
    let now = app.clock.now();
    let mut tx = app.db.begin().await.map_err(|e| failed(&e))?;
    let mut row = storage::lock_upload(&mut tx, &hash)
        .await
        .map_err(|e| failed(&e))?;
    if row.is_none() {
        // Only a first piece may begin an upload: any other is told where to begin.
        if query.offset != 0 {
            return Ok(refused_piece(ChunkRefusal::Offset { received: 0 }));
        }
        if query.size < 0 || query.size > max {
            return Ok(refused_piece(ChunkRefusal::BlobTooLarge));
        }
        storage::begin_upload(&mut tx, &hash, query.size, Some(asker.machine.id), now)
            .await
            .map_err(|e| failed(&e))?;
        row = storage::lock_upload(&mut tx, &hash)
            .await
            .map_err(|e| failed(&e))?;
    }
    let Some(row) = row else {
        return Err(failed(&"an upload that was just begun is not there"));
    };
    if row.machine_id != Some(asker.machine.id) {
        return Err(LinkFault::forbidden(
            "that unfinished upload belongs to another sender",
        ));
    }
    if row.size != query.size {
        return Err(LinkFault::invalid(
            "the size is not the one this content was announced with",
        ));
    }
    let progress = match accept_chunk(row.received, query, len, max) {
        Ok(progress) => progress,
        Err(refusal) => return Ok(refused_piece(refusal)),
    };
    app.blobs.make_uploads_dir().await.map_err(|e| failed(&e))?;
    let part = app.blobs.part_of(&hash);
    crate::blobs::write_piece(&part, row.received, &piece)
        .await
        .map_err(|e| failed(&e))?;
    storage::advance_upload(
        &mut tx,
        &hash,
        progress.received,
        (!asker.command_id.is_nil()).then_some(asker.command_id),
        now,
    )
    .await
    .map_err(|e| failed(&e))?;
    if !progress.complete {
        tx.commit().await.map_err(|e| failed(&e))?;
        return Ok(progress_reply(StatusCode::OK, progress));
    }
    // The last piece: the whole is checked against the hash and the size it was announced with.
    match app.blobs.put_file(&part, hash, row.size).await {
        Ok(stored) => {
            app.blobs
                .register(&mut tx, stored.meta, now)
                .await
                .map_err(|e| failed(&e))?;
            storage::end_upload(&mut tx, &hash)
                .await
                .map_err(|e| failed(&e))?;
            tx.commit().await.map_err(|e| failed(&e))?;
            crate::blobs::discard(&part).await;
            Ok(progress_reply(StatusCode::OK, progress))
        }
        Err(crate::blobs::BlobsError::Store(
            BlobError::HashMismatch { .. } | BlobError::SizeMismatch { .. },
        )) => {
            storage::end_upload(&mut tx, &hash)
                .await
                .map_err(|e| failed(&e))?;
            tx.commit().await.map_err(|e| failed(&e))?;
            crate::blobs::discard(&part).await;
            Ok(LinkFault::new(
                STATUS_HASH_MISMATCH,
                HASH_MISMATCH,
                "the content is not the content its hash names; what arrived was thrown away",
            )
            .into_response())
        }
        Err(error) => Err(failed(&error)),
    }
}
