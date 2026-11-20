//! Durable local event segments and overflow handling.
//!
//! A machine keeps what it has told the system, and what it has still to
//! tell, in a spool on disk: one directory for each stream, in it
//! append-only files of a few mebibytes. An event is in the spool before it
//! is sent, so that a machine that is stopped in the middle of a sentence
//! finishes it when it starts again.
//!
//! **Numbering.** A stream's events take their numbers when they are put on
//! the wire, in the order they were spooled, and a number, once given, stays
//! with its event however often it is sent. The spool therefore keeps no
//! number with an event: an event's number is its place after the oldest
//! one held. What it does keep, as a small mark, is how far numbering has
//! got, so that a machine that starts again knows which of its events the
//! system may already have seen. Events that were spooled and not yet sent
//! have no number yet, but remain authoritative and are retained until acknowledged.
//!
//! Raw frames have a separate segment number in their body, assigned before append. Its durable
//! allocator is outside the segment files, so overflow, expiration and ACK never reuse a number.
//!
//! **Diagnostics** (`raw.*`) wait in a queue of their own and are put into
//! the stream only when nothing else waits anywhere, so that they never
//! stand in front of an event that matters.
//!
//! **Authoritative events wait for acknowledgement.** Neither age nor pressure discards them,
//! including unsent deltas and their completed items. The configured byte/age allowances shed
//! only waiting diagnostics. Their capture allocator survives shedding, so the next frame's
//! original number exposes the gap; each discarded diagnostic is counted explicitly.
//!
//! The spool is plain, blocking file work and is owned by one task.

use orch_link_contract::streams::is_diagnostic;
use orch_types::{Id, Json, Seq, Time, new_id};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What begins every segment file, so that a file that is not one is not
/// read as one.
const MAGIC: &[u8; 8] = b"ORCHSPL1";
/// The bytes before the first record of a segment: the magic, and the number
/// of the first event it holds.
const SEGMENT_HEADER_BYTES: u64 = 16;
/// The bytes of a record's check: enough to tell a torn or damaged record
/// from a whole one, not meant to stop anybody.
const CHECK_BYTES: usize = 8;
/// The bytes before a record's content: its length, and its check.
const RECORD_HEAD_BYTES: usize = 4 + CHECK_BYTES;
/// A record that holds an event.
const RECORD_EVENT: u8 = 1;
/// A record that says how far numbering has got.
const RECORD_SENT: u8 = 2;
/// A record that says how far the server committed, retained before the local prefix is released.
const RECORD_ACKNOWLEDGED: u8 = 3;
/// A cursor record holds its type byte and one little-endian 64-bit sequence.
const CURSOR_PAYLOAD_BYTES: usize = 1 + 8;
/// The bytes of an event record before its kind: the type, the moment, and
/// the length of the kind.
const EVENT_FIXED_BYTES: usize = 1 + 8 + 2;
/// Milliseconds in a second.
const SECOND_MILLIS: i64 = 1_000;
/// What a stream's directory name is followed by for its diagnostics.
const DIAGNOSTICS_SUFFIX: &str = ".raw";
/// What a stream's directory name is followed by while it is being rewritten.
const REWRITING_SUFFIX: &str = ".rewriting";
/// What a stream's directory name is followed by while the old one waits to
/// be deleted.
const REPLACED_SUFFIX: &str = ".replaced";
/// The file that says a rewritten directory is whole.
const WHOLE_MARK: &str = "WHOLE";
/// A transfer is recorded before either log changes, so startup can finish it without renumbering.
const PROMOTION_FILE: &str = "PROMOTION";
/// The transfer record is published by rename only after its contents are synced.
const PROMOTION_STAGING_FILE: &str = "PROMOTION.new";
/// Capture allocators remain outside stream segment files, which may all be dropped.
const RAW_CURSORS_FILE: &str = "RAW_CURSORS";
/// Publish a new allocator atomically after its contents have reached disk.
const RAW_CURSORS_STAGING_FILE: &str = "RAW_CURSORS.new";
/// Why the spool could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum SpoolError {
    /// A file could not be read or written.
    #[error("spool file {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// What went wrong.
        #[source]
        source: std::io::Error,
    },
    /// The record cannot be represented by its 32-bit stored length/index.
    #[error("a spool record of {bytes} bytes exceeds its representation of {max} bytes")]
    TooLarge {
        /// Its size.
        bytes: u64,
        /// The record representation's limit.
        max: u64,
    },
    /// The events asked for are not held.
    #[error("events from {from} are not held, the oldest is {oldest}")]
    NotHeld {
        /// The first number asked for.
        from: Seq,
        /// The oldest number held.
        oldest: Seq,
    },
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> SpoolError + '_ {
    move |source| SpoolError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// The limits of a spool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpoolOptions {
    /// Waiting diagnostics give way above this allowance; authoritative events are retained.
    pub max_bytes: u64,
    /// Waiting diagnostics expire at this age; authoritative events wait for acknowledgement.
    pub max_age: Duration,
    /// The size of one segment file.
    pub segment_bytes: u64,
}

impl From<&orch_config::connector::Spool> for SpoolOptions {
    fn from(config: &orch_config::connector::Spool) -> Self {
        const SECONDS_PER_DAY: u64 = 86_400;
        Self {
            max_bytes: config.max_bytes,
            max_age: Duration::from_secs(config.max_age_days.saturating_mul(SECONDS_PER_DAY)),
            segment_bytes: config.segment_bytes,
        }
    }
}

/// An event waiting to be spooled.
#[derive(Debug, Clone, PartialEq)]
pub struct SpoolEvent {
    /// When it happened.
    pub at: Time,
    /// Its kind.
    pub kind: String,
    /// Its body.
    pub body: Json,
}

/// An event that has its number.
#[derive(Debug, Clone, PartialEq)]
pub struct Numbered {
    /// Its number in its stream.
    pub seq: Seq,
    /// The event.
    pub event: SpoolEvent,
}

/// Where an event lies, and what is needed to decide about it without
/// reading it.
#[derive(Debug, Clone)]
struct Entry {
    seq: Seq,
    segment: u64,
    offset: u64,
    record_bytes: u32,
    at_millis: i64,
}

#[derive(Debug)]
struct Segment {
    path: PathBuf,
    bytes: u64,
    damaged: bool,
    loss_from: Option<Seq>,
}

/// One append-only series of segments, with an index of the events in it.
#[derive(Debug)]
struct Log {
    dir: PathBuf,
    segments: BTreeMap<u64, Segment>,
    writer: Option<(u64, BufWriter<File>)>,
    entries: VecDeque<Entry>,
    /// The number the first held event has, when events have numbers.
    base: Seq,
    highest: Seq,
    acknowledged: Seq,
    dirty: bool,
    revision: Id,
    /// A failed record must be cut back before another record can follow it.
    rollback: Option<u64>,
    segment_bytes: u64,
}

fn segment_name(id: u64) -> String {
    format!("{id:020}.seg")
}

fn check_of(payload: &[u8]) -> [u8; CHECK_BYTES] {
    let mut check = [0u8; CHECK_BYTES];
    check.copy_from_slice(&blake3::hash(payload).as_bytes()[..CHECK_BYTES]);
    check
}

fn encode_event(event: &SpoolEvent) -> Vec<u8> {
    let kind = event.kind.as_bytes();
    let body = serde_json::to_vec(&event.body).unwrap_or_default();
    let mut payload = Vec::with_capacity(EVENT_FIXED_BYTES + kind.len() + body.len());
    payload.push(RECORD_EVENT);
    payload.extend_from_slice(&event.at.unix_millis().to_le_bytes());
    payload.extend_from_slice(&u16::try_from(kind.len()).unwrap_or(u16::MAX).to_le_bytes());
    payload.extend_from_slice(kind);
    payload.extend_from_slice(&body);
    payload
}

fn decode_event(payload: &[u8]) -> Option<SpoolEvent> {
    if payload.len() < EVENT_FIXED_BYTES || payload[0] != RECORD_EVENT {
        return None;
    }
    let at = i64::from_le_bytes(payload[1..9].try_into().ok()?);
    let kind_len = usize::from(u16::from_le_bytes(payload[9..11].try_into().ok()?));
    let kind =
        std::str::from_utf8(payload.get(EVENT_FIXED_BYTES..EVENT_FIXED_BYTES + kind_len)?).ok()?;
    let body = serde_json::from_slice(payload.get(EVENT_FIXED_BYTES + kind_len..)?).ok()?;
    let moment = chrono::DateTime::from_timestamp_millis(at)?;
    Some(SpoolEvent {
        at: Time::new(moment),
        kind: kind.to_owned(),
        body,
    })
}

fn frame_record(payload: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(RECORD_HEAD_BYTES + payload.len());
    record.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    record.extend_from_slice(&check_of(payload));
    record.extend_from_slice(payload);
    record
}

/// What reading a segment found.
struct Scanned {
    first_seq: Seq,
    entries: Vec<Entry>,
    sent_through: Seq,
    acknowledged_through: Seq,
    raw_last_n: Seq,
    /// Where the last whole record ends.
    good_bytes: u64,
    /// Whether anything after that was damaged.
    damaged: bool,
}

fn scan_segment(path: &Path, id: u64) -> Result<Option<Scanned>, SpoolError> {
    let mut file = File::open(path).map_err(io_error(path))?;
    let mut header = [0u8; SEGMENT_HEADER_BYTES as usize];
    if file.read_exact(&mut header).is_err() || &header[..8] != MAGIC {
        return Ok(None);
    }
    let first_seq = Seq::from_le_bytes(header[8..16].try_into().unwrap_or([0; 8]));
    if first_seq < 1 {
        return Ok(None);
    }
    let length = file.metadata().map_err(io_error(path))?.len();
    let mut reader = std::io::BufReader::new(file);
    let mut at = SEGMENT_HEADER_BYTES;
    let mut entries = Vec::new();
    let mut sent_through = 0;
    let mut acknowledged_through = 0;
    let mut raw_last_n = 0;
    let mut damaged = false;
    while at < length {
        let mut head = [0u8; RECORD_HEAD_BYTES];
        if reader.read_exact(&mut head).is_err() {
            damaged = true;
            break;
        }
        let payload_len = u32::from_le_bytes(head[..4].try_into().unwrap_or([0; 4]));
        if at + RECORD_HEAD_BYTES as u64 + u64::from(payload_len) > length {
            damaged = true;
            break;
        }
        let mut payload = vec![0u8; payload_len as usize];
        if reader.read_exact(&mut payload).is_err() || head[4..] != check_of(&payload) {
            damaged = true;
            break;
        }
        let Some(record_bytes) = payload_len.checked_add(RECORD_HEAD_BYTES as u32) else {
            damaged = true;
            break;
        };
        match payload.first() {
            Some(&RECORD_EVENT) => {
                let Some(event) = decode_event(&payload) else {
                    damaged = true;
                    break;
                };
                if event.kind == "raw.frame" {
                    raw_last_n =
                        raw_last_n.max(event.body.get("n").and_then(Json::as_i64).unwrap_or(0));
                }
                let Some(seq) = first_seq.checked_add(entries.len() as Seq) else {
                    damaged = true;
                    break;
                };
                entries.push(Entry {
                    seq,
                    segment: id,
                    offset: at,
                    record_bytes,
                    at_millis: event.at.unix_millis(),
                });
            }
            Some(&RECORD_SENT) if payload.len() == CURSOR_PAYLOAD_BYTES => {
                sent_through = sent_through.max(Seq::from_le_bytes(
                    payload[1..9].try_into().unwrap_or([0; 8]),
                ));
            }
            Some(&RECORD_ACKNOWLEDGED) if payload.len() == CURSOR_PAYLOAD_BYTES => {
                acknowledged_through = acknowledged_through.max(Seq::from_le_bytes(
                    payload[1..9].try_into().unwrap_or([0; 8]),
                ));
            }
            _ => {
                damaged = true;
                break;
            }
        }
        at += u64::from(record_bytes);
    }
    Ok(Some(Scanned {
        first_seq,
        entries,
        sent_through,
        acknowledged_through,
        raw_last_n,
        good_bytes: at,
        damaged,
    }))
}

impl Log {
    fn new(dir: PathBuf, segment_bytes: u64) -> Self {
        Self {
            dir,
            segments: BTreeMap::new(),
            writer: None,
            entries: VecDeque::new(),
            base: 1,
            highest: 0,
            acknowledged: 0,
            dirty: false,
            revision: new_id(),
            rollback: None,
            segment_bytes,
        }
    }

    fn bytes(&self) -> u64 {
        self.segments.values().map(|segment| segment.bytes).sum()
    }

    fn next_segment_id(&self) -> u64 {
        self.segments.keys().next_back().map_or(1, |last| last + 1)
    }

    fn next_seq(&self) -> Seq {
        self.highest.saturating_add(1)
    }

    fn open_new_segment(&mut self, first_seq: Seq) -> Result<(), SpoolError> {
        self.flush()?;
        // The segment being left is forced to disk now, once, so that it need
        // never be again.
        if let Some((_, writer)) = self.writer.as_ref() {
            writer.get_ref().sync_data().map_err(io_error(&self.dir))?;
        }
        std::fs::create_dir_all(&self.dir).map_err(io_error(&self.dir))?;
        if let Some(parent) = self.dir.parent() {
            sync_directory(parent)?;
        }
        let id = self.next_segment_id();
        let path = self.dir.join(segment_name(id));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(io_error(&path))?;
        let mut header = Vec::with_capacity(SEGMENT_HEADER_BYTES as usize);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&first_seq.to_le_bytes());
        // This id has no accepted records yet. A failed header can be rewritten at the
        // same id, rather than leaving an orphan that makes every retry AlreadyExists.
        file.write_all(&header).map_err(io_error(&path))?;
        // The new file has to be findable after a crash.
        sync_directory(&self.dir)?;
        self.segments.insert(
            id,
            Segment {
                path,
                bytes: SEGMENT_HEADER_BYTES,
                damaged: false,
                loss_from: None,
            },
        );
        self.writer = Some((id, BufWriter::new(file)));
        Ok(())
    }

    /// Appends a framed record; returns the segment and offset it was put
    /// at.
    fn append_record(
        &mut self,
        record: &[u8],
        next_event_seq: Seq,
    ) -> Result<(u64, u64), SpoolError> {
        self.rollback_tail()?;
        let needs_new = match self.writer.as_ref() {
            None => true,
            Some((id, _)) => {
                let bytes = self.segments.get(id).map_or(0, |segment| segment.bytes);
                bytes + record.len() as u64 > self.segment_bytes && bytes > SEGMENT_HEADER_BYTES
            }
        };
        if needs_new {
            self.open_new_segment(next_event_seq)?;
        }
        let Some((id, writer)) = self.writer.as_mut() else {
            return Err(SpoolError::Io {
                path: self.dir.display().to_string(),
                source: std::io::Error::other("no segment to write to"),
            });
        };
        let segment = self.segments.get_mut(id).ok_or_else(|| SpoolError::Io {
            path: self.dir.display().to_string(),
            source: std::io::Error::other("the segment being written is not known"),
        })?;
        let offset = segment.bytes;
        // Flush each whole record to the file, but batch durability separately. That
        // makes a failed write's exact rollback boundary independent of other records.
        let written = writer.write_all(record).and_then(|()| writer.flush());
        if let Err(source) = written {
            let path = segment.path.clone();
            let id = *id;
            let (_, writer) = self.writer.take().ok_or_else(|| SpoolError::Io {
                path: path.display().to_string(),
                source: std::io::Error::other("the failed writer was not retained"),
            })?;
            // into_parts never flushes a failed buffer again during Drop.
            let (file, _) = writer.into_parts();
            self.writer = Some((id, BufWriter::new(file)));
            self.rollback = Some(offset);
            let _ = self.rollback_tail();
            return Err(io_error(&path)(source));
        }
        segment.bytes += record.len() as u64;
        self.dirty = true;
        self.revision = new_id();
        Ok((*id, offset))
    }

    fn rollback_tail(&mut self) -> Result<(), SpoolError> {
        let Some(offset) = self.rollback else {
            return Ok(());
        };
        let Some((id, writer)) = self.writer.as_mut() else {
            return Err(io_error(&self.dir)(std::io::Error::other(
                "the failed record has no rollback writer",
            )));
        };
        let path = self
            .segments
            .get(id)
            .map_or(&self.dir, |segment| &segment.path);
        writer.get_mut().set_len(offset).map_err(io_error(path))?;
        writer
            .get_mut()
            .seek(SeekFrom::Start(offset))
            .map_err(io_error(path))?;
        self.rollback = None;
        self.dirty = true;
        self.revision = new_id();
        Ok(())
    }

    fn append_event(&mut self, event: &SpoolEvent) -> Result<(), SpoolError> {
        self.drop_front(0)?;
        let payload = encode_event(event);
        let total = payload.len().saturating_add(RECORD_HEAD_BYTES);
        let record_bytes = u32::try_from(total).map_err(|_| SpoolError::TooLarge {
            bytes: total as u64,
            max: u64::from(u32::MAX),
        })?;
        let record = frame_record(&payload);
        let next_seq = self.highest.checked_add(1).ok_or_else(|| {
            io_error(&self.dir)(std::io::Error::other(
                "the delivery sequence allocator is exhausted",
            ))
        })?;
        let (segment, offset) = self.append_record(&record, next_seq)?;
        self.entries.push_back(Entry {
            seq: next_seq,
            segment,
            offset,
            record_bytes,
            at_millis: event.at.unix_millis(),
        });
        self.highest = next_seq;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), SpoolError> {
        self.rollback_tail()?;
        if let Some((id, writer)) = self.writer.as_mut() {
            let path = self
                .segments
                .get(id)
                .map_or_else(|| self.dir.clone(), |segment| segment.path.clone());
            writer.flush().map_err(io_error(&path))?;
        }
        Ok(())
    }

    /// A handle on the file being written, to sync it elsewhere.
    fn sync_handle(&mut self) -> Result<Option<File>, SpoolError> {
        if !self.dirty {
            return Ok(None);
        }
        self.flush()?;
        match self.writer.as_ref() {
            Some((_, writer)) => writer
                .get_ref()
                .try_clone()
                .map(Some)
                .map_err(io_error(&self.dir)),
            None => Ok(None),
        }
    }

    fn sync(&mut self) -> Result<(), SpoolError> {
        if let Some(file) = self.sync_handle()? {
            file.sync_all().map_err(io_error(&self.dir))?;
        }
        self.dirty = false;
        Ok(())
    }

    fn read_entry(&self, entry: &Entry) -> Result<SpoolEvent, SpoolError> {
        let segment = self
            .segments
            .get(&entry.segment)
            .ok_or_else(|| SpoolError::Io {
                path: self.dir.display().to_string(),
                source: std::io::Error::other("a segment is missing"),
            })?;
        let mut file = File::open(&segment.path).map_err(io_error(&segment.path))?;
        file.seek(SeekFrom::Start(entry.offset))
            .map_err(io_error(&segment.path))?;
        let mut head = [0_u8; RECORD_HEAD_BYTES];
        file.read_exact(&mut head)
            .map_err(io_error(&segment.path))?;
        let mut payload = vec![0u8; entry.record_bytes as usize - RECORD_HEAD_BYTES];
        file.read_exact(&mut payload)
            .map_err(io_error(&segment.path))?;
        if u32::from_le_bytes(head[..4].try_into().unwrap_or([0; 4])) as usize != payload.len()
            || head[4..] != check_of(&payload)
        {
            return Err(io_error(&segment.path)(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "a retained record failed its integrity check",
            )));
        }
        decode_event(&payload).ok_or_else(|| SpoolError::Io {
            path: segment.path.display().to_string(),
            source: std::io::Error::other("a record is damaged"),
        })
    }

    /// Forgets the first `count` events, and deletes the segments that have
    /// none left, except the one being written: that one holds the marks of
    /// how far numbering got, and is replaced by the next one when it is
    /// full.
    fn drop_front(&mut self, count: usize) -> Result<(), SpoolError> {
        let count = count.min(self.entries.len());
        self.entries.drain(..count);
        self.base = self
            .entries
            .front()
            .map_or(self.next_seq(), |entry| entry.seq);
        let first_kept = self.entries.front().map(|entry| entry.segment);
        let active = self.writer.as_ref().map(|(id, _)| *id);
        let deletable: Vec<u64> = self
            .segments
            .keys()
            .copied()
            .filter(|id| {
                Some(*id) != active
                    && first_kept.is_none_or(|first| *id < first)
                    && self.segments.get(id).is_none_or(|segment| {
                        segment
                            .loss_from
                            .is_none_or(|from| self.acknowledged >= from)
                    })
            })
            .collect();
        for id in deletable {
            if let Some(segment) = self.segments.get(&id) {
                match std::fs::remove_file(&segment.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(io_error(&segment.path)(error)),
                }
                // Remove the prefix in durable order: a crash must not resurrect a noncontiguous
                // subset of old segments and make recovery count past a still-held event.
                if let Ok(directory) = File::open(&self.dir) {
                    directory.sync_all().map_err(io_error(&self.dir))?;
                }
            }
            self.segments.remove(&id);
        }
        Ok(())
    }

    fn consume_diagnostics(&mut self, count: usize) -> Result<(), SpoolError> {
        let count = count.min(self.entries.len());
        if count == 0 {
            return Ok(());
        }
        let through = self.entries[count - 1].seq;
        let record = frame_record(&sent_payload(through));
        let next = self.next_seq();
        self.append_record(&record, next)?;
        self.sync()?;
        if let Ok(directory) = File::open(&self.dir) {
            directory.sync_all().map_err(io_error(&self.dir))?;
        }
        self.acknowledged = through;
        self.drop_front(count)
    }

    /// Deletes everything, numbering included. For a queue that has none.
    fn discard_all(&mut self) -> Result<(), SpoolError> {
        self.writer = None;
        for (_, segment) in std::mem::take(&mut self.segments) {
            std::fs::remove_file(&segment.path).map_err(io_error(&segment.path))?;
        }
        self.entries.clear();
        self.highest = self.base - 1;
        self.dirty = false;
        self.rollback = None;
        let _ = std::fs::remove_dir(&self.dir);
        Ok(())
    }
}

/// What was taken out of a full spool, so that the system can be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relief {
    /// Diagnostics were dropped.
    Diagnostics {
        /// How many events.
        events: usize,
    },
}

/// The state of one stream's events.
#[derive(Debug)]
struct StreamLog {
    main: Log,
    diagnostics: Log,
    /// The number of the last event put on the wire.
    sent_through: Seq,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Promotion {
    main_from: Seq,
    diagnostic_from: Seq,
    count: usize,
}

impl StreamLog {
    fn finish_pending_promotion(&mut self) -> Result<Option<usize>, SpoolError> {
        let path = self.main.dir.join(PROMOTION_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let file = File::open(&path).map_err(io_error(&path))?;
        let plan: Promotion = serde_json::from_reader(file)
            .map_err(|error| io_error(&path)(std::io::Error::other(error)))?;
        self.finish_promotion(&plan)?;
        Ok(Some(plan.count))
    }

    fn unsent(&self) -> usize {
        self.main
            .entries
            .iter()
            .filter(|entry| entry.seq > self.sent_through)
            .count()
    }

    fn finish_promotion(&mut self, plan: &Promotion) -> Result<(), SpoolError> {
        let diagnostic_end = plan.diagnostic_from + plan.count as Seq;
        if self.diagnostics.base < diagnostic_end {
            let acknowledged = self
                .main
                .acknowledged
                .saturating_sub(plan.main_from)
                .saturating_add(1)
                .max(0)
                .min(plan.count as Seq);
            let copied = acknowledged
                + self
                    .main
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.seq >= plan.main_from && entry.seq > self.main.acknowledged
                    })
                    .count() as Seq;
            if self.diagnostics.base != plan.diagnostic_from
                || self.diagnostics.entries.len() < plan.count
                || copied < 0
                || copied > plan.count as Seq
            {
                return Err(SpoolError::Io {
                    path: self.main.dir.display().to_string(),
                    source: std::io::Error::other(
                        "an interrupted diagnostic transfer cannot be placed",
                    ),
                });
            }
            let remaining: Vec<_> = self
                .diagnostics
                .entries
                .iter()
                .take(plan.count)
                .skip(copied as usize)
                .map(|entry| self.diagnostics.read_entry(entry))
                .collect::<Result<_, _>>()?;
            for event in &remaining {
                self.main.append_event(event)?;
            }
            // The copied frames must be durable before their original queue records are consumed.
            self.main.sync()?;
            if let Ok(directory) = File::open(&self.main.dir) {
                directory.sync_all().map_err(io_error(&self.main.dir))?;
            }
            self.diagnostics.consume_diagnostics(plan.count)?;
        }
        let path = self.main.dir.join(PROMOTION_FILE);
        std::fs::remove_file(&path).map_err(io_error(&path))?;
        if let Ok(directory) = File::open(&self.main.dir) {
            directory.sync_all().map_err(io_error(&self.main.dir))?;
        }
        Ok(())
    }

    fn promote(&mut self, count: usize) -> Result<usize, SpoolError> {
        if let Some(done) = self.finish_pending_promotion()? {
            return Ok(done);
        }
        // Sync both the source and the earlier main-log tail before publishing the transfer's place.
        for log in [&mut self.main, &mut self.diagnostics] {
            log.sync()?;
            if let Ok(directory) = File::open(&log.dir) {
                directory.sync_all().map_err(io_error(&log.dir))?;
            }
        }
        std::fs::create_dir_all(&self.main.dir).map_err(io_error(&self.main.dir))?;
        if let Some(parent) = self.main.dir.parent()
            && let Ok(directory) = File::open(parent)
        {
            directory.sync_all().map_err(io_error(parent))?;
        }
        let plan = Promotion {
            main_from: self.main.next_seq(),
            diagnostic_from: self.diagnostics.base,
            count,
        };
        let staging = self.main.dir.join(PROMOTION_STAGING_FILE);
        let mut file = File::create(&staging).map_err(io_error(&staging))?;
        serde_json::to_writer(&mut file, &plan).map_err(|error| SpoolError::Io {
            path: staging.display().to_string(),
            source: std::io::Error::other(error),
        })?;
        file.sync_all().map_err(io_error(&staging))?;
        let path = self.main.dir.join(PROMOTION_FILE);
        std::fs::rename(&staging, &path).map_err(io_error(&path))?;
        if let Ok(directory) = File::open(&self.main.dir) {
            directory.sync_all().map_err(io_error(&self.main.dir))?;
        }
        self.finish_promotion(&plan)?;
        Ok(count)
    }
}

/// What a spool found when it was opened.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recovery {
    /// The streams it holds events of.
    pub streams: Vec<String>,
    /// The streams in which damage cost events that had been sent.
    pub damaged: Vec<String>,
}

/// The spool of a machine.
#[derive(Debug)]
pub struct Spool {
    root: PathBuf,
    metadata_root: PathBuf,
    options: SpoolOptions,
    streams: BTreeMap<String, StreamLog>,
    raw_cursors: BTreeMap<String, Seq>,
    raw_dirty: bool,
}

/// File handles and the exact append revisions whose durability they prove.
#[derive(Debug)]
pub struct SyncBatch {
    files: Vec<File>,
    receipts: Vec<(String, bool, Id)>,
}
impl SyncBatch {
    /// Forces every file. A failure acknowledges none of the receipts.
    pub fn force(&self) -> std::io::Result<()> {
        for file in &self.files {
            file.sync_all()?;
        }
        Ok(())
    }
    /// Whether this batch needs any filesystem work.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

fn sync_directory(path: &Path) -> Result<(), SpoolError> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error(path))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The directory name of a stream: its name, with the colons that some file
/// systems refuse written as underscores. A stream name never holds an
/// underscore, so the two are the same name.
fn dir_name(stream: &str) -> String {
    stream.replace(':', "_")
}

fn stream_name(dir: &str) -> String {
    dir.replace('_', ":")
}

impl Spool {
    /// Opens the spool in `root`, reading what an earlier run left.
    pub fn open(root: &Path, options: SpoolOptions) -> Result<(Self, Recovery), SpoolError> {
        Self::open_at(root, root, options)
    }

    /// Opens a machine's spool with capture allocators kept separately from its disposable bytes.
    pub fn open_for_machine(
        data_dir: &Path,
        options: SpoolOptions,
    ) -> Result<(Self, Recovery), SpoolError> {
        Self::open_at(
            &data_dir.join("spool"),
            &data_dir.join("spool-meta"),
            options,
        )
    }

    fn open_at(
        root: &Path,
        metadata_root: &Path,
        options: SpoolOptions,
    ) -> Result<(Self, Recovery), SpoolError> {
        std::fs::create_dir_all(root).map_err(io_error(root))?;
        std::fs::create_dir_all(metadata_root).map_err(io_error(metadata_root))?;
        for directory in [root, metadata_root] {
            if let Some(parent) = directory.parent() {
                sync_directory(parent)?;
            }
        }
        let raw_path = metadata_root.join(RAW_CURSORS_FILE);
        let raw_cursors = if raw_path.exists() {
            let file = File::open(&raw_path).map_err(io_error(&raw_path))?;
            serde_json::from_reader(file).map_err(|error| SpoolError::Io {
                path: raw_path.display().to_string(),
                source: std::io::Error::other(error),
            })?
        } else {
            BTreeMap::new()
        };
        let mut spool = Self {
            root: root.to_owned(),
            metadata_root: metadata_root.to_owned(),
            options,
            streams: BTreeMap::new(),
            raw_cursors,
            raw_dirty: false,
        };
        let mut dirs: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(root).map_err(io_error(root))? {
            let entry = entry.map_err(io_error(root))?;
            if entry.file_type().map_err(io_error(root))?.is_dir() {
                dirs.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        // Finish or undo a rewrite that was cut off.
        for dir in &dirs {
            if let Some(name) = dir.strip_suffix(REWRITING_SUFFIX) {
                let staged = root.join(dir);
                let target = root.join(name);
                let replaced = root.join(format!("{name}{REPLACED_SUFFIX}"));
                if !target.exists() && staged.join(WHOLE_MARK).exists() {
                    std::fs::rename(&staged, &target).map_err(io_error(&target))?;
                } else if !target.exists() && replaced.exists() {
                    std::fs::rename(&replaced, &target).map_err(io_error(&target))?;
                    std::fs::remove_dir_all(&staged).map_err(io_error(&staged))?;
                } else {
                    std::fs::remove_dir_all(&staged).map_err(io_error(&staged))?;
                }
            }
        }
        for dir in &dirs {
            if let Some(name) = dir.strip_suffix(REPLACED_SUFFIX) {
                let replaced = root.join(dir);
                let target = root.join(name);
                if !replaced.exists() {
                    continue;
                }
                if target.exists() {
                    std::fs::remove_dir_all(&replaced).map_err(io_error(&replaced))?;
                } else {
                    std::fs::rename(&replaced, &target).map_err(io_error(&target))?;
                }
            }
        }
        let mut names = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(root).map_err(io_error(root))? {
            let entry = entry.map_err(io_error(root))?;
            if !entry.file_type().map_err(io_error(root))?.is_dir() {
                continue;
            }
            let dir = entry.file_name().to_string_lossy().into_owned();
            let stem = dir.strip_suffix(DIAGNOSTICS_SUFFIX).unwrap_or(&dir);
            names.insert(stream_name(stem));
        }
        let mut recovery = Recovery::default();
        for name in names {
            let (log, damaged) = spool.recover_stream(&name)?;
            if damaged {
                recovery.damaged.push(name.clone());
            }
            recovery.streams.push(name.clone());
            spool.streams.insert(name, log);
        }
        Ok((spool, recovery))
    }

    fn stream_logs(&self, name: &str) -> (Log, Log) {
        let dir = self.root.join(dir_name(name));
        let diagnostics = self
            .root
            .join(format!("{}{DIAGNOSTICS_SUFFIX}", dir_name(name)));
        (
            Log::new(dir, self.options.segment_bytes),
            Log::new(diagnostics, self.options.segment_bytes),
        )
    }

    fn recover_stream(&mut self, name: &str) -> Result<(StreamLog, bool), SpoolError> {
        let (mut main, mut diagnostics) = self.stream_logs(name);
        let mut damaged = false;
        let mut sent_through = 0;
        let mut acknowledged_through = 0;
        let mut consumed_through = 0;
        let mut raw_last_n = 0;
        for (log, is_main) in [(&mut main, true), (&mut diagnostics, false)] {
            let mut base_set = false;
            let mut files: Vec<(u64, PathBuf)> = Vec::new();
            if log.dir.exists() {
                for entry in std::fs::read_dir(&log.dir).map_err(io_error(&log.dir))? {
                    let entry = entry.map_err(io_error(&log.dir))?;
                    let file_name = entry.file_name().to_string_lossy().into_owned();
                    if let Some(id) = file_name
                        .strip_suffix(".seg")
                        .and_then(|id| id.parse().ok())
                    {
                        files.push((id, entry.path()));
                    }
                }
            }
            files.sort();
            for (id, path) in files {
                let expected = log.next_seq();
                let Some(scanned) = scan_segment(&path, id)? else {
                    damaged = true;
                    let bytes = std::fs::metadata(&path).map_err(io_error(&path))?.len();
                    log.segments.insert(
                        id,
                        Segment {
                            path,
                            bytes,
                            damaged: true,
                            loss_from: Some(expected),
                        },
                    );
                    log.highest = log.highest.max(expected);
                    continue;
                };
                let gap = base_set && scanned.first_seq > expected;
                let mut loss_from = if gap {
                    Some(expected)
                } else if scanned.damaged {
                    Some(
                        scanned
                            .first_seq
                            .saturating_add(scanned.entries.len() as Seq),
                    )
                } else {
                    None
                };
                damaged |= loss_from.is_some();
                raw_last_n = raw_last_n.max(scanned.raw_last_n);
                if !base_set {
                    log.base = scanned.first_seq;
                    base_set = true;
                }
                if is_main {
                    sent_through = sent_through.max(scanned.sent_through);
                    acknowledged_through = acknowledged_through.max(scanned.acknowledged_through);
                } else {
                    consumed_through = consumed_through.max(scanned.sent_through);
                }
                log.highest = log
                    .highest
                    .max(scanned.first_seq - 1)
                    .max(scanned.sent_through)
                    .max(scanned.acknowledged_through);
                let mut overlap = false;
                for entry in scanned.entries {
                    log.highest = log.highest.max(entry.seq);
                    if log.entries.back().is_none_or(|last| last.seq < entry.seq) {
                        log.entries.push_back(entry);
                    } else {
                        damaged = true;
                        overlap = true;
                        loss_from = Some(loss_from.map_or(entry.seq, |from| from.min(entry.seq)));
                    }
                }
                log.highest = log.highest.max(loss_from.unwrap_or(0));
                let bytes = if scanned.damaged {
                    std::fs::metadata(&path).map_err(io_error(&path))?.len()
                } else {
                    scanned.good_bytes
                };
                log.segments.insert(
                    id,
                    Segment {
                        path,
                        bytes,
                        damaged: scanned.damaged || overlap,
                        loss_from,
                    },
                );
            }
            log.base = log
                .entries
                .front()
                .map_or(log.next_seq(), |entry| entry.seq);
            // Writing goes on at the end of the last segment.
            if let Some((&id, segment)) = log
                .segments
                .iter()
                .next_back()
                .filter(|(_, segment)| !segment.damaged)
            {
                let file = OpenOptions::new()
                    .append(true)
                    .open(&segment.path)
                    .map_err(io_error(&segment.path))?;
                log.writer = Some((id, BufWriter::new(file)));
            }
        }
        diagnostics.acknowledged = consumed_through;
        let consumed = diagnostics
            .entries
            .iter()
            .take_while(|entry| entry.seq <= consumed_through)
            .count();
        diagnostics.drop_front(consumed)?;
        main.acknowledged = acknowledged_through;
        let acknowledged = main
            .entries
            .iter()
            .take_while(|entry| entry.seq <= acknowledged_through)
            .count();
        main.drop_front(acknowledged)?;
        // Everything before the oldest held event was acknowledged or
        // dropped, so numbering has got at least that far.
        sent_through = sent_through.max(main.base - 1);
        let mut log = StreamLog {
            main,
            diagnostics,
            sent_through,
        };
        let path = log.main.dir.join(PROMOTION_FILE);
        if path.exists() {
            let file = File::open(&path).map_err(io_error(&path))?;
            let plan = serde_json::from_reader(file).map_err(|error| SpoolError::Io {
                path: path.display().to_string(),
                source: std::io::Error::other(error),
            })?;
            log.finish_promotion(&plan)?;
        }
        self.raw_after(name, raw_last_n)?;
        Ok((log, damaged))
    }

    fn stream_mut(&mut self, name: &str) -> &mut StreamLog {
        if !self.streams.contains_key(name) {
            let (main, diagnostics) = self.stream_logs(name);
            self.streams.insert(
                name.to_owned(),
                StreamLog {
                    main,
                    diagnostics,
                    sent_through: 0,
                },
            );
        }
        self.streams
            .get_mut(name)
            .unwrap_or_else(|| unreachable!("the stream was just inserted"))
    }

    /// The streams that hold events.
    #[must_use]
    pub fn streams(&self) -> Vec<String> {
        self.streams.keys().cloned().collect()
    }

    /// The number the first held event has, or the number the next would
    /// have when none is held.
    #[must_use]
    pub fn oldest(&self, stream: &str) -> Seq {
        self.streams.get(stream).map_or(1, |log| log.main.base)
    }

    /// The next retained logical position at or after `from`, including unnumbered events.
    #[must_use]
    pub fn next_available(&self, stream: &str, from: Seq) -> Option<Seq> {
        self.streams
            .get(stream)?
            .main
            .entries
            .iter()
            .find(|entry| entry.seq >= from)
            .map(|entry| entry.seq)
    }

    /// The allocator floor survives a hole in an earlier segment.
    #[must_use]
    pub fn next_sequence(&self, stream: &str) -> Seq {
        self.streams
            .get(stream)
            .map_or(1, |log| log.main.next_seq())
    }

    /// Earliest retained uncertainty not yet covered by a server acknowledgement.
    #[must_use]
    pub fn damage_floor(&self, stream: &str) -> Option<Seq> {
        let log = &self.streams.get(stream)?.main;
        log.segments
            .values()
            .filter_map(|segment| segment.loss_from)
            .filter(|from| *from > log.acknowledged)
            .min()
    }

    /// Advances only missing positions. The sender reports this as a gap before later data.
    pub fn advance_gap(&mut self, stream: &str, next: Seq) -> Result<(), SpoolError> {
        let Some(log) = self.streams.get_mut(stream) else {
            return Ok(());
        };
        let through = next.saturating_sub(1);
        if through > log.sent_through {
            let mark = frame_record(&sent_payload(through));
            log.main.append_record(&mark, log.main.next_seq())?;
            log.sent_through = through;
            log.main.highest = log.main.highest.max(through);
        }
        Ok(())
    }

    /// The number of the last event that was put on the wire; 0 when
    /// none was.
    #[must_use]
    pub fn sent_through(&self, stream: &str) -> Seq {
        self.streams.get(stream).map_or(0, |log| log.sent_through)
    }

    /// How many events of a stream wait to be numbered and sent.
    #[must_use]
    pub fn unsent(&self, stream: &str) -> usize {
        self.streams.get(stream).map_or(0, StreamLog::unsent)
    }

    /// How many diagnostics of a stream wait.
    #[must_use]
    pub fn diagnostics_waiting(&self, stream: &str) -> usize {
        self.streams
            .get(stream)
            .map_or(0, |log| log.diagnostics.entries.len())
    }

    /// Whether an event of any stream waits to be numbered and sent.
    #[must_use]
    pub fn anything_unsent(&self) -> bool {
        self.streams.values().any(|log| log.unsent() > 0)
    }

    /// How many bytes the spool holds in all.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.streams
            .values()
            .map(|log| log.main.bytes() + log.diagnostics.bytes())
            .sum()
    }

    /// Raises the floor for newly captured raw frames, without changing already spooled numbers.
    pub fn raw_after(&mut self, stream: &str, after: Seq) -> Result<(), SpoolError> {
        let previous = self.raw_cursors.get(stream).copied().unwrap_or(0);
        if after > previous {
            self.raw_cursors.insert(stream.to_owned(), after);
            self.raw_dirty = true;
        }
        if self.raw_dirty {
            self.persist_raw_cursors()?;
            self.raw_dirty = false;
        }
        Ok(())
    }

    fn persist_raw_cursors(&self) -> Result<(), SpoolError> {
        let staging = self.metadata_root.join(RAW_CURSORS_STAGING_FILE);
        let mut file = File::create(&staging).map_err(io_error(&staging))?;
        serde_json::to_writer(&mut file, &self.raw_cursors).map_err(|error| SpoolError::Io {
            path: staging.display().to_string(),
            source: std::io::Error::other(error),
        })?;
        // Allocate durably before appending the frame. An interrupted capture can leave a gap,
        // but even if every segment file is later lost the next capture cannot reuse its number.
        file.sync_all().map_err(io_error(&staging))?;
        let path = self.metadata_root.join(RAW_CURSORS_FILE);
        std::fs::rename(&staging, &path).map_err(io_error(&path))?;
        if let Ok(directory) = File::open(&self.metadata_root) {
            directory
                .sync_all()
                .map_err(io_error(&self.metadata_root))?;
        }
        if let Some(parent) = self.metadata_root.parent()
            && let Ok(directory) = File::open(parent)
        {
            directory.sync_all().map_err(io_error(parent))?;
        }
        Ok(())
    }

    fn captured(&mut self, stream: &str, event: &SpoolEvent) -> Result<SpoolEvent, SpoolError> {
        let n = self.reserve_capture(stream, event)?;
        let mut captured = event.clone();
        let body = captured
            .body
            .as_object_mut()
            .ok_or_else(|| SpoolError::Io {
                path: self.root.display().to_string(),
                source: std::io::Error::other("a raw frame has no object body"),
            })?;
        body.insert("n".to_owned(), Json::from(n));
        Ok(captured)
    }

    fn reserve_capture(&mut self, stream: &str, event: &SpoolEvent) -> Result<Seq, SpoolError> {
        let last = self.raw_cursors.get(stream).copied().unwrap_or(0);
        let n = match event.body.get("n").and_then(Json::as_i64) {
            Some(n) if n > 0 => n,
            _ => last.checked_add(1).ok_or_else(|| SpoolError::Io {
                path: self.root.display().to_string(),
                source: std::io::Error::other("the raw-frame allocator is exhausted"),
            })?,
        };
        self.raw_after(stream, n)?;
        Ok(n)
    }

    /// Intentionally lost diagnostics still consume a durable capture number.
    pub fn discard_diagnostic(
        &mut self,
        stream: &str,
        event: &SpoolEvent,
    ) -> Result<(), SpoolError> {
        if !is_diagnostic(&event.kind) {
            return Err(io_error(&self.root)(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "authoritative output cannot be discarded",
            )));
        }
        self.reserve_capture(stream, event)?;
        Ok(())
    }

    /// Puts authoritative events durably at the end of their stream. Only waiting
    /// diagnostics use the lossy byte allowance; their discarded count is returned.
    pub fn append(&mut self, stream: &str, event: &SpoolEvent) -> Result<Vec<Relief>, SpoolError> {
        self.stream_mut(stream).finish_pending_promotion()?;
        let captured;
        let event = if event.kind == "raw.frame" {
            captured = self.captured(stream, event)?;
            &captured
        } else {
            event
        };
        if !is_diagnostic(&event.kind) {
            self.stream_mut(stream).main.append_event(event)?;
            return Ok(Vec::new());
        }
        let size = (RECORD_HEAD_BYTES
            + EVENT_FIXED_BYTES
            + event.kind.len()
            + event.body.to_string().len()) as u64;
        if size > u64::from(u32::MAX) {
            return Err(SpoolError::TooLarge {
                bytes: size,
                max: u64::from(u32::MAX),
            });
        }
        let mut relief = Vec::new();
        if self.diagnostic_bytes().saturating_add(size) > self.options.max_bytes {
            relief = self.make_room(size)?;
        }
        if self.diagnostic_bytes().saturating_add(size) > self.options.max_bytes {
            relief.push(Relief::Diagnostics { events: 1 });
            return Ok(relief);
        }
        self.stream_mut(stream).diagnostics.append_event(event)?;
        Ok(relief)
    }

    /// Waiting diagnostics have a separate budget; native history is never scanned or shed for it.
    fn make_room(&mut self, needed: u64) -> Result<Vec<Relief>, SpoolError> {
        if self.diagnostic_bytes().saturating_add(needed) <= self.options.max_bytes {
            return Ok(Vec::new());
        }
        let mut dropped = 0;
        for log in self.streams.values_mut() {
            if log.main.dir.join(PROMOTION_FILE).exists() {
                continue;
            }
            dropped += log.diagnostics.entries.len();
            log.diagnostics.discard_all()?;
        }
        Ok(if dropped == 0 {
            Vec::new()
        } else {
            vec![Relief::Diagnostics { events: dropped }]
        })
    }

    fn diagnostic_bytes(&self) -> u64 {
        self.streams
            .values()
            .map(|log| log.diagnostics.bytes())
            .sum()
    }

    /// Rewrites a stream's main log into fresh segments, leaving out the
    /// events marked, and swaps them in. Numbered events keep their numbers unless the server
    /// already accepted all of them and the remaining, unnumbered tail resumes above its cursor.
    ///
    /// The new directory is built beside the old one and marked whole; the
    /// old one is moved aside, the new one moved in, the old one deleted.
    /// [`Spool::open`] finishes or undoes the swap if it was cut off, so that
    /// at every moment exactly one complete copy can be found.
    fn rewrite_without(
        &mut self,
        stream: &str,
        leave_out: &[bool],
        resume_after: Option<Seq>,
    ) -> Result<(), SpoolError> {
        let root = self.root.clone();
        let segment_bytes = self.options.segment_bytes;
        let Some(log) = self.streams.get_mut(stream) else {
            return Ok(());
        };
        log.main.flush()?;
        let kept: Vec<SpoolEvent> = log
            .main
            .entries
            .iter()
            .zip(leave_out)
            .filter(|(_, leave)| !**leave)
            .map(|(entry, _)| log.main.read_entry(entry))
            .collect::<Result<_, _>>()?;
        let target = log.main.dir.clone();
        let name = dir_name(stream);
        let staging = root.join(format!("{name}{REWRITING_SUFFIX}"));
        let replaced = root.join(format!("{name}{REPLACED_SUFFIX}"));
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_dir_all(&replaced);
        let mut fresh = Log::new(staging.clone(), segment_bytes);
        fresh.base = match resume_after {
            Some(through) => through.checked_add(1).ok_or_else(|| SpoolError::Io {
                path: target.display().to_string(),
                source: std::io::Error::other("the delivery sequence allocator is exhausted"),
            })?,
            None => log.main.base,
        };
        fresh.highest = fresh.base - 1;
        let sent_through = resume_after.unwrap_or(log.sent_through);
        for event in &kept {
            fresh.append_event(event)?;
        }
        // A mark of how far numbering had got goes with the rewrite.
        let mark = frame_record(&sent_payload(sent_through));
        fresh.append_record(&mark, fresh.next_seq())?;
        fresh.flush()?;
        fresh.sync()?;
        let whole = staging.join(WHOLE_MARK);
        File::create(&whole)
            .and_then(|file| file.sync_all())
            .map_err(io_error(&whole))?;
        if let Ok(directory) = File::open(&staging) {
            directory.sync_all().map_err(io_error(&staging))?;
        }
        if let Ok(directory) = File::open(&root) {
            directory.sync_all().map_err(io_error(&root))?;
        }

        log.main.writer = None;
        if target.exists() {
            std::fs::rename(&target, &replaced).map_err(io_error(&target))?;
            if let Ok(directory) = File::open(&root) {
                directory.sync_all().map_err(io_error(&root))?;
            }
        }
        std::fs::rename(&staging, &target).map_err(io_error(&target))?;
        if let Ok(directory) = File::open(&root) {
            directory.sync_all().map_err(io_error(&root))?;
        }
        let _ = std::fs::remove_dir_all(&replaced);
        let _ = std::fs::remove_file(target.join(WHOLE_MARK));
        if let Ok(directory) = File::open(&root) {
            directory.sync_all().map_err(io_error(&root))?;
        }

        // The new segments, now in place, are the log; new events go to the
        // end of the last.
        let mut segments = BTreeMap::new();
        for (id, segment) in std::mem::take(&mut fresh.segments) {
            segments.insert(
                id,
                Segment {
                    path: target.join(segment_name(id)),
                    bytes: segment.bytes,
                    damaged: segment.damaged,
                    loss_from: segment.loss_from,
                },
            );
        }
        log.main.segments = segments;
        log.main.entries = fresh.entries;
        log.main.base = fresh.base;
        log.main.highest = fresh.highest;
        log.main.acknowledged = resume_after.unwrap_or(log.main.acknowledged);
        log.sent_through = sent_through;
        log.main.dirty = false;
        log.main.revision = new_id();
        log.main.rollback = None;
        if let Some((&id, segment)) = log.main.segments.iter().next_back() {
            let file = OpenOptions::new()
                .append(true)
                .open(&segment.path)
                .map_err(io_error(&segment.path))?;
            log.main.writer = Some((id, BufWriter::new(file)));
        }
        Ok(())
    }

    /// Resumes an unnumbered tail above a server cursor ahead of the remembered delivery counter.
    /// Already accepted entries are discarded; capture numbers and times remain unchanged.
    pub fn resume_after(&mut self, stream: &str, through: Seq) -> Result<(), SpoolError> {
        if through <= self.sent_through(stream) {
            return Ok(());
        }
        let log = self.stream_mut(stream);
        let accepted: Vec<bool> = log
            .main
            .entries
            .iter()
            .map(|entry| entry.seq <= log.sent_through)
            .collect();
        self.rewrite_without(stream, &accepted, Some(through))
    }

    /// Takes up to `count` of the events that wait, numbers them in order
    /// and records that numbering has got that far, so that the numbers
    /// survive a stop. They are returned to be sent.
    pub fn number(&mut self, stream: &str, count: usize) -> Result<Vec<Numbered>, SpoolError> {
        let Some(log) = self.streams.get_mut(stream) else {
            return Ok(Vec::new());
        };
        log.finish_pending_promotion()?;
        let count = count.min(log.unsent());
        if count == 0 {
            return Ok(Vec::new());
        }
        log.main.flush()?;
        let first_place = log
            .main
            .entries
            .iter()
            .take_while(|entry| entry.seq <= log.sent_through)
            .count();
        let expected = log.sent_through.saturating_add(1);
        if log
            .main
            .entries
            .get(first_place)
            .is_some_and(|entry| entry.seq != expected)
        {
            return Err(SpoolError::NotHeld {
                from: expected,
                oldest: log.main.entries[first_place].seq,
            });
        }
        let mut numbered = Vec::with_capacity(count);
        for place in first_place..first_place + count {
            let entry = log.main.entries[place].clone();
            if entry.seq != expected.saturating_add(numbered.len() as Seq) {
                break;
            }
            numbered.push(Numbered {
                seq: entry.seq,
                event: log.main.read_entry(&entry)?,
            });
        }
        let Some(through) = numbered.last().map(|event| event.seq) else {
            return Ok(numbered);
        };
        let mark = frame_record(&sent_payload(through));
        let next_seq = log.main.next_seq();
        log.main.append_record(&mark, next_seq)?;
        log.sent_through = through;
        Ok(numbered)
    }

    /// Reads events that were numbered, from `from` on, at most `max`.
    pub fn read(
        &mut self,
        stream: &str,
        from: Seq,
        max: usize,
    ) -> Result<Vec<Numbered>, SpoolError> {
        let Some(log) = self.streams.get_mut(stream) else {
            return Ok(Vec::new());
        };
        log.main.flush()?;
        if from < log.main.base {
            return Err(SpoolError::NotHeld {
                from,
                oldest: log.main.base,
            });
        }
        let mut events = Vec::new();
        for entry in log
            .main
            .entries
            .iter()
            .filter(|entry| entry.seq >= from && entry.seq <= log.sent_through)
            .take(max)
        {
            let expected = from.saturating_add(events.len() as Seq);
            if entry.seq != expected {
                if events.is_empty() {
                    return Err(SpoolError::NotHeld {
                        from,
                        oldest: entry.seq,
                    });
                }
                break;
            }
            events.push(Numbered {
                seq: entry.seq,
                event: log.main.read_entry(entry)?,
            });
        }
        if max > 0 && events.is_empty() && from <= log.sent_through {
            return Err(SpoolError::NotHeld {
                from,
                oldest: log.main.next_seq(),
            });
        }
        Ok(events)
    }

    /// The sizes in bytes of the events from `from` on that were numbered, as
    /// they are written in a frame, up to `max` of them. The sizes of the
    /// records stand in for them; the caller adds the framing around each.
    #[must_use]
    pub fn sizes(&self, stream: &str, from: Seq, max: usize) -> Vec<usize> {
        let Some(log) = self.streams.get(stream) else {
            return Vec::new();
        };
        if from < log.main.base {
            return Vec::new();
        }
        log.main
            .entries
            .iter()
            .filter(|entry| entry.seq >= from && entry.seq <= log.sent_through)
            .take(max)
            .map(|entry| entry.record_bytes as usize)
            .collect()
    }

    /// Lets go of everything through `through`: it was acknowledged.
    pub fn acknowledge(&mut self, stream: &str, through: Seq) -> Result<(), SpoolError> {
        let Some(log) = self.streams.get_mut(stream) else {
            return Ok(());
        };
        let through = through.min(log.sent_through);
        log.main.acknowledged = log.main.acknowledged.max(through);
        let count = log
            .main
            .entries
            .iter()
            .take_while(|entry| entry.seq <= through)
            .count();
        if count > 0 {
            let mut payload = sent_payload(through);
            payload[0] = RECORD_ACKNOWLEDGED;
            let record = frame_record(&payload);
            let next = log.main.next_seq();
            if let Err(error) = log.main.append_record(&record, next) {
                // The server already owns these facts durably. Reclaim eligible old files
                // even when space for the local ACK mark was exhausted.
                log.main.drop_front(count)?;
                return Err(error);
            }
            // ACKs use the same batch sync as event numbering, and shutdown forces that batch.
            // Forcing every ACK separately would stall ordinary event delivery behind disk I/O.
            log.main.flush()?;
            log.main.drop_front(count)?;
            log.sent_through = log.sent_through.max(log.main.base - 1);
        }
        log.main.drop_front(0)?;
        Ok(())
    }

    /// Takes up to `max` diagnostics of a stream out of their queue and
    /// puts them at the end of the stream, where they are numbered like the
    /// rest. Returns how many.
    pub fn promote_diagnostics(&mut self, stream: &str, max: usize) -> Result<usize, SpoolError> {
        let Some(log) = self.streams.get_mut(stream) else {
            return Ok(0);
        };
        let count = max.min(log.diagnostics.entries.len());
        if count == 0 {
            return Ok(0);
        }
        log.promote(count)
    }

    /// Expires only waiting diagnostics, preserving all unacknowledged protocol events.
    pub fn expire(&mut self, now: Time) -> Result<Vec<Relief>, SpoolError> {
        let horizon = now.unix_millis().saturating_sub(
            i64::try_from(self.options.max_age.as_secs())
                .unwrap_or(i64::MAX / SECOND_MILLIS)
                .saturating_mul(SECOND_MILLIS),
        );
        let mut dropped = 0;
        for log in self.streams.values_mut() {
            if log.main.dir.join(PROMOTION_FILE).exists() {
                continue;
            }
            let old = log
                .diagnostics
                .entries
                .iter()
                .take_while(|entry| entry.at_millis < horizon)
                .count();
            if old > 0 {
                log.diagnostics.consume_diagnostics(old)?;
                dropped += old;
            }
        }
        Ok(if dropped == 0 {
            Vec::new()
        } else {
            vec![Relief::Diagnostics { events: dropped }]
        })
    }

    /// Writes what is buffered out and returns a handle on each file that has
    /// something to be forced to disk, which the caller syncs where blocking
    /// is fine.
    pub fn flush_for_sync(&mut self) -> Result<SyncBatch, SpoolError> {
        if self.raw_dirty {
            self.persist_raw_cursors()?;
            self.raw_dirty = false;
        }
        let mut handles = Vec::new();
        let mut receipts = Vec::new();
        for (stream, log) in &mut self.streams {
            for (diagnostic, log) in [(false, &mut log.main), (true, &mut log.diagnostics)] {
                if let Some(file) = log.sync_handle()? {
                    handles.push(file);
                    receipts.push((stream.clone(), diagnostic, log.revision));
                }
            }
        }
        Ok(SyncBatch {
            files: handles,
            receipts,
        })
    }

    /// Acknowledges only unchanged revisions after every file in the batch was forced.
    pub fn complete_sync(&mut self, batch: &SyncBatch) {
        for (stream, diagnostic, revision) in &batch.receipts {
            if let Some(log) = self.streams.get_mut(stream) {
                let log = if *diagnostic {
                    &mut log.diagnostics
                } else {
                    &mut log.main
                };
                if log.revision == *revision {
                    log.dirty = false;
                }
            }
        }
    }

    /// Writes what is buffered out and forces it to disk, here and now.
    pub fn sync(&mut self) -> Result<(), SpoolError> {
        let batch = self.flush_for_sync()?;
        batch.force().map_err(io_error(&self.root))?;
        self.complete_sync(&batch);
        Ok(())
    }

    /// Whether anything has been appended since the last sync.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.raw_dirty
            || self
                .streams
                .values()
                .any(|log| log.main.dirty || log.diagnostics.dirty)
    }
}

fn sent_payload(through: Seq) -> Vec<u8> {
    let mut payload = Vec::with_capacity(9);
    payload.push(RECORD_SENT);
    payload.extend_from_slice(&through.to_le_bytes());
    payload
}

#[cfg(all(test, target_os = "linux"))]
mod io_failures;

#[cfg(test)]
mod raw_restarts {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_transfer_interrupted_before_during_or_after_copying_keeps_each_diagnostic_once_in_order() {
        // Simulate the durable states at which power can be lost: no copy, one copy, the whole
        // copy, and the queue already consumed but the transfer record's removal not durable yet.
        for stage in 0..=3 {
            let directory = tempfile::tempdir().unwrap();
            let options = SpoolOptions {
                max_bytes: 64 * 1024 * 1024,
                max_age: Duration::from_secs(7 * 86_400),
                segment_bytes: 512,
            };
            let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
            let mut spool = Spool::open(directory.path(), options).unwrap().0;
            spool
                .append(
                    "machine",
                    &SpoolEvent {
                        at,
                        kind: "machine.facts".into(),
                        body: json!({}),
                    },
                )
                .unwrap();
            spool.number("machine", 1).unwrap();
            for ordinal in 1..=3 {
                spool
                    .append(
                        "machine",
                        &SpoolEvent {
                            at,
                            kind: "raw.frame".into(),
                            body: json!({ "ordinal": ordinal }),
                        },
                    )
                    .unwrap();
            }
            spool.sync().unwrap();
            let log = spool.streams.get_mut("machine").unwrap();
            let plan = Promotion {
                main_from: 2,
                diagnostic_from: 1,
                count: 3,
            };
            let path = log.main.dir.join(PROMOTION_FILE);
            let mut file = File::create(&path).unwrap();
            serde_json::to_writer(&mut file, &plan).unwrap();
            file.sync_all().unwrap();
            let copied = match stage {
                0 => 0,
                1 => 1,
                _ => 3,
            };
            for place in 0..copied {
                let event = log
                    .diagnostics
                    .read_entry(&log.diagnostics.entries[place])
                    .unwrap();
                log.main.append_event(&event).unwrap();
            }
            if let Some(file) = log.main.sync_handle().unwrap() {
                file.sync_all().unwrap();
            }
            if stage == 3 {
                let record = frame_record(&sent_payload(3));
                log.diagnostics.append_record(&record, 4).unwrap();
                if let Some(file) = log.diagnostics.sync_handle().unwrap() {
                    file.sync_all().unwrap();
                }
            }
            drop(spool);
            let (mut spool, recovery) = Spool::open(directory.path(), options).unwrap();
            assert!(recovery.damaged.is_empty(), "stage {stage}");
            assert_eq!(spool.diagnostics_waiting("machine"), 0, "stage {stage}");
            let sent = spool.number("machine", 10).unwrap();
            assert_eq!(
                sent.iter()
                    .map(|event| (event.seq, event.event.body["ordinal"].as_i64().unwrap()))
                    .collect::<Vec<_>>(),
                [(2, 1), (3, 2), (4, 3)],
                "stage {stage}"
            );
        }
    }
}
