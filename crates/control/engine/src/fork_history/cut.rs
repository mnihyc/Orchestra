//! Captured full-history consumers, distinct from exact prompt-prefix editing.

use super::{Engine, EngineError};
use crate::{
    history_pages::{Backward, Bundle, Forward, Source},
    views::People,
    written,
};
use futures::StreamExt;
use orch_db::{
    Conn,
    queries::{storage, thread_history_cuts as rows, threads},
};
use orch_link_contract::commands::AgentReadSessionResult;
use orch_types::{Hash, Id, Json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// How many turns are written at a time when a cut is read: enough that the queries of a page are shared by many turns,
/// few enough that a page of written turns is held in memory without notice.
const RENDER_PAGE_TURNS: usize = 25;

/// A captured turn written at one detail, with its line of the index.
struct Rendered {
    /// The turn's number.
    ordinal: i64,
    /// Its line of the index, before the number of the line it begins at.
    preview: String,
    /// Its written form.
    text: crate::history_pages::Text,
}

/// The line of the index that names a turn: its number, when it began, who asked, what was asked and how it ended.
pub(crate) fn preview_of(
    turn: &orch_db::queries::threads::TurnRow,
    author: Option<&str>,
    asked: &str,
) -> String {
    format!(
        "Turn {} · {} · {} · {} · {}",
        turn.ordinal,
        turn.started_at.unwrap_or(turn.created_at).to_rfc3339(),
        author.unwrap_or("Orch"),
        asked
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(written::BRIEF_ASKED_CHARS)
            .collect::<String>(),
        turn.state
    )
}

/// Writes a page of captured turns at `detail`. Both the text and the preview of a turn come from the frozen records; what
/// a page needs beside its turns (the people, the agents, the approvals, the last words of sub-agents) is asked for that
/// page alone, in a few queries, however long the cut is.
async fn render(
    conn: &mut Conn,
    cut: &rows::Cut,
    bundles: Vec<Bundle>,
    detail: written::Detail,
) -> Result<Vec<Rendered>, EngineError> {
    let segment_ids: Vec<Id> = bundles
        .iter()
        .map(|bundle| bundle.turn.segment_id)
        .collect();
    let agents: HashMap<Id, String> = rows::metadata_among(conn, cut.id, "segment", &segment_ids)
        .await?
        .into_iter()
        .map(|body| {
            Ok((
                serde_json::from_value(body["id"].clone())?,
                body["captured_agent_name"]
                    .as_str()
                    .unwrap_or("Agent")
                    .to_owned(),
            ))
        })
        .collect::<Result<_, serde_json::Error>>()?;
    let mut who = BTreeSet::new();
    for bundle in &bundles {
        who.extend(bundle.authors()?);
    }
    let people = People::load(conn, who.iter().copied()).await?;
    let names: HashMap<Id, String> = threads::people(conn, &who.into_iter().collect::<Vec<_>>())
        .await?
        .into_iter()
        .map(|person| (person.id, person.display_name))
        .collect();
    let infos = bundles
        .iter()
        .map(|bundle| bundle.presentation(&people))
        .collect::<Result<Vec<_>, _>>()?;
    let approvals: Vec<Id> = infos
        .iter()
        .flat_map(|info| info.approvals.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let streams: Vec<String> = infos
        .iter()
        .flat_map(|info| info.streams.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut beside = written::Beside::default();
    if !approvals.is_empty() {
        for body in rows::metadata_among(conn, cut.id, "approval", &approvals).await? {
            let id = serde_json::from_value(body["id"].clone())?;
            beside.approvals.insert(
                id,
                written::ApprovalFacts {
                    title: body["title"].as_str().unwrap_or_default().to_owned(),
                    decided_by: body["captured_decided_by"].as_str().map(str::to_owned),
                },
            );
        }
    }
    if !streams.is_empty() {
        // Child streams are part of the captured tool result, never today's suffix.
        let turn_ids: Vec<Id> = bundles.iter().map(|bundle| bundle.turn.id).collect();
        beside.sub_agents = rows::last_sub_messages(conn, cut.id, &turn_ids, &streams).await?;
    }
    let mut out = Vec::with_capacity(bundles.len());
    for (bundle, info) in bundles.iter().zip(&infos) {
        let author = info
            .author
            .and_then(|id| names.get(&id))
            .map(String::as_str);
        out.push(Rendered {
            ordinal: i64::from(bundle.turn.ordinal),
            preview: preview_of(&bundle.turn, author, &info.asked),
            text: bundle.write(
                &people,
                &beside,
                agents
                    .get(&bundle.turn.segment_id)
                    .map_or("Agent", String::as_str),
                detail,
                info,
            )?,
        });
    }
    Ok(out)
}

/// What is kept of a transcript that was written to a file: its place in the store, how large it is and how many turns it
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the size tests read what was kept; the code only keeps it"
    )
)]
pub(crate) struct TranscriptFile {
    /// Its content's hash.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
    /// How many turns it names in its index.
    pub turns: i64,
}

/// What masks the text written from a captured history: the source's redaction, found once for a whole write.
async fn masking(
    conn: &mut Conn,
    cut: &rows::Cut,
) -> Result<orch_chat::redaction::Redactor, EngineError> {
    let mut source = threads::thread(conn, cut.thread_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The captured source is unavailable."))?;
    source.environment_id = serde_json::from_value(cut.head["environment_id"].clone())?;
    source.name = cut.head["name"]
        .as_str()
        .unwrap_or("Conversation")
        .to_owned();
    crate::raw_events::live_redactor(conn, &source, source.environment_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The captured source cannot be redacted."))
}

/// How many bytes of a file are read or written at a time when a transcript is put together.
const COPY_PIECE_BYTES: usize = 64 * 1024;

/// A file of the scratch directory that is gone when it is dropped.
pub(crate) struct Scratch(std::path::PathBuf);

impl Scratch {
    /// A file that is removed, if it is there, when this is dropped.
    pub(crate) fn new(path: std::path::PathBuf) -> Self {
        Self(path)
    }

    /// Where the file is.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn scratch_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::unreadable(format!("the transcript could not be written: {error}"))
}

/// Writes the transcript of the cut to a file, a page of turns at a time, and puts it in the store: an index with a line for
/// each turn, then every turn written in full, each as it was recorded, redacted as the source is. The turns are read
/// oldest first from the frozen records and appended to a file in the scratch directory as they are written; the lines of the
/// index are spooled to a second file, because each names the line its turn begins at; the two are joined as the file
/// is hashed. Only the page being written is held, whatever the length of the cut, and the cut is written once.
pub(crate) async fn transcript_file(
    engine: &Engine,
    cut: &rows::Cut,
    name: &str,
) -> Result<TranscriptFile, EngineError> {
    use std::io::Write as _;
    let directory = engine.scratch_dir();
    std::fs::create_dir_all(&directory).map_err(scratch_error)?;
    let stem = format!(
        "transcript-{}-{}",
        cut.id.simple(),
        name.replace(['.', '/'], "_")
    );
    let body = Scratch::new(directory.join(format!("{stem}.body")));
    let spool = Scratch::new(directory.join(format!("{stem}.index")));
    let whole = Scratch::new(directory.join(format!("{stem}.md")));
    let (redactor, counts) = {
        let mut conn = engine.db().acquire().await?;
        (
            masking(&mut conn, cut).await?,
            rows::counts(&mut conn, cut.id).await?,
        )
    };
    let mask = |text: String| crate::raw_events::mask_text(&redactor, text);
    let mut turns = 0_i64;
    if counts.turns == 0 {
        // Adopted, before-Orch records still belong to the captured history. Do not call a genuinely nonempty cut an
        // empty native prefix.
        let mut file =
            std::io::BufWriter::new(std::fs::File::create(whole.path()).map_err(scratch_error)?);
        if counts.items.is_empty() {
            file.write_all(
                mask(written::empty(&written::Head {
                    name: cut.head["name"].as_str().unwrap_or("Conversation"),
                    agent: cut.head["agent_name"].as_str().unwrap_or("Agent"),
                    state: cut.head["state"].as_str().unwrap_or("retained"),
                }))
                .as_bytes(),
            )
            .map_err(scratch_error)?;
        } else {
            file.write_all(mask("# Before Orch\n\nThese are the retained records from before Orch; no Orch turn is invented for them.\n\n```jsonl\n".to_owned()).as_bytes())
                .map_err(scratch_error)?;
            let mut after = 0;
            loop {
                let page = {
                    let mut conn = engine.db().acquire().await?;
                    rows::items_page(&mut conn, cut.id, after, cut.captured_seq, Some("main"))
                        .await?
                };
                let Some(last) = page.last().map(|item| item.seq_created) else {
                    break;
                };
                after = last;
                for item in page.iter().filter(|item| item.before_orch) {
                    file.write_all(mask(super::neutral_line(item, 0)).as_bytes())
                        .and_then(|()| file.write_all(b"\n"))
                        .map_err(scratch_error)?;
                }
            }
            file.write_all(b"```\n").map_err(scratch_error)?;
        }
        file.flush().map_err(scratch_error)?;
    } else {
        let mut body_file =
            std::io::BufWriter::new(std::fs::File::create(body.path()).map_err(scratch_error)?);
        let mut index_file =
            std::io::BufWriter::new(std::fs::File::create(spool.path()).map_err(scratch_error)?);
        let mut reader = {
            let mut conn = engine.db().acquire().await?;
            Forward::between(
                Source::Cut(cut),
                &mut conn,
                (i64::from(cut.first_turn.max(1)), i64::from(cut.last_turn)),
                false,
            )
            .await?
        };
        // The line of the body each turn begins at, counted from the first.
        let mut offset = 0_usize;
        loop {
            let rendered = {
                let mut conn = engine.db().acquire().await?;
                let bundles = reader.next(&mut conn, RENDER_PAGE_TURNS).await?;
                if bundles.is_empty() {
                    break;
                }
                render(&mut conn, cut, bundles, written::Detail::Full).await?
            };
            for each in rendered {
                let text = crate::raw_events::mask_spooled(&redactor, &each.text)?;
                let preview = mask(each.preview);
                if turns > 0 {
                    body_file.write_all(b"\n").map_err(scratch_error)?;
                }
                text.copy_to(&mut body_file)?;
                writeln!(
                    index_file,
                    "{}",
                    serde_json::to_string(&(offset, preview)).map_err(scratch_error)?
                )
                .map_err(scratch_error)?;
                offset += text.lines() + 1;
                turns += 1;
            }
        }
        body_file.flush().map_err(scratch_error)?;
        index_file.flush().map_err(scratch_error)?;
        drop((body_file, index_file));
        join_transcript(spool.path(), body.path(), whole.path(), turns)?;
    }
    keep_file(engine, cut.id, name, whole.path(), turns).await
}

/// Joins the spooled index and the body into the transcript: the header, a line for each turn that names the line its turn
/// begins at, a blank line and the turns. The bytes are read and written in pieces.
pub(crate) fn join_transcript(
    spool: &std::path::Path,
    body: &std::path::Path,
    whole: &std::path::Path,
    turns: i64,
) -> Result<(), EngineError> {
    use std::io::{BufRead as _, Read as _, Write as _};
    let header = "# Conversation index\n\n";
    // The body begins after the header, a line for each turn, the blank line that follows them and the one that is
    // left blank, as the lines of a text are counted.
    let first_line = header.lines().count() + usize::try_from(turns).unwrap_or(usize::MAX) + 2;
    let mut out = std::io::BufWriter::new(std::fs::File::create(whole).map_err(scratch_error)?);
    out.write_all(header.as_bytes()).map_err(scratch_error)?;
    let index = std::io::BufReader::new(std::fs::File::open(spool).map_err(scratch_error)?);
    let mut first = true;
    for line in index.lines() {
        let (offset, preview): (usize, String) =
            serde_json::from_str(&line.map_err(scratch_error)?).map_err(scratch_error)?;
        if !first {
            out.write_all(b"\n").map_err(scratch_error)?;
        }
        first = false;
        write!(out, "- {preview} · line {}", first_line + offset).map_err(scratch_error)?;
    }
    out.write_all(b"\n\n").map_err(scratch_error)?;
    let mut body = std::fs::File::open(body).map_err(scratch_error)?;
    let mut piece = vec![0_u8; COPY_PIECE_BYTES];
    loop {
        let read = body.read(&mut piece).map_err(scratch_error)?;
        if read == 0 {
            break;
        }
        out.write_all(&piece[..read]).map_err(scratch_error)?;
    }
    out.flush().map_err(scratch_error)
}

/// Puts a file in the store, hashing it and then reading it again, each in pieces: the content is never held. Says what
/// it is called there and how large it is.
pub(crate) async fn put_file(
    engine: &Engine,
    path: &std::path::Path,
) -> Result<(Hash, i64), EngineError> {
    let owned = path.to_path_buf();
    let (hash, size) = tokio::task::spawn_blocking(move || {
        use std::io::Read as _;
        let mut file = std::fs::File::open(&owned)?;
        let mut hasher = Hash::hasher();
        let mut piece = vec![0_u8; COPY_PIECE_BYTES];
        let mut size = 0_u64;
        loop {
            let read = file.read(&mut piece)?;
            if read == 0 {
                break;
            }
            hasher.update(&piece[..read]);
            size += read as u64;
        }
        Ok::<_, std::io::Error>((Hash::finish(&hasher), size))
    })
    .await
    .map_err(scratch_error)?
    .map_err(scratch_error)?;
    let size = i64::try_from(size)
        .map_err(|_| EngineError::unreadable("The history artifact is too large."))?;
    let store = engine
        .blobs()
        .ok_or_else(|| EngineError::unreadable("The history content store is unavailable."))?;
    let file = tokio::fs::File::open(path).await.map_err(scratch_error)?;
    let content: orch_blob_contract::store::ByteStream =
        tokio_util::io::ReaderStream::with_capacity(file, COPY_PIECE_BYTES)
            .map(|piece| {
                piece.map_err(|error| orch_blob_contract::store::BlobError::Io(error.to_string()))
            })
            .boxed();
    store
        .put(&hash, Some(size), content)
        .await
        .map_err(EngineError::unreadable)?;
    Ok((hash, size))
}

/// Puts a file of the scratch directory in the store as an artifact of a cut.
async fn keep_file(
    engine: &Engine,
    cut: Id,
    name: &str,
    path: &std::path::Path,
    turns: i64,
) -> Result<TranscriptFile, EngineError> {
    let (hash, size) = put_file(engine, path).await?;
    let mut tx = engine.db().begin().await?;
    storage::register_blob(&mut tx, &hash, size, engine.inner.clock.now()).await?;
    if !rows::keep_artifact(
        &mut tx,
        cut,
        name,
        &serde_json::json!({"blob":hash,"size":size,"turns":turns}),
        &[hash],
    )
    .await?
    {
        return Err(EngineError::unreadable(
            "Another retained history artifact already owns this stage.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(TranscriptFile {
        blob: hash,
        size,
        turns,
    })
}

/// The latest captured turns whose written form at `detail` fits in `budget` bytes (and no more than `last` of
/// them where it is given), oldest first and redacted as the transcript is: what a switch carries into the new
/// agent's context. A last turn that alone is larger than the budget is carried cut, and says so. The turns are read from
/// the end of the cut a page at a time, and the reading stops at the first one that does not fit, so what this costs
/// follows the budget and not the length of the conversation.
pub(crate) async fn recent(
    conn: &mut Conn,
    cut: &rows::Cut,
    detail: written::Detail,
    budget: Option<usize>,
    last: Option<usize>,
) -> Result<Option<crate::summaries::Carried>, EngineError> {
    let mut reader = Backward::between(
        Source::Cut(cut),
        conn,
        (i64::from(cut.first_turn.max(1)), i64::from(cut.last_turn)),
        i64::from(cut.last_turn),
        false,
    )
    .await?;
    let mut taken: Vec<Rendered> = Vec::new();
    let mut used = 0usize;
    'read: loop {
        let bundles = reader.next(conn, RENDER_PAGE_TURNS).await?;
        if bundles.is_empty() {
            break;
        }
        for each in render(conn, cut, bundles, detail).await? {
            if last.is_some_and(|last| taken.len() >= last) {
                break 'read;
            }
            let size = each.text.len() + 1;
            if budget.is_some_and(|budget| used + size > budget) {
                if taken.is_empty() {
                    taken.push(each);
                }
                break 'read;
            }
            used += size;
            taken.push(each);
        }
    }
    taken.reverse();
    let (Some(first), Some(latest)) = (taken.first(), taken.last()) else {
        return Ok(None);
    };
    let (from, to) = (first.ordinal, latest.ordinal);
    let mut text = String::new();
    let mut clipped = false;
    for each in &taken {
        let room = budget.map(|budget| budget.saturating_sub(text.len()));
        if !text.is_empty() {
            if room == Some(0) {
                clipped = true;
                break;
            }
            text.push('\n');
        }
        let room = budget.map(|budget| budget.saturating_sub(text.len()));
        let part = each.text.read(room)?;
        clipped |= part.len() < each.text.len();
        text.push_str(&part);
        if clipped {
            break;
        }
    }
    if clipped {
        text.push_str("\n\n(This turn is cut here; the rest of it is in the transcript file.)");
    }
    Ok(Some(crate::summaries::Carried {
        from,
        to,
        brief: detail == written::Detail::Brief,
        text: masked(conn, cut, text).await?,
    }))
}

/// The newest captured turns in brief, followed by older index lines, both newest first and within the summary model's
/// share. Reading stops when that share is full; the full transcript remains the file beside the summary.
pub(crate) async fn summary_input(
    engine: &Engine,
    cut: &rows::Cut,
    budget: &crate::summaries::InputBudget,
) -> Result<String, EngineError> {
    let room = budget
        .bytes
        .saturating_sub(crate::summaries::HANDOFF_FRAME_BYTES);
    let mut left = room.saturating_sub(crate::summaries::HANDOFF_HEADINGS_BYTES);
    let brief_room = crate::summaries::brief_bytes(budget).min(left);
    let mut conn = engine.db().acquire().await?;
    let mut reader = Backward::between(
        Source::Cut(cut),
        &mut conn,
        (i64::from(cut.first_turn.max(1)), i64::from(cut.last_turn)),
        i64::from(cut.last_turn),
        false,
    )
    .await?;
    let mut brief = Vec::new();
    let mut older = Vec::new();
    let mut brief_used = 0usize;
    let mut indexing = false;
    'pages: loop {
        if left == 0 {
            break;
        }
        let bundles = reader.next(&mut conn, RENDER_PAGE_TURNS).await?;
        if bundles.is_empty() {
            break;
        }
        for each in render(&mut conn, cut, bundles, written::Detail::Brief).await? {
            let size = each.text.len().saturating_add(1);
            if !indexing && brief_used.saturating_add(size) <= brief_room {
                brief_used += size;
                left = left.saturating_sub(size);
                // Materialize only a turn already proven to fit the summary's remaining budget.
                brief.push(each.text.read(None)?);
                continue;
            }
            indexing = true;
            // Even a single oversized turn leaves its index; its full body is in the transcript file.
            let line = format!("- {}", each.preview);
            let size = line.len().saturating_add(1);
            if size > left {
                break 'pages;
            }
            left -= size;
            older.push(line);
        }
    }
    let total = i64::from(cut.last_turn - cut.first_turn.max(1) + 1).max(0);
    let kept = i64::try_from(brief.len().saturating_add(older.len())).unwrap_or(i64::MAX);
    let left_out = total.saturating_sub(kept);
    let mut text = String::new();
    if !brief.is_empty() {
        text.push_str("# Newest turns, in brief\n");
        text.push_str(&brief.join("\n"));
        text.push_str("\n\n");
    }
    if !older.is_empty() || left_out > 0 {
        text.push_str("# Older turns, one line each");
        if left_out > 0 {
            text.push_str(&format!(" ({left_out} still older are not listed)"));
        }
        text.push('\n');
        text.push_str(&older.join("\n"));
    }
    // Tiny windows can leave less room than the headings themselves.
    let mut end = room.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    masked(&mut conn, cut, text).await
}

/// Text written from a captured history, with what the source's redaction removes taken out.
async fn masked(conn: &mut Conn, cut: &rows::Cut, text: String) -> Result<String, EngineError> {
    let redactor = masking(conn, cut).await?;
    Ok(crate::raw_events::mask_text(&redactor, text))
}

/// Bytes are prepared without holding metadata connections. Registration and an
/// independent cut pin then commit together before any native consumer is issued.
pub(crate) async fn keep_bytes(
    engine: &Engine,
    cut: Id,
    name: &str,
    bytes: Vec<u8>,
) -> Result<(Hash, i64), EngineError> {
    let hash = Hash::of(&bytes);
    let size = i64::try_from(bytes.len())
        .map_err(|_| EngineError::unreadable("The history artifact is too large."))?;
    let store = engine
        .blobs()
        .ok_or_else(|| EngineError::unreadable("The history content store is unavailable."))?;
    store
        .put(
            &hash,
            Some(size),
            orch_blob_contract::store::stream_of(vec![bytes.into()]),
        )
        .await
        .map_err(EngineError::unreadable)?;
    let mut tx = engine.db().begin().await?;
    storage::register_blob(&mut tx, &hash, size, engine.inner.clock.now()).await?;
    if !rows::keep_artifact(
        &mut tx,
        cut,
        name,
        &serde_json::json!({"blob":hash,"size":size}),
        &[hash],
    )
    .await?
    {
        return Err(EngineError::unreadable(
            "Another retained history artifact already owns this stage.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok((hash, size))
}

pub(crate) async fn read_bytes(engine: &Engine, artifact: &Json) -> Result<Vec<u8>, EngineError> {
    let hash: Hash = serde_json::from_value(artifact["blob"].clone())?;
    let size = artifact["size"]
        .as_i64()
        .filter(|size| *size >= 0)
        .ok_or_else(|| EngineError::unreadable("The retained history artifact size is invalid."))?;
    let store = engine
        .blobs()
        .ok_or_else(|| EngineError::unreadable("The history content store is unavailable."))?;
    let mut body = store
        .get(&hash, None)
        .await
        .map_err(EngineError::unreadable)?
        .body;
    let mut bytes = Vec::new();
    while let Some(piece) = body.next().await {
        let piece = piece.map_err(EngineError::unreadable)?;
        if bytes.len().saturating_add(piece.len()) as u64 > size as u64 {
            return Err(EngineError::unreadable(
                "The retained history artifact exceeds its recorded size.",
            ));
        }
        bytes.extend_from_slice(&piece);
    }
    if bytes.len() as u64 != size as u64 || Hash::of(&bytes) != hash {
        return Err(EngineError::unreadable(
            "The retained history artifact failed its recorded identity.",
        ));
    }
    Ok(bytes)
}

/// A native ordinal is derived only from a real recorded native item identity.
/// Global Orch ordinal, row count, text and absent/truncated records are not substitutes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NativeBoundary {
    pub source_segment: Id,
    pub native_ref: String,
    pub ordinal: u32,
    pub witness: String,
    pub record: Hash,
    pub record_size: i64,
    pub record_last: u32,
    pub evidence: NativeRecordEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NativeRecordEvidence {
    turns: i64,
    entries: i64,
    report: orch_agent_contract::history::HistoryReport,
}

impl From<&AgentReadSessionResult> for NativeRecordEvidence {
    fn from(read: &AgentReadSessionResult) -> Self {
        Self {
            turns: read.turns,
            entries: read.entries,
            report: read.report.clone(),
        }
    }
}

mod native;
pub(crate) use native::{native_boundary, verify_native_record};

/// Metadata uses the same bounded publication batches as turns and payloads.
/// All callers pass fixed SQL fragments; cut is always parameter two and the
/// cursor follows that statement's existing values. The primary key supplies
/// both its membership lookup and row-id order without sorting the whole cut.
fn metadata_page_sql(kind: &str, cursor: usize, statement: &str) -> String {
    let limit = cursor + 1;
    let statement = statement.replacen("FROM thread_history_cut_rows r", "FROM page r", 1);
    format!(
        "WITH page AS MATERIALIZED (
      SELECT r.* FROM thread_history_cut_rows r WHERE r.cut_id=$2 AND r.kind='{kind}'
        AND r.row_id>=COALESCE(${cursor}::uuid,'00000000-0000-0000-0000-000000000000'::uuid)
        AND (${cursor}::uuid IS NULL OR r.row_id>${cursor}) ORDER BY r.row_id LIMIT ${limit}
    ),copied AS ({statement}) SELECT row_id FROM page ORDER BY row_id DESC LIMIT 1"
    )
}

/// Copy readable history, not another execution lane or pending queue. Every
/// copied row is owned by the target root. The whole publication is one TX.
pub(crate) async fn copy(
    unit: &mut crate::unit::Unit<'_>,
    cut: &rows::Cut,
    target: Id,
    segment: Id,
    replay_segment: Option<Id>,
) -> Result<(), EngineError> {
    let started = std::time::Instant::now();
    let replay_segment = match (replay_segment, cut.artifacts.get("native_boundary")) {
        (Some(source), Some(boundary))
            if serde_json::from_value::<NativeBoundary>(boundary.clone())?.source_segment
                == source =>
        {
            Some(source)
        }
        _ => None,
    };
    let old: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1
      AND (native_ref LIKE 'fork-copy:%' OR native_ref LIKE 'fork-history:%')) OR EXISTS(
        SELECT 1 FROM turns WHERE thread_id=$1 AND native_turn_ref LIKE 'fork-copy:%')",
    )
    .bind(target)
    .fetch_one(&mut *unit.conn)
    .await
    .map_err(orch_db::DbError::from)?;
    if old {
        return Ok(());
    }
    let owner = threads::thread(unit.conn, target)
        .await?
        .ok_or_else(|| EngineError::unreadable("The fork target is unavailable."))?
        .active_continuation_id;
    // The identity a copied row has in the target is made from the target's and the original's, in the database
    // (`md5(target || original)`), so that no table of all the identities exists to hold or to send: a history of any
    // length copies its turns and items a page at a time. A reference to a row that the cut does not hold
    // is nothing, as it was when the identities were tabled.
    let items_held = rows::item_count(unit.conn, cut.id).await?;
    let copyable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_history_cut_members($1) WHERE kind<>'marker')",
    )
    .bind(cut.id)
    .fetch_one(&mut *unit.conn)
    .await
    .map_err(orch_db::DbError::from)?;
    if !copyable && rows::row_count(unit.conn, cut.id, "turn").await? == 0 {
        return Ok(());
    }
    let stamps = items_held
        .checked_add(1)
        .ok_or_else(|| EngineError::unreadable("The captured history is too large to number."))?;
    let first: i64 = sqlx::query_scalar(
        "UPDATE threads SET next_seq=next_seq+$2 WHERE node_id=$1 RETURNING next_seq-$2",
    )
    .bind(target)
    .bind(stamps)
    .fetch_one(&mut *unit.conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let highest = rows::max_ordinal(unit.conn, cut.id, "segment").await?;
    let next_ordinal = i32::try_from(highest + 1)
        .map_err(|_| EngineError::unreadable("The captured segment order is too large."))?;
    sqlx::query(
        "UPDATE segments SET ordinal=$2 WHERE id=$1 AND thread_id=$3 AND continuation_id=$4",
    )
    .bind(segment)
    .bind(next_ordinal)
    .bind(target)
    .bind(owner)
    .execute(&mut *unit.conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let sql = metadata_page_sql("segment", 6, "INSERT INTO segments SELECT p.* FROM thread_history_cut_rows r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::segments,r.body||jsonb_build_object(
      'id',md5($1::text||r.row_id::text)::uuid,'thread_id',$1::uuid,'continuation_id',$3::uuid,'ordinal',r.ordinal,
      'native_session_ref',NULL,'raw_last_n',0,'ended_at',COALESCE((r.body->>'ended_at')::timestamptz,$4::timestamptz),'seq_updated',$5::bigint)) p
      WHERE r.cut_id=$2 AND r.kind='segment'");
    let mut metadata_after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(target)
            .bind(cut.id)
            .bind(owner)
            .bind(unit.now())
            .bind(first)
            .bind(metadata_after)
            .bind(rows::PAGE)
            .fetch_optional(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let Some(next) = next else { break };
        metadata_after = Some(next);
    }
    tracing::debug!(phase = "segments", elapsed = ?started.elapsed(), "Published history phase");
    let sql = metadata_page_sql("input", 5, "INSERT INTO thread_inputs SELECT p.* FROM thread_history_cut_rows r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::thread_inputs,r.body||jsonb_build_object(
      'id',md5($1::text||r.row_id::text)::uuid,'thread_id',$1::uuid,'continuation_id',$3::uuid,'turn_id',NULL,'client_id',NULL,
      'state',CASE WHEN r.body->>'state'='delivered' THEN 'delivered' ELSE 'cancelled' END,
      'queue_held',false,'state_detail',NULL,'limit_bypass',NULL,'attachment_issues','[]'::jsonb,
      'seq_created',$4::bigint,'seq_updated',$4::bigint)) p
      WHERE r.cut_id=$2 AND r.kind='input'");
    let mut metadata_after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(target)
            .bind(cut.id)
            .bind(owner)
            .bind(first)
            .bind(metadata_after)
            .bind(rows::PAGE)
            .fetch_optional(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let Some(next) = next else { break };
        metadata_after = Some(next);
    }
    tracing::debug!(phase = "inputs", elapsed = ?started.elapsed(), "Published history phase");
    let mut turn_after = None;
    while let Some(last) =
        rows::copy_turns_page(unit.conn, cut.id, target, owner, first, turn_after).await?
    {
        turn_after = Some(last);
    }
    tracing::debug!(phase = "turns", elapsed = ?started.elapsed(), "Published history phase");
    let sql = metadata_page_sql("input", 3, "UPDATE thread_inputs i SET turn_id=CASE WHEN EXISTS(SELECT 1 FROM thread_history_cut_rows t
        WHERE t.cut_id=$2 AND t.kind='turn' AND t.row_id=(r.body->>'turn_id')::uuid)
        THEN md5($1::text||(r.body->>'turn_id'))::uuid END
      FROM thread_history_cut_rows r WHERE r.cut_id=$2 AND r.kind='input' AND i.id=md5($1::text||r.row_id::text)::uuid");
    let mut metadata_after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(target)
            .bind(cut.id)
            .bind(metadata_after)
            .bind(rows::PAGE)
            .fetch_optional(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let Some(next) = next else { break };
        metadata_after = Some(next);
    }
    let mut metadata_after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(
            "WITH page AS MATERIALIZED (
          SELECT r.* FROM thread_history_cut_rows r WHERE r.cut_id=$2 AND r.kind='input'
            AND r.row_id>=COALESCE($3::uuid,'00000000-0000-0000-0000-000000000000'::uuid) AND ($3::uuid IS NULL OR r.row_id>$3) ORDER BY r.row_id LIMIT $4
        ),added AS(INSERT INTO thread_input_blob_refs(input_id,blob)
          SELECT md5($1::text||r.row_id::text)::uuid,decode(b.value,'hex') FROM page r
            CROSS JOIN LATERAL jsonb_array_elements_text(r.body->'captured_blobs') b(value)
          ON CONFLICT DO NOTHING RETURNING blob),counts AS(
          SELECT blob,count(*) AS n FROM added GROUP BY blob),pinned AS(
          UPDATE blobs b SET pin_count=b.pin_count+c.n FROM counts c WHERE b.hash=c.blob)
          SELECT row_id FROM page ORDER BY row_id DESC LIMIT 1",
        )
        .bind(target)
        .bind(cut.id)
        .bind(metadata_after)
        .bind(rows::PAGE)
        .fetch_optional(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        let Some(next) = next else { break };
        metadata_after = Some(next);
    }
    tracing::debug!(phase = "links-and-pins", elapsed = ?started.elapsed(), "Published history phase");
    // Copied payloads receive new positions in frozen order within the same raw reservation.
    // Source markers remain readable at the source and consume no copy payload pages.
    let mut after = 0;
    let mut offset = 0_i64;
    loop {
        let page = rows::copy_item_bodies_page(unit.conn, cut.id, after).await?;
        let Some(last) = page.last() else { break };
        after = last["seq_created"]
            .as_i64()
            .ok_or_else(|| EngineError::unreadable("A captured item position is missing."))?;
        let count = page.len() as i64;
        sqlx::query("INSERT INTO items SELECT p.* FROM jsonb_array_elements($2::jsonb) WITH ORDINALITY b(body,n)
          CROSS JOIN LATERAL jsonb_populate_record(NULL::items,b.body||jsonb_build_object(
          'id',md5($1::text||(b.body->>'id'))::uuid,'thread_id',$1::uuid,'continuation_id',$3::uuid,
          'segment_id',md5($1::text||(b.body->>'segment_id'))::uuid,
          'turn_id',CASE WHEN EXISTS(SELECT 1 FROM thread_history_cut_rows t WHERE t.cut_id=$7 AND t.kind='turn'
            AND t.row_id=(b.body->>'turn_id')::uuid) THEN md5($1::text||(b.body->>'turn_id'))::uuid END,
          'parent_item_id',NULL,'model_input_id',NULL,'pinned_at',NULL,'pinned_by',NULL,'chat_refs',0,'before_orch',false,
          'status',CASE WHEN b.body->>'kind'='approval' AND b.body->>'status'='pending' THEN 'cancelled' ELSE b.body->>'status' END,
          'native_ref',CASE WHEN b.body->>'kind'='user_message' THEN NULL
            WHEN $4::uuid IS NOT NULL AND b.body->>'history_native_alias' IS NOT NULL
              THEN 'fork-history:'||$5::uuid::text||':'||(b.body->>'history_native_alias') ELSE 'fork-copy:'||(b.body->>'id') END,
          'payload',CASE WHEN b.body->>'kind'='user_message' THEN (b.body->'payload')||jsonb_build_object('input_id',COALESCE(
              CASE WHEN EXISTS(SELECT 1 FROM thread_history_cut_rows x WHERE x.cut_id=$7 AND x.kind='input'
                AND x.row_id=(b.body#>>'{payload,input_id}')::uuid) THEN md5($1::text||(b.body#>>'{payload,input_id}'))::uuid::text END,
              b.body#>>'{payload,input_id}'))
            WHEN b.body->>'kind'='approval' THEN ((b.body->'payload')-'approval')||jsonb_build_object('approval_id',
              CASE WHEN EXISTS(SELECT 1 FROM thread_history_cut_rows x WHERE x.cut_id=$7 AND x.kind='approval'
                AND x.row_id=(b.body#>>'{payload,approval_id}')::uuid) THEN md5($1::text||(b.body#>>'{payload,approval_id}'))::uuid::text END)
            ELSE b.body->'payload' END,
          'seq_created',$6::bigint+b.n,'seq_updated',$6::bigint+b.n)) p
          WHERE b.body->>'kind'<>'marker'")
            .bind(target).bind(serde_json::to_value(&page)?).bind(owner)
            .bind(replay_segment).bind(segment).bind(first+offset).bind(cut.id)
            .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        offset += count;
    }
    tracing::debug!(phase = "items", elapsed = ?started.elapsed(), "Published history phase");
    if replay_segment.is_some() {
        sqlx::query("INSERT INTO thread_native_item_aliases(item_id,native_ref)
          SELECT md5($2::text||c.item_id::text)::uuid,c.native_alias FROM thread_history_cut_members($1) c
            WHERE c.kind NOT IN ('user_message','marker') AND c.native_alias IS NOT NULL")
            .bind(cut.id).bind(target).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
    }
    // Parents may occur on a later payload page. Restore them from the same frozen selector,
    // one bounded database page at a time, after every child and parent has been inserted.
    after = 0;
    while let Some(last) = rows::restore_copy_parents_page(unit.conn, cut.id, target, after).await?
    {
        after = last;
    }
    tracing::debug!(phase = "parents", elapsed = ?started.elapsed(), "Published history phase");
    let sql = metadata_page_sql("approval", 5, "INSERT INTO approvals SELECT a.* FROM thread_history_cut_rows r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::approvals,r.body||jsonb_build_object(
      'id',md5($1::text||r.row_id::text)::uuid,'thread_id',$1::uuid,
      'node_id',CASE WHEN r.body->>'node_id'=$4::uuid::text THEN $1::uuid ELSE (r.body->>'node_id')::uuid END,
      'requester_thread_id',CASE WHEN r.body->>'requester_thread_id'=$4::uuid::text THEN $1::uuid ELSE (r.body->>'requester_thread_id')::uuid END,
      'approver_node_id',CASE WHEN r.body->>'approver_node_id'=$4::uuid::text THEN $1::uuid ELSE (r.body->>'approver_node_id')::uuid END,
      'turn_id',CASE WHEN EXISTS(SELECT 1 FROM thread_history_cut_rows t WHERE t.cut_id=$2 AND t.kind='turn'
          AND t.row_id=(r.body->>'turn_id')::uuid) THEN md5($1::text||(r.body->>'turn_id'))::uuid END,
      'item_id',CASE WHEN EXISTS(SELECT 1 FROM thread_history_cut_members($2) p
          WHERE p.item_id=(r.body->>'item_id')::uuid AND p.kind<>'marker') THEN md5($1::text||(r.body->>'item_id'))::uuid END,
      'tool_item_id',CASE WHEN EXISTS(SELECT 1 FROM thread_history_cut_members($2) p
          WHERE p.item_id=(r.body->>'tool_item_id')::uuid AND p.kind<>'marker') THEN md5($1::text||(r.body->>'tool_item_id'))::uuid END,
      'native_ref',NULL,
      'state',CASE WHEN r.body->>'state'='pending' THEN 'cancelled' ELSE r.body->>'state' END,
      'expires_at',NULL,'updated_at',$3::timestamptz)) a
      WHERE r.cut_id=$2 AND r.kind='approval'");
    let mut metadata_after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(target)
            .bind(cut.id)
            .bind(unit.now())
            .bind(cut.thread_id)
            .bind(metadata_after)
            .bind(rows::PAGE)
            .fetch_optional(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let Some(next) = next else { break };
        metadata_after = Some(next);
    }
    tracing::debug!(phase = "approvals", elapsed = ?started.elapsed(), "Published history phase");
    unit.stamp(target).await?;
    Ok(())
}
