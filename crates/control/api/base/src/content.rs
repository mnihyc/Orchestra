//! What machines upload to and download from the store of content over the internal listener: pieces that go on where they stopped, checked against their hash, and only for commands that are open.
//!
//! A machine moves content beside its session, to the same host, and its gateway hands each request
//! to the routes here with the machine it found the request to be from in a header only a gateway can
//! send. Nothing is moved for its own sake: every request names a command that is open for that
//! machine. An upload is taken only for a kind of command that uploads and only up to the largest
//! file the system stores; a download is served only for content that the command itself lists
//! (and, for a command that writes a tree down, the files its list names); at most four transfers
//! run for a machine at once. Content a machine produces on its own account, including its tree,
//! shell outputs and retained agent outputs, names the nil command. Environment and agent roles
//! may probe and upload that content; neither may download on the standing account.
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

use axum::body::Body;

use axum::body::to_bytes;

use axum::extract::Extension;

use axum::extract::Path;

use axum::extract::Query;

use axum::extract::State;

use axum::response::IntoResponse;

use axum::response::Response;

use axum::routing::get;

use axum::routing::post;

use futures::StreamExt;

use http::HeaderMap;

use http::HeaderName;

use http::HeaderValue;

use http::StatusCode;

use http::header;

use orch_blob_contract::range::ByteRange;

use orch_blob_contract::range::unsatisfied_content_range;

use orch_blob_contract::store::BlobError;

use orch_db::queries::resources::MachineRow;

use orch_db::queries::link as commands;

use orch_db::queries::resources as machines;

use orch_db::queries::storage;

use orch_link_contract::commands::CommandKind;

use orch_link_contract::content::BlobProgress;

use orch_link_contract::content::COMMAND_HEADER;

use orch_link_contract::content::COMPLETE_HEADER;

use orch_link_contract::content::ChunkRefusal;

use orch_link_contract::content::HASH_MISMATCH;

use orch_link_contract::content::MAX_CHUNK_BYTES;

use orch_link_contract::content::MAX_MISSING_HASHES;

use orch_link_contract::content::MissingReply;

use orch_link_contract::content::MissingRequest;

use orch_link_contract::content::PutQuery;

use orch_link_contract::content::RECEIVED_HEADER;

use orch_link_contract::content::STATUS_HASH_MISMATCH;

use orch_link_contract::content::STATUS_OFFSET_CONFLICT;

use orch_link_contract::content::accept_chunk;

use orch_link_contract::content::listed_hashes;

use orch_link_contract::content::serves_downloads;

use orch_link_contract::content::takes_uploads;

use orch_pki::certificates::ServiceName;

use orch_settings::Place;

use orch_types::Hash;

use orch_types::Id;

use orch_types::Json;
type ArchiveDelivery =
    crate::key_operations::native::blob_delivery::Delivery<template_import::Archives>;

fn source_fault(error: crate::middleware::requests::ApiError) -> LinkFault {
    LinkFault::new(
        error.problem.status,
        error.problem.code.as_str(),
        error.problem.detail,
    )
}

async fn archive_check(source: &Option<ArchiveDelivery>) -> Result<(), LinkFault> {
    if let Some(source) = source {
        source.check().await.map_err(source_fault)?;
    }
    Ok(())
}

async fn archive_response(
    source: Option<ArchiveDelivery>,
    response: Response,
) -> Result<Response, LinkFault> {
    match source {
        Some(source) => source.response(response).await.map_err(source_fault),
        None => Ok(response),
    }
}

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

const AGENT_ROLE: &str = "agent";

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

fn uploads_request(kind: CommandKind, body: &Json) -> bool {
    uploads(kind)
        || kind == CommandKind::ProviderGenerate
            && body.get("capture_native_result").and_then(Json::as_bool) == Some(true)
}

#[cfg(any(test, feature = "test-support"))]
#[cfg(any(test, feature = "test-support"))]
fn native_result_uploads_require_the_exact_capture_command_and_typed_opt_in() {
    assert!(uploads_request(
        CommandKind::ProviderGenerate,
        &serde_json::json!({"capture_native_result":true})
    ));
    for value in [
        serde_json::json!({}),
        serde_json::json!({"capture_native_result":false}),
        serde_json::json!({"capture_native_result":"true"}),
    ] {
        assert!(!uploads_request(CommandKind::ProviderGenerate, &value));
    }
    assert!(!uploads_request(
        CommandKind::ProviderComplete,
        &serde_json::json!({"capture_native_result":true})
    ));
}
#[cfg(any(test, feature = "test-support"))]
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_native_result_uploads_require_the_exact_capture_command_and_typed_opt_in() {
    self::native_result_uploads_require_the_exact_capture_command_and_typed_opt_in()
}

impl Asker {
    async fn archives(&self, app: &App, hash: Hash) -> Result<Option<ArchiveDelivery>, LinkFault> {
        if matches!(
            &self.command,
            Named::Open {
                kind: CommandKind::NodeTemplateImport,
                ..
            }
        ) {
            Ok(Some(
                template_import::prepare(app, self.machine.id, self.command_id, hash)
                    .await
                    .map_err(source_fault)?,
            ))
        } else {
            Ok(None)
        }
    }
    /// Whether the command lets the machine do `wants`; content for a download is judged apart
    /// ([`Asker::lists`]).
    fn permits(&self, wants: Wants) -> bool {
        match (&self.command, wants) {
            (Named::Standing, Wants::Probe | Wants::Upload) => self
                .machine
                .roles
                .iter()
                .any(|role| role == ENVIRONMENT_ROLE || role == AGENT_ROLE),
            (Named::Standing, Wants::Download) => false,
            (Named::Open { kind, body }, Wants::Upload) => uploads_request(*kind, body),
            (Named::Open { kind, .. }, Wants::Download) => {
                serves_downloads(*kind) || *kind == CommandKind::ProviderService
            }
            (Named::Open { kind, .. }, Wants::Probe) => uploads(*kind) || serves_downloads(*kind),
        }
    }

    /// Whether the command lists the content `hash`: in its body, or, for a command that writes a
    /// tree down, in the list of that tree.
    async fn lists(&self, app: &App, hash: &Hash) -> Result<bool, LinkFault> {
        let Named::Open { kind, body } = &self.command else {
            return Ok(false);
        };
        if *kind == CommandKind::ProviderService {
            let request: orch_link_contract::commands::ProviderService =
                serde_json::from_value(body.clone())
                    .map_err(|error| LinkFault::internal(&error))?;
            return Ok(request.input_images.iter().any(|part| matches!(part,
                orch_model_contract::generation::GenerationPart::ImageRef { blob, .. } if blob == hash)));
        }
        if *kind == CommandKind::ProviderGenerate {
            let request: orch_link_contract::commands::ProviderGenerate =
                serde_json::from_value(body.clone())
                    .map_err(|error| LinkFault::internal(&error))?;
            return Ok(request.input.messages.iter().flat_map(|message|&message.content).any(|part|
                matches!(part,orch_model_contract::generation::GenerationPart::ImageRef {blob,..} if blob==hash)));
        }
        if listed_hashes(body).contains(hash) {
            return Ok(true);
        }
        if !writes_a_tree_down(*kind) {
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

/// Whether a command of this kind writes a tree down from a list it names in `manifest_blob`, and
/// fetches every file of the list by its hash: a copy of a workspace, and the import of a data part
/// that a machine move carries. Their bodies hold the list alone, so the files are those the list names.
fn writes_a_tree_down(kind: CommandKind) -> bool {
    matches!(kind, CommandKind::FsMaterialize | CommandKind::FsDataImport)
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
    let source = asker.archives(&app, hash).await?;
    if let Some(size) = app
        .blobs
        .store()
        .size(&hash)
        .await
        .map_err(|e| LinkFault::internal(&e))?
    {
        return archive_response(
            source,
            progress_reply(
                StatusCode::OK,
                BlobProgress {
                    received: size,
                    complete: true,
                },
            ),
        )
        .await;
    }
    let mut conn = app
        .db
        .acquire()
        .await
        .map_err(|e| LinkFault::internal(&e))?;
    let result = match storage::upload(&mut conn, &hash)
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
    };
    drop(conn);
    match result {
        Ok(response) => archive_response(source, response).await,
        Err(error) => {
            archive_check(&source).await?;
            Err(error)
        }
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
    let source = asker.archives(&app, hash).await?;
    let transfer = app
        .blobs
        .begin_transfer(asker.machine.id)
        .map_err(|error| LinkFault::new(429, "rate_limited", error.to_string()))?;
    let range = header_text(&headers, header::RANGE.as_str()).and_then(ByteRange::from_header);
    let read = match app.blobs.read(&hash, range).await {
        Ok(read) => read,
        Err(BlobError::NotFound(_)) => {
            archive_check(&source).await?;
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
            return archive_response(source, response).await;
        }
        Err(error) => {
            archive_check(&source).await?;
            return Err(LinkFault::internal(&error));
        }
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
    archive_response(source, response).await
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

#[path = "content/template_import.rs"]
pub mod template_import;
