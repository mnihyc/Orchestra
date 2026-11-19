//! The job that writes an archive of what a person may read in a place: each thing read as that person at the moment it is written, laid out as the tree an agent is given, streamed into a zip beside the disk and put in the store of content, with an index and a file that says what it holds.
//!
//! **Read as its person, at the moment it is written.** The job holds what the request was made with (a browser
//! session, or a token with its scopes), and asks the access engine again for every hundred things just before
//! they are written: what the person lost since they asked is left out and counted, what they may read and not
//! take away is left out and counted, and what they may not read is neither written nor named.
//!
//! **Streamed.** A thing is written, put into the zip, and dropped before the next is read; a file a thing carries
//! goes through in pieces. The zip is made by a thread of its own that takes entries from a channel of a few places,
//! so that what waits to be written is bounded, whatever the size of the place; the archive is written to a file in
//! the scratch directory, hashed, and put in the store from the file.
//!
//! **Phases.** `select` says what would be written and refuses at once what no longer fits (the place changed since
//! it was asked); `write` makes the zip; `store` puts it in the store of content, pins it and makes the archive ready,
//! and is the commit: a job that has begun it is not undone. Everything before is: the scratch file goes, the blob
//! this job made goes, and the archive is marked as failed.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::export_layout::{INDEX, Layout, MANIFEST, PACKAGE_MANIFEST, Placed};
use crate::exports::{
    BATCH, DocumentHow, NOT_EXPORTABLE, Selected, TOO_LARGE, Thing, Wanted, blockers_of,
};
use crate::jobs::runner::{Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone};
use crate::tools::documents as document_text;
use crate::written::{self, Detail};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::documents::DocumentExport;
use orch_api_contract::exports::{ExportCounts, ExportLeftOut, ExportRequest};
use orch_api_contract::threads::{ContentPart, ExportFormat};
use orch_authz::{Caller, Principal, TokenGrant};
use orch_blob_contract::store::{BlobError, ByteStream};
use orch_db::audit::{Actor, Entry as AuditEntry, Origin, append};
use orch_db::queries::documents::history;
use orch_db::queries::exports::{self as rows, DocumentFile, Written};
use orch_db::queries::jobs::{JobRow, kind};
use orch_db::queries::{identity, shares as share_rows, storage};
use orch_db::queries::{thread_continuations, thread_history as selected};
use orch_documents::markdown;
use orch_types::{Hash, Id, Json, Time};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio_util::io::ReaderStream;

mod package;

/// What the job is called in the table of jobs.
pub const KIND: &str = kind::EXPORT;

/// The phases of the job, with the weight of each: reading what there is is quick, writing is where the time
/// goes, and putting the archive in the store is a copy of one file.
pub const PHASES: [Phase; 3] = [
    Phase {
        name: "select",
        weight: 5,
    },
    Phase {
        name: "write",
        weight: 90,
    },
    Phase {
        name: "store",
        weight: 5,
    },
];

/// How many pieces wait for the thread that makes the zip: a few, so that a slow disk slows the reading down
/// and no more than this many pieces of memory are ever held for it.
const PIECES_WAITING: usize = 4;

/// How large a piece of a file is that goes into the zip, and how much of a file the hash reads at a time: a
/// quarter of a mebibyte, the size the store reads and writes in.
const PIECE_BYTES: usize = 256 * 1024;

/// Thirty-two turns/messages keep text projections and their lookups bounded,
/// without a database round trip per turn. A single turn may be larger than a window.
const TEXT_WINDOW: i64 = 32;

/// Record pages carry no rendered copies, so they can hold four text windows.
const RECORD_PAGE: i64 = TEXT_WINDOW * 4;

/// How many things are written before the percentage is written down again: often enough to be seen to move,
/// rarely enough for the row of the job to be changed a few times a second at the most.
const PROGRESS_EVERY: usize = 10;

/// The mode of the files in the zip.
const FILE_MODE: u32 = 0o644;

// ---------------------------------------------------------------- the zip, made beside the reading

enum Piece {
    /// A whole entry: its path and its bytes.
    Whole(String, Vec<u8>),
    /// The start of an entry that comes in pieces.
    Start(String),
    /// A piece of it.
    Chunk(Bytes),
    /// Its end.
    End,
    /// The end of the zip.
    Finish,
}

/// A zip being written by a thread of its own, from pieces that wait in a channel of a few places.
struct Archive {
    sender: Option<mpsc::Sender<Piece>>,
    worker: Option<std::thread::JoinHandle<std::io::Result<u64>>>,
    /// How many entries have been given.
    entries: i64,
    /// How many bytes of content have been given, before they are compressed.
    fed: i64,
    /// The paths that have been given: a zip holds each once.
    names: HashSet<String>,
    /// The SHA-256 and the size of each entry given, by its path: what a package's manifest lists.
    digests: HashMap<String, (String, i64)>,
    /// The entry that is being given in pieces, with the hash of what has been given of it.
    open: Option<(String, Sha256, i64)>,
    /// The people the records written name, where the zip is a package; none where it is not.
    people: Option<BTreeSet<Id>>,
}

fn broken(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

impl Archive {
    /// Begins a zip in the file `path`, which is made anew.
    fn begin(path: &Path) -> Result<Self, Failure> {
        let file = std::fs::File::create(path).map_err(|error| scratch_failed(&error))?;
        let (sender, mut receiver) = mpsc::channel::<Piece>(PIECES_WAITING);
        let worker = std::thread::spawn(move || -> std::io::Result<u64> {
            let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
            let whole = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(FILE_MODE);
            let streamed = whole.large_file(true);
            while let Some(piece) = receiver.blocking_recv() {
                match piece {
                    Piece::Whole(name, bytes) => {
                        zip.start_file(name, whole).map_err(broken)?;
                        zip.write_all(&bytes)?;
                    }
                    Piece::Start(name) => zip.start_file(name, streamed).map_err(broken)?,
                    Piece::Chunk(bytes) => zip.write_all(&bytes)?,
                    Piece::End => {}
                    Piece::Finish => break,
                }
            }
            let mut file = zip.finish().map_err(broken)?;
            file.flush()?;
            let size = file.get_ref().metadata()?.len();
            Ok(size)
        });
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            entries: 0,
            fed: 0,
            names: HashSet::new(),
            digests: HashMap::new(),
            open: None,
            people: None,
        })
    }

    /// Begins to note the people that the records name, for the manifest of a package.
    fn note_people(&mut self) {
        self.people.get_or_insert_with(BTreeSet::new);
    }

    /// The SHA-256 (in lower case hexadecimal) and the size of the entry written at `path`.
    fn digest_of(&self, path: &str) -> Option<(String, i64)> {
        self.digests.get(path).cloned()
    }

    /// Whether an entry of this path has been given already.
    fn holds(&self, path: &str) -> bool {
        self.names.contains(path)
    }

    async fn send(&mut self, piece: Piece) -> Result<(), Failure> {
        let Some(sender) = &self.sender else {
            return Err(scratch_failed(&"the archive is closed"));
        };
        if sender.send(piece).await.is_err() {
            // The thread stopped: it says why when it is joined.
            return Err(self.stopped().await);
        }
        Ok(())
    }

    async fn stopped(&mut self) -> Failure {
        self.sender = None;
        match self.worker.take() {
            Some(worker) => match tokio::task::spawn_blocking(move || worker.join()).await {
                Ok(Ok(Err(error))) => scratch_failed(&error),
                _ => scratch_failed(&"the thread that writes the archive stopped"),
            },
            None => scratch_failed(&"the archive is closed"),
        }
    }

    /// Adds a file given whole.
    async fn add(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), Failure> {
        self.names.insert(path.to_owned());
        self.entries += 1;
        self.digests.insert(
            path.to_owned(),
            (
                hex::encode(Sha256::digest(&bytes)),
                i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            ),
        );
        self.fed = self
            .fed
            .saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX));
        self.send(Piece::Whole(path.to_owned(), bytes)).await
    }

    async fn start(&mut self, path: &str) -> Result<(), Failure> {
        self.names.insert(path.to_owned());
        self.entries += 1;
        self.open = Some((path.to_owned(), Sha256::new(), 0));
        self.send(Piece::Start(path.to_owned())).await
    }

    /// Even a large turn/record occupies at most one bounded channel piece at
    /// a time. Awaiting each send applies the zip worker's backpressure.
    async fn chunk(&mut self, bytes: impl Into<Bytes>) -> Result<(), Failure> {
        let bytes = bytes.into();
        self.fed = self
            .fed
            .saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX));
        if let Some((_, hasher, size)) = self.open.as_mut() {
            hasher.update(&bytes);
            *size = size.saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX));
        }
        for from in (0..bytes.len()).step_by(PIECE_BYTES) {
            self.send(Piece::Chunk(
                bytes.slice(from..(from + PIECE_BYTES).min(bytes.len())),
            ))
            .await?;
        }
        Ok(())
    }

    async fn end(&mut self) -> Result<(), Failure> {
        if let Some((path, hasher, size)) = self.open.take() {
            self.digests
                .insert(path, (hex::encode(hasher.finalize()), size));
        }
        self.send(Piece::End).await
    }

    async fn record<T: serde::Serialize>(
        &mut self,
        record: &T,
        comma: &mut bool,
    ) -> Result<(), Stop> {
        if *comma {
            self.chunk(Bytes::from_static(b",")).await?;
        }
        let bytes = if self.people.is_some() {
            let value = serde_json::to_value(record)
                .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
            if let Some(people) = self.people.as_mut() {
                package::people_in(&value, people);
            }
            serde_json::to_vec(&value).map_err(|error| Failure::new(INTERNAL, error.to_string()))?
        } else {
            serde_json::to_vec(record).map_err(|error| Failure::new(INTERNAL, error.to_string()))?
        };
        self.chunk(bytes).await?;
        *comma = true;
        Ok(())
    }

    /// Adds a file that comes from the store in pieces.
    async fn add_stream(&mut self, path: &str, mut body: ByteStream) -> Result<(), Failure> {
        self.start(path).await?;
        while let Some(piece) = body.next().await {
            let piece = piece.map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
            self.chunk(piece).await?;
        }
        self.end().await
    }

    /// Ends the zip and says how large it came to.
    async fn finish(mut self) -> Result<u64, Failure> {
        self.send(Piece::Finish).await?;
        self.sender = None;
        let Some(worker) = self.worker.take() else {
            return Err(scratch_failed(&"the archive is closed"));
        };
        match tokio::task::spawn_blocking(move || worker.join()).await {
            Ok(Ok(Ok(size))) => Ok(size),
            Ok(Ok(Err(error))) => Err(scratch_failed(&error)),
            _ => Err(scratch_failed(
                &"the thread that writes the archive stopped",
            )),
        }
    }
}

fn scratch_failed(error: &dyn std::fmt::Display) -> Failure {
    Failure::new(
        INTERNAL,
        format!("the archive could not be written: {error}"),
    )
}

/// The hash of a file and its size, read a piece at a time.
fn hash_file(path: &Path) -> std::io::Result<(Hash, i64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hash::hasher();
    let mut buffer = vec![0_u8; PIECE_BYTES];
    let mut size: i64 = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size.saturating_add(i64::try_from(read).unwrap_or(i64::MAX));
    }
    Ok((Hash::finish(&hasher), size))
}

// ---------------------------------------------------------------- the job

/// The phased work of one job that writes an archive.
pub struct Exporting {
    engine: Engine,
    export: Id,
    user: Id,
    node: Id,
    credential: Option<(Vec<String>, Option<Id>)>,
    /// Whether the people of a package are named with their address.
    emails: bool,
}

fn text_id(plan: &Json, member: &str) -> Option<Id> {
    plan.get(member)?.as_str()?.parse().ok()
}

impl Exporting {
    /// The work of the job `row`, from what its plan holds.
    pub fn of(engine: Engine, row: &JobRow) -> Result<Self, EngineError> {
        let missing = |member: &str| {
            EngineError::unreadable(format!("a job that writes an archive lacks its {member}"))
        };
        let credential = row
            .plan
            .get("credential")
            .filter(|each| !each.is_null())
            .map(|each| {
                let scopes: Vec<String> = each
                    .get("scopes")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|scope| scope.as_str().map(str::to_owned))
                    .collect();
                (scopes, text_id(each, "subtree"))
            });
        Ok(Self {
            engine,
            export: text_id(&row.plan, "export").ok_or_else(|| missing("archive"))?,
            user: text_id(&row.plan, "user").ok_or_else(|| missing("person"))?,
            node: text_id(&row.plan, "node").ok_or_else(|| missing("place"))?,
            credential,
            emails: row
                .plan
                .get("emails")
                .and_then(Json::as_bool)
                .unwrap_or(false),
        })
    }

    /// Whoever asked, reading with what they asked with.
    fn caller(&self) -> Result<Caller, Failure> {
        match &self.credential {
            None => Ok(Caller::session(self.user)),
            Some((scopes, subtree)) => TokenGrant::from_stored(scopes, *subtree)
                .map(|grant| Caller::token(self.user, grant))
                .map_err(|error| Failure::new(INTERNAL, error.to_string())),
        }
    }

    async fn principal(&self, conn: &mut orch_db::Conn) -> Result<Principal, Stop> {
        Principal::load(
            conn,
            self.caller()?,
            Time::new(self.engine.inner.clock.now()),
        )
        .await
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))
    }

    /// The request, as it was asked and as the row of the archive keeps it, and whether it is still waiting.
    async fn asked(&self, conn: &mut orch_db::Conn) -> Result<(Wanted, ExportRequest), Stop> {
        let row = rows::get(conn, self.export)
            .await?
            .filter(|row| row.state == rows::state::PENDING)
            .ok_or(Stop::Cancelled)?;
        let request: ExportRequest = serde_json::from_value(row.request)
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        let wanted =
            Wanted::read(&request).map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        Ok((wanted, request))
    }

    fn scratch_path(&self, job: &Job) -> PathBuf {
        self.engine
            .scratch_dir()
            .join(format!("export-{}.zip", job.id().simple()))
    }
}

#[async_trait]
impl Phased for Exporting {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "select" => self.select(job).await,
            "write" => self.write(job).await,
            "store" => self.store(job).await,
            other => Err(Failure::new(
                INTERNAL,
                format!("a job that writes an archive has no phase {other}"),
            )
            .into()),
        }
    }

    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        let scratch = self.scratch_path(job);
        if let Err(error) = std::fs::remove_file(&scratch)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "the scratch file of an archive that was not made could not be removed");
        }
        if let Ok(plan) = job.plan().await
            && let Some(hash) = plan
                .get("made_blob")
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Hash>().ok())
            && let Some(store) = self.engine.blobs()
            && let Err(error) = store.delete(&hash).await
        {
            tracing::warn!(%error, "the content an archive was put in could not be removed");
        }
        let left_out = job
            .plan()
            .await
            .ok()
            .and_then(|plan| plan.get("left_out").cloned())
            .unwrap_or_else(|| json!([]));
        match self.engine.inner.db.acquire().await {
            Ok(mut conn) => {
                if let Err(error) = rows::make_failed(&mut conn, self.export, &left_out).await {
                    tracing::warn!(%error, "an archive that was not made could not be marked as failed");
                    return Undone::Partly("the archive was not marked as failed".to_owned());
                }
                Undone::Everything
            }
            Err(_) => Undone::Partly("the archive was not marked as failed".to_owned()),
        }
    }

    async fn result(&self, _job: &Job) -> Option<Json> {
        Some(json!({ "export_id": self.export }))
    }
}

// ---------------------------------------------------------------- select

impl Exporting {
    async fn select(&self, job: &Job) -> Result<Done, Stop> {
        let mut conn = self.engine.inner.db.acquire().await?;
        let (wanted, _) = self.asked(&mut conn).await?;
        let principal = self.principal(&mut conn).await?;
        let selected = self
            .engine
            .select_export(&mut conn, &principal, self.node, &wanted)
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        let limits = self
            .engine
            .export_limits(&mut conn, self.node)
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        drop(conn);
        if let Some((by, _, _, message)) = blockers_of(&selected, &limits).into_iter().next() {
            return Err(Failure::new(by.code(), message).into());
        }
        let (items, bytes) = (selected.items(), selected.bytes);
        job.amend(move |progress, _| {
            progress.files_total = Some(items);
            progress.bytes_total = Some(bytes);
        })
        .await?;
        Ok(Done::Finished)
    }
}

// ---------------------------------------------------------------- write

/// Puts a file of the store into the zip under `path`, unless it is past the largest file an archive takes or is
/// not there any more. Says whether it went in.
async fn copy_blob(
    engine: &Engine,
    archive: &mut Archive,
    path: &str,
    blob: &Hash,
) -> Result<bool, Stop> {
    let Some(store) = engine.blobs() else {
        return Ok(false);
    };
    let most = engine.inner.config.export_file_max_bytes;
    let read = match store.get(blob, None).await {
        Ok(read) => read,
        Err(BlobError::NotFound(_)) => return Ok(false),
        Err(error) => return Err(Failure::new(INTERNAL, error.to_string()).into()),
    };
    if read.meta.size > most {
        return Ok(false);
    }
    archive.add_stream(path, read.body).await?;
    Ok(true)
}

/// The files the messages of a chat carry, `range` of them by number, each with the name it has in the directory of
/// the chat's files (its message number, carried-file number and cleaned name) and where its content is.
async fn chat_file_entries(
    conn: &mut orch_db::Conn,
    chat: Id,
    range: (Option<i64>, Option<i64>),
) -> Result<Vec<(String, Hash)>, Stop> {
    let last = share_rows::chat_total(conn, chat).await?;
    if last == 0 {
        return Ok(Vec::new());
    }
    let Some((low, high)) = written::export_range(range, last) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for each in share_rows::chat_messages_numbered(conn, chat, low, high).await? {
        if each.row.is_deleted() {
            continue;
        }
        let Some(parts) = each.row.content.as_array() else {
            continue;
        };
        let mut carried_number = 0_usize;
        for part in parts {
            let Ok(part) =
                serde_json::from_value::<orch_api_contract::chats::ChatPart>(part.clone())
            else {
                continue;
            };
            // What the message carries, and what a message forwarded in it carried: the text links to each.
            let carried: Vec<(String, Hash)> = match part {
                orch_api_contract::chats::ChatPart::PastedText(paste) => {
                    vec![(paste.name, paste.blob)]
                }
                orch_api_contract::chats::ChatPart::Image { name, blob, .. }
                | orch_api_contract::chats::ChatPart::Attachment { name, blob, .. } => {
                    vec![(name, blob)]
                }
                orch_api_contract::chats::ChatPart::Forwarded(copy) => copy
                    .images
                    .into_iter()
                    .map(|image| (image.name, image.blob))
                    .chain(
                        copy.attachments
                            .into_iter()
                            .map(|file| (file.name, file.blob)),
                    )
                    .collect(),
                _ => continue,
            };
            for (name, blob) in carried {
                carried_number += 1;
                out.push((
                    orch_chat::written::file_name(each.number, carried_number, &name),
                    blob,
                ));
            }
        }
    }
    Ok(out)
}

/// Inclusive windows of one already resolved range; no list of windows is kept.
fn text_windows((first, last): (i64, i64)) -> impl Iterator<Item = (i64, i64)> {
    std::iter::successors((first <= last).then_some(first), move |from| {
        let to = from.saturating_add(TEXT_WINDOW - 1).min(last);
        (to < last).then(|| to + 1)
    })
    .map(move |from| (from, from.saturating_add(TEXT_WINDOW - 1).min(last)))
}

/// Both place archives and a chat's own zip use this bounded producer.
async fn stream_chat(
    conn: &mut orch_db::Conn,
    archive: &mut Archive,
    path: &str,
    chat: Id,
    format: ExportFormat,
    range: (Option<i64>, Option<i64>),
    files: &str,
) -> Result<i64, Stop> {
    let last = share_rows::chat_total(conn, chat).await?;
    let span = written::export_range(range, last);
    archive.start(path).await?;
    match format {
        ExportFormat::Markdown => {
            let title = written::chat_title(conn, chat).await?;
            if let Some(span) = span {
                for (at, window) in text_windows(span).enumerate() {
                    let read = written::read_chat_window(
                        conn,
                        chat,
                        &title,
                        window,
                        files,
                        (at == 0).then_some(span),
                    )
                    .await?;
                    archive.chunk(read.text.into_bytes()).await?;
                }
            } else {
                let read = written::read_chat_in(
                    conn,
                    chat,
                    &title,
                    range,
                    Detail::Normal,
                    None,
                    Some(files),
                )
                .await?;
                archive.chunk(read.text.into_bytes()).await?;
            }
        }
        ExportFormat::Json => {
            archive.chunk(Bytes::from_static(b"[")).await?;
            let mut comma = false;
            if let Some(span) = span {
                for (low, high) in text_windows(span) {
                    let records =
                        written::chat_records(conn, chat, (Some(low), Some(high))).await?;
                    for record in &records {
                        archive.record(record, &mut comma).await?;
                    }
                }
            }
            archive.chunk(Bytes::from_static(b"]")).await?;
        }
    }
    archive.end().await?;
    Ok(last)
}

/// The directory of the archive that holds the files of the text kept at `placed`: beside it, named the way the text
/// names it ([`written::files_beside`]), so that a link in the text leads to the file.
fn files_directory(placed: &Placed) -> String {
    let beside = written::files_beside(&placed.path);
    match placed.path.rsplit_once('/') {
        Some((directory, _)) => format!("{directory}/{beside}"),
        None => beside,
    }
}

/// What the writing keeps count of.
#[derive(Default)]
struct Tally {
    conversations: i64,
    chats: i64,
    documents: i64,
    files: i64,
    /// What was read and not written, by why and by place.
    left_out: BTreeMap<(u8, Id), i64>,
    /// The files of the archive, each with what it is, for the index.
    listed: Vec<(String, String)>,
    /// What a package's manifest is made from: the things written.
    entries: Vec<package::Entry>,
    /// The files the thing now being written carries, by the name it has them under and the path they lie at.
    carried: Vec<package::Carried>,
}

#[derive(Clone, Copy)]
struct ConversationScope<'a> {
    thread: Id,
    projection: &'a thread_continuations::Projection,
    range: (Option<i64>, Option<i64>),
}

impl Exporting {
    async fn write(&self, job: &Job) -> Result<Done, Stop> {
        let scratch = self.scratch_path(job);
        if let Some(dir) = scratch.parent() {
            std::fs::create_dir_all(dir).map_err(|error| scratch_failed(&error))?;
        }
        // A runner that took the job over starts the zip again.
        let _ = std::fs::remove_file(&scratch);
        let mut conn = self.engine.inner.db.acquire().await?;
        let (wanted, request) = self.asked(&mut conn).await?;
        let principal = self.principal(&mut conn).await?;
        let selected = self
            .engine
            .select_export(&mut conn, &principal, self.node, &wanted)
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        let limits = self
            .engine
            .export_limits(&mut conn, self.node)
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        drop(conn);
        let layout = selected.layout(&wanted);
        let mut archive = Archive::begin(&scratch)?;
        if wanted.package {
            archive.note_people();
        }
        let mut tally = Tally::default();
        // What was left out when the things were read goes in the count as well.
        tally.left_out.clone_from(&selected.left_out);
        let mut items: Vec<(Kind, &Thing)> = Vec::new();
        items.extend(
            selected
                .conversations
                .iter()
                .map(|thing| (Kind::Conversation, thing)),
        );
        items.extend(selected.chats.iter().map(|thing| (Kind::Chat, thing)));
        items.extend(
            selected
                .documents
                .iter()
                .map(|thing| (Kind::Document, thing)),
        );
        let total = items.len().max(1);
        let mut done = 0_usize;
        for batch in items.chunks(BATCH) {
            job.check()?;
            let mut conn = self.engine.inner.db.acquire().await?;
            let principal = self.principal(&mut conn).await?;
            let threads: Vec<Id> = batch
                .iter()
                .filter(|(kind, _)| *kind == Kind::Conversation)
                .map(|(_, thing)| thing.id)
                .collect();
            let chat_nodes: Vec<Id> = batch
                .iter()
                .filter(|(kind, _)| *kind == Kind::Chat)
                .map(|(_, thing)| thing.node)
                .collect();
            let documents: Vec<Id> = batch
                .iter()
                .filter(|(kind, _)| *kind == Kind::Document)
                .map(|(_, thing)| thing.id)
                .collect();
            let access = crate::exports::Access::of(
                &mut conn,
                &principal,
                &threads,
                &chat_nodes,
                &documents,
            )
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
            for (kind, thing) in batch {
                job.check()?;
                let (reads, exports) = match kind {
                    Kind::Conversation => (
                        access.reads_conversations.contains(&thing.id),
                        access.exports_conversations.contains(&thing.id),
                    ),
                    Kind::Chat => (
                        access.reads_chats.contains(&thing.node),
                        access.exports_chats.contains(&thing.node),
                    ),
                    Kind::Document => (
                        access.reads_documents.contains(&thing.id),
                        access.exports_documents.contains(&thing.id),
                    ),
                };
                if reads && exports {
                    self.write_thing(
                        &mut conn,
                        &principal,
                        &wanted,
                        &layout,
                        &mut archive,
                        &mut tally,
                        *kind,
                        thing,
                    )
                    .await?;
                } else if reads {
                    // The Export row was lost since the things were read: left out and counted, in the place
                    // it lay in. What cannot be read any more is not named anywhere.
                    *tally
                        .left_out
                        .entry((NOT_EXPORTABLE, thing.home))
                        .or_default() += 1;
                }
                if archive.fed > limits.max_bytes {
                    return Err(Failure::new(
                        "export.too_large",
                        format!(
                            "the archive passed the {} bytes the place allows",
                            limits.max_bytes
                        ),
                    )
                    .into());
                }
                done += 1;
                if done.is_multiple_of(PROGRESS_EVERY) {
                    let (fed, files) = (archive.fed, tally.files);
                    job.progress(
                        done as f64 / total as f64,
                        Some(&format!("{done} of {total} things")),
                    )
                    .await?;
                    job.amend(move |progress, _| {
                        progress.bytes_done = fed;
                        progress.files_done = files;
                    })
                    .await?;
                }
            }
        }
        // The index and the file that says what it holds come last: they know what was written.
        self.finish_archive(job, &selected, &wanted, &request, &layout, archive, tally)
            .await?;
        Ok(Done::Finished)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "what is read, as whom, in what form, where it goes, and what is counted"
    )]
    async fn write_thing(
        &self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        wanted: &Wanted,
        layout: &Layout,
        archive: &mut Archive,
        tally: &mut Tally,
        kind: Kind,
        thing: &Thing,
    ) -> Result<(), Stop> {
        match kind {
            Kind::Conversation => {
                let Some(placed) = layout.conversations.get(&thing.id) else {
                    return Ok(());
                };
                self.write_conversation(conn, principal, wanted, archive, tally, thing, placed)
                    .await
            }
            Kind::Chat => {
                let Some(placed) = layout.chats.get(&thing.id) else {
                    return Ok(());
                };
                self.write_chat(conn, wanted, archive, tally, thing, placed)
                    .await
            }
            Kind::Document => {
                let Some(placed) = layout.documents.get(&thing.id) else {
                    return Ok(());
                };
                self.write_document(conn, principal, wanted, archive, tally, thing, placed)
                    .await
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "what is read, as whom, in what form, where it goes, and what is counted"
    )]
    async fn write_conversation(
        &self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        wanted: &Wanted,
        archive: &mut Archive,
        tally: &mut Tally,
        thing: &Thing,
        placed: &Placed,
    ) -> Result<(), Stop> {
        let projection = thread_continuations::projection(conn, thing.id, None)
            .await?
            .ok_or_else(|| Failure::new(INTERNAL, "The thread history was removed."))?;
        let view = projection.viewed_continuation_id;
        let range = match share_rows_turns(conn, thing.id, view, wanted).await? {
            Some(range) => (Some(range.0), Some(range.1)),
            None => (None, None),
        };
        let scope = ConversationScope {
            thread: thing.id,
            projection: &projection,
            range,
        };
        let last = selected::last_turn(conn, thing.id, view).await?;
        // A package writes both forms of a conversation: its record and its reading copy.
        let reading = package_reading(wanted, placed);
        let record =
            (wanted.package || wanted.format == ExportFormat::Json).then(|| placed.path.clone());
        if let Some(path) = &reading {
            let files = written::files_beside(path);
            archive.start(path).await?;
            if let Some(span) = written::export_range(range, last) {
                for (at, window) in text_windows(span).enumerate() {
                    let read = written::read_conversation_window_view(
                        conn,
                        thing.id,
                        view,
                        window,
                        Detail::from(wanted.detail),
                        &files,
                        (at == 0).then_some(span),
                    )
                    .await?
                    .ok_or_else(|| Failure::new(INTERNAL, "a thread that was read is gone"))?;
                    archive.chunk(read.text.into_bytes()).await?;
                }
            } else {
                let read = written::read_conversation_view(
                    conn,
                    thing.id,
                    view,
                    range,
                    Detail::from(wanted.detail),
                    None,
                    Some(&files),
                )
                .await?
                .ok_or_else(|| Failure::new(INTERNAL, "a thread that was read is gone"))?;
                archive.chunk(read.text.into_bytes()).await?;
            }
            archive.end().await?;
            tally.listed.push((
                path.clone(),
                format!("the conversation of {}, {last} turns", thing.title),
            ));
        }
        if let Some(path) = &record {
            self.stream_conversation_json(conn, principal, archive, scope, path)
                .await?;
            tally.listed.push((
                path.clone(),
                format!("the conversation of {}, as records", thing.title),
            ));
        }
        tally.conversations += 1;
        tally.carried.clear();
        if wanted.files {
            self.write_input_files(conn, wanted, archive, tally, thing, placed, scope)
                .await?;
        }
        if wanted.package {
            tally.entries.push(package::Entry {
                kind: orch_api_contract::package::PackageKind::Thread,
                id: thing.id,
                parent: thing.home,
                path: Some(placed.path.clone()),
                reading,
                name: thing.title.clone(),
                turns: Some(last),
                files: std::mem::take(&mut tally.carried),
                ..package::Entry::default()
            });
        }
        Ok(())
    }

    /// The same wire records as ThreadExportJson, serialized one bounded page
    /// at a time. Each array has its own pass, so neither turn ids nor segments
    /// for the whole conversation need to stay in memory.
    async fn stream_conversation_json(
        &self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        archive: &mut Archive,
        scope: ConversationScope<'_>,
        path: &str,
    ) -> Result<(), Stop> {
        use crate::views::{People, bypass_people, item_view, segment_view, turn_view};
        use orch_db::queries::{resources::thread_engine as resources, threads};
        let thread = scope.thread;
        let range = scope.range;
        let history = scope.projection.viewed_continuation_id;
        let Some(forms) = self.engine.forms() else {
            return Err(forms_missing().into());
        };
        let view = forms
            .thread(conn, principal, thread, self.engine.inner.clock.now())
            .await
            .map_err(|fault| Failure::new(INTERNAL, fault.to_string()))?;
        archive.start(path).await?;
        archive
            .chunk(Bytes::from_static(b"{\"projection\":"))
            .await?;
        archive
            .chunk(
                serde_json::to_vec(&crate::continuations::projection::scope(scope.projection))
                    .map_err(|error| Failure::new(INTERNAL, error.to_string()))?,
            )
            .await?;
        archive.chunk(Bytes::from_static(b",\"thread\":")).await?;
        let bytes =
            serde_json::to_vec(&view).map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        if let Some(people) = archive.people.as_mut()
            && let Ok(value) = serde_json::from_slice::<Json>(&bytes)
        {
            package::people_in(&value, people);
        }
        archive.chunk(bytes).await?;
        drop(view);
        archive
            .chunk(Bytes::from_static(b",\"segments\":["))
            .await?;
        let mut comma = false;
        let mut after_segment = 0;
        loop {
            let segments =
                selected::segments_after(conn, thread, history, after_segment, RECORD_PAGE).await?;
            let Some(last) = segments.last() else { break };
            after_segment = last.ordinal;
            let people = People::load(
                conn,
                segments.iter().filter_map(|segment| segment.started_by),
            )
            .await?;
            for segment in &segments {
                let view = segment_view(
                    conn,
                    segment,
                    &people,
                    Time::new(self.engine.inner.clock.now()),
                )
                .await?;
                archive.record(&view, &mut comma).await?;
            }
        }
        archive.chunk(Bytes::from_static(b"],\"turns\":[")).await?;
        comma = false;
        let last = selected::last_turn(conn, thread, history).await?;
        if let Some(span) = written::export_range(range, last) {
            for (low, high) in text_windows(span) {
                let turns = selected::turns_between(conn, thread, history, low, high).await?;
                let who = turns.iter().flat_map(|turn| {
                    turn.interrupted_by
                        .into_iter()
                        .chain(bypass_people(turn.limit_bypass.as_ref()))
                });
                let people = People::load(conn, who).await?;
                for turn in &turns {
                    let display = match threads::segment(conn, turn.segment_id).await? {
                        Some(segment) => resources::model(conn, segment.provider_id, &turn.model)
                            .await?
                            .map(|model| model.display_name)
                            .unwrap_or_default(),
                        None => String::new(),
                    };
                    archive
                        .record(&turn_view(turn, &display, &people)?, &mut comma)
                        .await?;
                }
            }
        }
        archive.chunk(Bytes::from_static(b"],\"items\":[")).await?;
        comma = false;
        let mut after_item = 0;
        loop {
            let mut items = thread_continuations::reading::items(
                conn,
                thread,
                history,
                Some(after_item),
                true,
                None,
                RECORD_PAGE,
            )
            .await?;
            let Some(last) = items.last() else { break };
            after_item = last.seq_created;
            let narrowed = range.0.is_some() || range.1.is_some();
            if narrowed {
                let ids: Vec<_> = items.iter().filter_map(|item| item.turn_id).collect();
                let turns =
                    thread_continuations::reading::turns_among(conn, thread, history, &ids).await?;
                let kept: HashSet<_> = turns
                    .iter()
                    .filter(|turn| {
                        range.0.is_none_or(|from| i64::from(turn.ordinal) >= from)
                            && range.1.is_none_or(|to| i64::from(turn.ordinal) <= to)
                    })
                    .map(|turn| turn.id)
                    .collect();
                items.retain(|item| item.turn_id.is_none_or(|turn| kept.contains(&turn)));
            }
            let who = items
                .iter()
                .flat_map(|item| item.author_id.into_iter().chain(item.pinned_by));
            let people = People::load(conn, who).await?;
            for item in &items {
                archive
                    .record(&item_view(item, &people), &mut comma)
                    .await?;
            }
        }
        archive.chunk(Bytes::from_static(b"]}")).await?;
        archive.end().await?;
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "what is read, in what form, where it goes, what is counted, and the files"
    )]
    async fn write_input_files(
        &self,
        conn: &mut orch_db::Conn,
        _wanted: &Wanted,
        archive: &mut Archive,
        tally: &mut Tally,
        thing: &Thing,
        placed: &Placed,
        scope: ConversationScope<'_>,
    ) -> Result<(), Stop> {
        let range = scope.range;
        let view = scope.projection.viewed_continuation_id;
        let last = selected::last_turn(conn, thing.id, view).await?;
        if last == 0 {
            return Ok(());
        }
        let Some(span) = written::export_range(range, last) else {
            return Ok(());
        };
        for (low, high) in text_windows(span) {
            let turn_rows = selected::turns_between(conn, thing.id, view, low, high).await?;
            for turn in &turn_rows {
                let items = selected::items_of_turns(conn, thing.id, view, &[turn.id]).await?;
                let mut carried = 0_usize;
                for item in items.iter().filter(|item| item.kind == "user_message") {
                    let Some(parts) = item.payload.get("parts").and_then(Json::as_array) else {
                        continue;
                    };
                    for part in parts {
                        let Ok(part) = serde_json::from_value::<ContentPart>(part.clone()) else {
                            continue;
                        };
                        let (name, blob) = match part {
                            ContentPart::PastedText(paste) => (paste.name, Some(paste.blob)),
                            ContentPart::Image { name, blob, .. } => (name, Some(blob)),
                            ContentPart::File { name, blob, .. } => (name, blob),
                            _ => continue,
                        };
                        carried += 1;
                        let Some(blob) = blob else { continue };
                        let path = format!(
                            "{}/in-{carried}-{}",
                            written::files_in(&files_directory(placed), i64::from(turn.ordinal)),
                            written::clean_name(&name)
                        );
                        self.copy_blob(archive, tally, &path, &blob).await?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Puts a file of the store into the zip under `path` and counts it.
    async fn copy_blob(
        &self,
        archive: &mut Archive,
        tally: &mut Tally,
        path: &str,
        blob: &Hash,
    ) -> Result<(), Stop> {
        if archive.holds(path) {
            return Ok(());
        }
        if copy_blob(&self.engine, archive, path, blob).await? {
            tally.files += 1;
            tally.listed.push((path.to_owned(), "a file".to_owned()));
            tally.carried.push(package::Carried {
                id: None,
                name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                path: path.to_owned(),
            });
        }
        Ok(())
    }

    async fn write_chat(
        &self,
        conn: &mut orch_db::Conn,
        wanted: &Wanted,
        archive: &mut Archive,
        tally: &mut Tally,
        thing: &Thing,
        placed: &Placed,
    ) -> Result<(), Stop> {
        let range = match rows::message_numbers_in(conn, thing.id, wanted.span).await? {
            Some((first, last)) if wanted.span.from.is_some() || wanted.span.until.is_some() => {
                (Some(first), Some(last))
            }
            _ => (None, None),
        };
        // A package writes both forms of a chat: its messages as records, and its reading copy.
        let files_dir = written::files_beside(&placed.path);
        let mut last = 0;
        if wanted.package || wanted.format == ExportFormat::Json {
            last = stream_chat(
                conn,
                archive,
                &placed.path,
                thing.id,
                ExportFormat::Json,
                range,
                &files_dir,
            )
            .await?;
            tally.listed.push((
                placed.path.clone(),
                format!("the chat of {}, as records", thing.title),
            ));
        }
        let reading = package_reading(wanted, placed);
        if let Some(path) = &reading {
            last = stream_chat(
                conn,
                archive,
                path,
                thing.id,
                ExportFormat::Markdown,
                range,
                &files_dir,
            )
            .await?;
            tally.listed.push((
                path.clone(),
                format!("the chat of {}, {last} messages", thing.title),
            ));
        }
        tally.chats += 1;
        tally.carried.clear();
        if wanted.files {
            self.write_chat_files(conn, archive, tally, thing, placed, range)
                .await?;
        }
        if wanted.package {
            tally.entries.push(package::Entry {
                kind: orch_api_contract::package::PackageKind::Chat,
                id: thing.id,
                parent: thing.node,
                path: Some(placed.path.clone()),
                reading,
                name: thing.title.clone(),
                messages: Some(last),
                files: std::mem::take(&mut tally.carried),
                ..package::Entry::default()
            });
        }
        Ok(())
    }

    async fn write_chat_files(
        &self,
        conn: &mut orch_db::Conn,
        archive: &mut Archive,
        tally: &mut Tally,
        thing: &Thing,
        placed: &Placed,
        range: (Option<i64>, Option<i64>),
    ) -> Result<(), Stop> {
        let directory = files_directory(placed);
        let last = share_rows::chat_total(conn, thing.id).await?;
        if let Some(span) = written::export_range(range, last) {
            for (low, high) in text_windows(span) {
                for (name, blob) in
                    chat_file_entries(conn, thing.id, (Some(low), Some(high))).await?
                {
                    let path = format!("{directory}/{name}");
                    self.copy_blob(archive, tally, &path, &blob).await?;
                }
            }
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "what is read, as whom, in what form, where it goes, and what is counted"
    )]
    async fn write_document(
        &self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        wanted: &Wanted,
        archive: &mut Archive,
        tally: &mut Tally,
        thing: &Thing,
        placed: &Placed,
    ) -> Result<(), Stop> {
        let Some(forms) = self.engine.forms() else {
            return Err(forms_missing().into());
        };
        // A package keeps the lossless record and both reading copies beside the same carried files.
        let reading = package_reading(wanted, placed);
        let html = wanted.package.then(|| format!("{}.html", placed.stem));
        let mut written_forms = Vec::new();
        if wanted.package || wanted.format == ExportFormat::Json {
            written_forms.push((placed.path.clone(), DocumentExport::Json));
        }
        if let Some(path) = &reading {
            written_forms.push((path.clone(), DocumentExport::Markdown));
        }
        if let Some(path) = &html {
            written_forms.push((path.clone(), DocumentExport::Html));
        }
        for (path, format) in written_forms {
            let form = forms
                .document(
                    conn,
                    principal,
                    thing.id,
                    DocumentHow {
                        format,
                        files: wanted.files,
                        comments: wanted.comments,
                        beside: written::files_beside(&path),
                    },
                    self.engine.inner.clock.now(),
                )
                .await
                .map_err(|fault| Failure::new(INTERNAL, fault.to_string()))?;
            let what = if form.losses.is_empty() {
                format!("the document {}", thing.title)
            } else {
                format!(
                    "the document {}; as Markdown it could not keep: {}",
                    thing.title, form.losses
                )
            };
            if format == DocumentExport::Json
                && let Some(people) = archive.people.as_mut()
            {
                // The people a record names, for the manifest of the package.
                if let Ok(value) = serde_json::from_slice::<Json>(&form.bytes) {
                    package::people_in(&value, people);
                }
            }
            archive.add(&path, form.bytes).await?;
            tally.listed.push((path, what));
        }
        tally.documents += 1;
        tally.carried.clear();
        if wanted.files {
            let files: Vec<DocumentFile> = rows::document_files(conn, &[thing.id]).await?;
            let directory = files_directory(placed);
            for file in files {
                // The name of a file is the one a link in the text names it by: it is already fit for a path.
                let path = format!("{directory}/{}", file.name);
                let before = tally.carried.len();
                self.copy_blob(archive, tally, &path, &file.blob).await?;
                if tally.carried.len() > before
                    && let Some(carried) = tally.carried.last_mut()
                {
                    carried.id = Some(file.id);
                    carried.name.clone_from(&file.name);
                }
            }
        }
        let mut versions = Vec::new();
        if wanted.versions {
            versions = self
                .write_versions(conn, wanted, archive, tally, thing, placed)
                .await?;
        }
        if wanted.package {
            tally.entries.push(package::Entry {
                kind: orch_api_contract::package::PackageKind::Document,
                id: thing.id,
                parent: thing.node,
                path: Some(placed.path.clone()),
                reading,
                html,
                name: thing.title.clone(),
                files: std::mem::take(&mut tally.carried),
                versions,
                ..package::Entry::default()
            });
        }
        Ok(())
    }

    /// Writes the earlier versions of a document beside it, as records and as reading copies in a package; says
    /// which it wrote, by number, time and the path of the record.
    async fn write_versions(
        &self,
        conn: &mut orch_db::Conn,
        wanted: &Wanted,
        archive: &mut Archive,
        tally: &mut Tally,
        thing: &Thing,
        placed: &Placed,
    ) -> Result<Vec<package::Cut>, Stop> {
        let forms = if wanted.package {
            vec![ExportFormat::Json, ExportFormat::Markdown]
        } else {
            vec![wanted.format]
        };
        let mut cuts = Vec::new();
        for (_, number, cut_at) in rows::version_lines(conn, &[thing.id]).await? {
            let Some(version) = history::get(conn, thing.id, number).await? else {
                continue;
            };
            let Some(stored) = history::snapshot(conn, thing.id, number).await? else {
                continue;
            };
            let blocks: Vec<_> = stored.iter().map(document_text::block_of).collect();
            if let Some(people) = archive.people.as_mut() {
                for author in version
                    .authors
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(orch_documents::versions::Writer::from_json)
                {
                    if let orch_documents::versions::Writer::User(user) = author {
                        people.insert(user);
                    }
                }
            }
            for format in &forms {
                let extension = match format {
                    ExportFormat::Markdown => ".md",
                    ExportFormat::Json => ".json",
                };
                let name = format!(
                    "{number:04} {}{extension}",
                    cut_at.format("%Y-%m-%dT%H-%MZ")
                );
                let path = format!("{}.history/{name}", placed.stem);
                let bytes = match format {
                    ExportFormat::Markdown => {
                        let known =
                            document_text::known_for(conn, &stored, &written::files_beside(&path))
                                .await?;
                        let who = writer_of(conn, &version.authors).await?;
                        let line =
                            markdown::context_line(&version.title, &who, Time::new(version.to_at));
                        let text = document_text::markdown_of(&blocks, &known);
                        if blocks.is_empty() {
                            format!("{line}\n").into_bytes()
                        } else {
                            format!("{line}\n\n{text}\n").into_bytes()
                        }
                    }
                    ExportFormat::Json => serde_json::to_vec(&json!({
                        "version": number,
                        "title": version.title,
                        "cut_at": Time::new(version.to_at),
                        "authors": version.authors,
                        "blocks": blocks,
                    }))
                    .map_err(|error| Failure::new(INTERNAL, error.to_string()))?,
                };
                archive.add(&path, bytes).await?;
                if *format == ExportFormat::Json {
                    cuts.push(package::Cut {
                        number: i64::from(number),
                        cut_at,
                        path: path.clone(),
                    });
                }
                tally.listed.push((
                    path,
                    format!("version {number} of the document {}", thing.title),
                ));
            }
        }
        Ok(cuts)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "what was asked, what was found, where it went, the zip and the count"
    )]
    async fn finish_archive(
        &self,
        job: &Job,
        selected: &Selected,
        wanted: &Wanted,
        request: &ExportRequest,
        layout: &Layout,
        mut archive: Archive,
        tally: Tally,
    ) -> Result<(), Stop> {
        let counts = ExportCounts {
            places: layout.places(),
            conversations: tally.conversations,
            chats: tally.chats,
            documents: tally.documents,
            files: tally.files,
        };
        let left_out: Vec<ExportLeftOut> = selected.left_out_told(&tally.left_out);
        let by = {
            let mut conn = self.engine.inner.db.acquire().await?;
            identity::users_brief(&mut conn, &[self.user])
                .await?
                .into_iter()
                .next()
        };
        let now = self.engine.inner.clock.now();
        let manifest = json!({
            "made_by": by.as_ref().map(|person| json!({
                "id": person.id, "username": person.username, "display_name": person.display_name,
            })),
            "of": { "id": selected.place.id, "kind": selected.place.kind, "name": selected.place.name },
            "at": Time::new(now),
            "options": request,
            "counts": counts,
            "left_out": left_out,
        });
        if wanted.package {
            let manifest = self
                .manifest(
                    selected, request, layout, &archive, &tally, counts, &left_out, now,
                )
                .await?;
            archive
                .add(
                    PACKAGE_MANIFEST,
                    serde_json::to_vec_pretty(&manifest)
                        .map_err(|error| Failure::new(INTERNAL, error.to_string()))?,
                )
                .await?;
        }
        let index = index_text(
            selected,
            by.as_ref().map(|person| person.display_name.as_str()),
            now,
            &tally,
            &counts,
            wanted.package,
        );
        archive.add(INDEX, index.into_bytes()).await?;
        archive
            .add(
                MANIFEST,
                serde_json::to_vec_pretty(&manifest)
                    .map_err(|error| Failure::new(INTERNAL, error.to_string()))?,
            )
            .await?;
        let entries = archive.entries;
        let size = archive.finish().await?;
        job.remember(&json!({
            "scratch": self.scratch_path(job),
            "size": size,
            "entries": entries,
            "counts": counts,
            "left_out": left_out,
        }))
        .await?;
        Ok(())
    }
}

/// The path a thing's reading copy is written at: the Markdown beside its record in a package, the text itself where
/// the archive is made of the written forms, and none where it is made of records alone.
fn package_reading(wanted: &Wanted, placed: &Placed) -> Option<String> {
    if wanted.package {
        Some(format!("{}.md", placed.stem))
    } else if wanted.format == ExportFormat::Markdown {
        Some(placed.path.clone())
    } else {
        None
    }
}

/// The first turn and the last of the days a request narrows a conversation to, where it does.
async fn share_rows_turns(
    conn: &mut orch_db::Conn,
    thread: Id,
    view: Id,
    wanted: &Wanted,
) -> Result<Option<(i64, i64)>, Stop> {
    if wanted.span.from.is_none() && wanted.span.until.is_none() {
        return Ok(None);
    }
    Ok(Some(
        selected::turn_numbers_in(conn, thread, view, wanted.span)
            .await?
            // No turn in those days: a range that holds none.
            .unwrap_or((i64::MAX - 1, i64::MAX)),
    ))
}

/// The name of who changed a document in a version, for the line that heads it.
async fn writer_of(conn: &mut orch_db::Conn, authors: &Json) -> Result<String, Stop> {
    let first = authors
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(orch_documents::versions::Writer::from_json)
        .next();
    Ok(match first {
        Some(orch_documents::versions::Writer::Agent { agent, .. }) => agent,
        Some(orch_documents::versions::Writer::User(user)) => identity::users_brief(conn, &[user])
            .await?
            .into_iter()
            .next()
            .map_or_else(|| "someone".to_owned(), |person| person.display_name),
        None => "Orch".to_owned(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Conversation,
    Chat,
    Document,
}

fn forms_missing() -> Failure {
    Failure::new(
        "unsupported",
        "nothing is connected that writes a document or a thread as its single export does",
    )
}

/// The text of the index: what the archive is, and the files in it with what each is.
fn index_text(
    selected: &Selected,
    by: Option<&str>,
    at: DateTime<Utc>,
    tally: &Tally,
    counts: &ExportCounts,
    package: bool,
) -> String {
    use std::fmt::Write as _;
    let mut text = format!(
        "# {}\n\nAn export of what {} may read here, made {}. It holds {} files, listed here with what each is.\n\n",
        selected.place.name,
        by.unwrap_or("its person"),
        written::minute(Time::new(at)),
        tally.listed.len() + 2
    );
    let _ = writeln!(
        text,
        "It holds {} conversations, {} chats, {} documents and {} files in {} places.\n",
        counts.conversations, counts.chats, counts.documents, counts.files, counts.places
    );
    for (path, what) in &tally.listed {
        let _ = writeln!(text, "- {path} \u{2014} {what}");
    }
    let _ = writeln!(
        text,
        "- {MANIFEST} \u{2014} who made this export, of what, when, with which options, and what it holds"
    );
    if package {
        let _ = writeln!(
            text,
            "- {PACKAGE_MANIFEST} \u{2014} the manifest of this Orch package: every thing in it with its record and its hash, the people it names and what was left out; it can be imported into another place"
        );
    }
    let hidden: i64 = tally
        .left_out
        .iter()
        .filter(|((what, _), _)| *what == NOT_EXPORTABLE)
        .map(|(_, count)| *count)
        .sum();
    if hidden > 0 {
        let _ = write!(
            text,
            "\n{hidden} things you may read here and not take away as files were left out.\n"
        );
    }
    let large: i64 = tally
        .left_out
        .iter()
        .filter(|((what, _), _)| *what == TOO_LARGE)
        .map(|(_, count)| *count)
        .sum();
    if large > 0 {
        let _ = write!(
            text,
            "\n{large} files were larger than an export takes in and were left out.\n"
        );
    }
    let _ = write!(
        text,
        "\nThe files a thing carries lie beside it as `<name>.files/` and its earlier versions as `<name>.history/`; a link in the text of a conversation, a chat or a document names a file by the path it lies at from the text: `<name>.files/…`, `../<name>.files/…` from an earlier version.\n"
    );
    text
}

// ---------------------------------------------------------------- store

impl Exporting {
    async fn store(&self, job: &Job) -> Result<Done, Stop> {
        // A run that was lost after it made the archive ready, and before the job said so, has nothing to do again.
        {
            let mut conn = self.engine.inner.db.acquire().await?;
            let made = rows::get(&mut conn, self.export).await?;
            if made.is_some_and(|row| row.state == rows::state::READY) {
                return Ok(Done::Finished);
            }
        }
        let plan = job.plan().await?;
        let scratch = plan
            .get("scratch")
            .and_then(Json::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| Failure::new(INTERNAL, "the archive was not written"))?;
        let Some(store) = self.engine.blobs() else {
            return Err(Failure::new(
                "unsupported",
                "no store of content is attached, so an archive cannot be kept",
            )
            .into());
        };
        job.check()?;
        let path = scratch.clone();
        let (hash, size) = tokio::task::spawn_blocking(move || hash_file(&path))
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?
            .map_err(|error| scratch_failed(&error))?;
        let file = tokio::fs::File::open(&scratch)
            .await
            .map_err(|error| scratch_failed(&error))?;
        let content: ByteStream = ReaderStream::with_capacity(file, PIECE_BYTES)
            .map(|piece| piece.map_err(|error| BlobError::Io(error.to_string())))
            .boxed();
        let stored = store
            .put(&hash, Some(size), content)
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        if stored.created {
            job.remember(&json!({ "made_blob": hash.to_string() }))
                .await?;
        }
        job.pass_commit().await?;
        let counts = plan.get("counts").cloned().unwrap_or_else(|| json!({}));
        let left_out = plan.get("left_out").cloned().unwrap_or_else(|| json!([]));
        let now = self.engine.inner.clock.now();
        let mut conn = self.engine.inner.db.acquire().await?;
        let keep_hours = self
            .engine
            .export_limits(&mut conn, self.node)
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?
            .keep_hours;
        drop(conn);
        let expires_at = now + chrono::Duration::hours(keep_hours);
        let mut tx = self.engine.inner.db.begin().await?;
        storage::register_blob(&mut tx, &hash, size, now).await?;
        storage::pin_blobs(&mut tx, &[hash]).await?;
        let hash_text = hash.to_string();
        let made = rows::make_ready(
            &mut tx,
            self.export,
            &Written {
                blob: &hash_text,
                bytes: size,
                counts: &counts,
                left_out: &left_out,
                at: now,
                expires_at,
            },
        )
        .await?;
        if !made {
            // It was removed while it was being put away: nothing of it is kept.
            tx.rollback().await.map_err(orch_db::DbError::from)?;
            if stored.created {
                let _ = store.delete(&hash).await;
            }
            let _ = std::fs::remove_file(&scratch);
            return Err(Stop::Cancelled);
        }
        let mut audit = AuditEntry::new(
            Actor::User(self.user),
            AuditAction::ExportCreated.as_str(),
            "export",
            Some(self.export),
            "An archive of what a person may read in a place was written.",
        );
        audit.node_id = Some(self.node);
        audit.details = json!({
            "export_id": self.export,
            "written": { "counts": counts, "bytes": size, "left_out": left_out },
        });
        append(&mut tx, &audit, &Origin::default(), now).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let _ = std::fs::remove_file(&scratch);
        Ok(Done::Finished)
    }
}

// ---------------------------------------------------------------- when it ends

impl Engine {
    /// What follows an archive that was written, or was not: the person who asked is told, with the link to the
    /// archive. One that was cancelled tells nobody.
    pub(crate) async fn after_export_job(&self, row: &JobRow) {
        let Some(creator) = row.created_by else {
            return;
        };
        let Some(export) = text_id(&row.plan, "export") else {
            return;
        };
        let at = self.inner.clock.now();
        let Ok(mut conn) = self.inner.db.acquire().await else {
            return;
        };
        let told =
            crate::notifications::export_finished(&mut conn, row.id, export, creator, at).await;
        if let Err(error) = told {
            tracing::warn!(%error, job = %row.id, "the person who asked for an archive could not be told that it ended");
        }
    }
}

// ---------------------------------------------------------------- a chat with its files (the export of a chat)

impl Engine {
    /// The directory the zips of archives and of chats with their files are written in before they are put in the
    /// store or streamed to a person: the one the program was given, made if it is not there, else the system's
    /// directory for temporary files, which is said once in the log.
    #[must_use]
    pub fn scratch_dir(&self) -> PathBuf {
        let Some(dir) = self.inner.config.scratch.clone() else {
            return std::env::temp_dir();
        };
        match std::fs::create_dir_all(&dir) {
            Ok(()) => dir,
            Err(error) => {
                tracing::warn!(%error, dir = %dir.display(), "the scratch directory of archives cannot be made, so the system's is used");
                std::env::temp_dir()
            }
        }
    }

    /// A chat and the files its messages carry as one zip in the scratch directory: what the export of a chat is
    /// when it is asked for with its files. The caller streams the file and removes it. Says where it is and how
    /// large it is.
    pub async fn chat_with_files(
        &self,
        conn: &mut orch_db::Conn,
        chat: Id,
        format: ExportFormat,
        range: (Option<i64>, Option<i64>),
    ) -> Result<(PathBuf, u64), EngineError> {
        let path = self
            .scratch_dir()
            .join(format!("chat-{}.zip", orch_types::new_id().simple()));
        let outcome = self.chat_zip(conn, chat, format, range, &path).await;
        if outcome.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        outcome.map(|size| (path, size))
    }

    async fn chat_zip(
        &self,
        conn: &mut orch_db::Conn,
        chat: Id,
        format: ExportFormat,
        range: (Option<i64>, Option<i64>),
        path: &Path,
    ) -> Result<u64, EngineError> {
        let failed = |stop: Stop| match stop {
            Stop::Failed(failure) => EngineError::unreadable(failure.message),
            other => EngineError::unreadable(format!("{other:?}")),
        };
        let mut archive =
            Archive::begin(path).map_err(|failure| EngineError::unreadable(failure.message))?;
        let name = match format {
            ExportFormat::Markdown => "chat.md",
            ExportFormat::Json => "chat.json",
        };
        let last = stream_chat(conn, &mut archive, name, chat, format, range, "chat.files")
            .await
            .map_err(failed)?;
        if let Some(span) = written::export_range(range, last) {
            for (low, high) in text_windows(span) {
                for (file, blob) in chat_file_entries(conn, chat, (Some(low), Some(high)))
                    .await
                    .map_err(failed)?
                {
                    copy_blob(self, &mut archive, &format!("chat.files/{file}"), &blob)
                        .await
                        .map_err(failed)?;
                }
            }
        }
        archive
            .finish()
            .await
            .map_err(|failure| EngineError::unreadable(failure.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_zip_is_written_from_pieces_that_wait_a_few_at_a_time() {
        let dir = std::env::temp_dir().join(format!(
            "orch-export-test-{}",
            orch_types::new_id().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("one.zip");
        let mut archive = Archive::begin(&path).unwrap();
        archive.add("a/hello.md", b"hello".to_vec()).await.unwrap();
        let pieces: Vec<Result<Bytes, BlobError>> = vec![
            Ok(Bytes::from_static(b"0123456789")),
            Ok(Bytes::from_static(b"abcdef")),
        ];
        archive
            .add_stream("a/big.bin", futures::stream::iter(pieces).boxed())
            .await
            .unwrap();
        assert!(archive.holds("a/hello.md") && archive.holds("a/big.bin"));
        assert!(!archive.holds("a/other.bin"));
        let size = archive.finish().await.unwrap();
        assert_eq!(size, std::fs::metadata(&path).unwrap().len());
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut hello = String::new();
        zip.by_name("a/hello.md")
            .unwrap()
            .read_to_string(&mut hello)
            .unwrap();
        assert_eq!(hello, "hello");
        let mut big = String::new();
        zip.by_name("a/big.bin")
            .unwrap()
            .read_to_string(&mut big)
            .unwrap();
        assert_eq!(big, "0123456789abcdef");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_files_of_a_text_lie_in_the_directory_beside_it_that_its_links_name() {
        let at = |path: &str| {
            files_directory(&Placed {
                path: path.to_owned(),
                stem: path
                    .rsplit_once('.')
                    .map_or(path, |(stem, _)| stem)
                    .to_owned(),
            })
        };
        assert_eq!(at("Acme/Web/chat.md"), "Acme/Web/chat.files");
        assert_eq!(
            at("Acme/Web/conversations/Fix login.md"),
            "Acme/Web/conversations/Fix login.files"
        );
        assert_eq!(at("Acme/docs/Handbook.json"), "Acme/docs/Handbook.files");
        assert_eq!(at("Notes.md"), "Notes.files");
    }

    #[test]
    fn the_hash_of_a_file_is_the_hash_of_its_bytes_and_its_size_is_its_length() {
        let dir = std::env::temp_dir().join(format!(
            "orch-export-hash-{}",
            orch_types::new_id().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f");
        let bytes = vec![7_u8; PIECE_BYTES * 2 + 5];
        std::fs::write(&path, &bytes).unwrap();
        let (hash, size) = hash_file(&path).unwrap();
        assert_eq!(hash, Hash::of(&bytes));
        assert_eq!(size, i64::try_from(bytes.len()).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
