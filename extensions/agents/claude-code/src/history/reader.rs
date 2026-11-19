//! Captured transcripts yield one historical mapping batch at a time.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use orch_agent_contract::events::SessionEvent;
use orch_types::{Clock, Json, Time};
use serde_json::json;

use super::{HistoryError, entry_time, not_words};
use crate::discovery::prompt_text;
use crate::events::{HistoryMapper, MapperSettings};
use crate::stream::Frame;

/// Filesystem copies and JSON source reads use one page, independent of transcript length.
pub(super) const PAGE_BYTES: usize = 64 * 1024;

pub(super) fn check_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "transcript reading was cancelled",
        ))
    } else {
        Ok(())
    }
}

pub(super) struct CancelOnDrop(pub(super) Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

struct CancellableRead<'a, R> {
    input: R,
    cancel: &'a AtomicBool,
    line_bytes: usize,
}
impl<R: Read> Read for CancellableRead<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        check_cancel(self.cancel)?;
        if self.line_bytes > crate::config::FRAME_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a transcript entry exceeds the native frame limit",
            ));
        }
        let count = out.len().min(PAGE_BYTES);
        let read = self.input.read(&mut out[..count])?;
        for part in out[..read].split_inclusive(|byte| *byte == b'\n') {
            self.line_bytes = self.line_bytes.saturating_add(part.len());
            if self.line_bytes > crate::config::FRAME_MAX_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a transcript entry exceeds the native frame limit",
                ));
            }
            if part.last() == Some(&b'\n') {
                self.line_bytes = 0;
            }
        }
        Ok(read)
    }
}

fn capture(path: &Path, cancel: &AtomicBool, record: bool) -> Result<File, HistoryError> {
    let mut source = File::open(path)?;
    let before = source.metadata()?;
    if record {
        // Refuse an unsupported source before reserving disk for its potentially huge snapshot.
        let version = super::version_in_reader(CancellableRead {
            input: &mut source,
            cancel,
            line_bytes: 0,
        });
        check_cancel(cancel)?;
        if let Some(version) = version
            && crate::descriptor::support_of(&version)
                == orch_agent_contract::descriptor::HarnessSupport::Unsupported
        {
            return Err(HistoryError::OtherVersion(version));
        }
        source.seek(SeekFrom::Start(0))?;
    }
    let mut snapshot = tempfile::tempfile()?;
    let mut left = before.len();
    let mut page = [0; PAGE_BYTES];
    let mut line_bytes = 0_usize;
    while left > 0 {
        check_cancel(cancel)?;
        let wanted = usize::try_from(left.min(PAGE_BYTES as u64)).map_err(io::Error::other)?;
        let read = source.read(&mut page[..wanted])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the transcript changed while it was captured",
            )
            .into());
        }
        for part in page[..read].split_inclusive(|byte| *byte == b'\n') {
            line_bytes = line_bytes.saturating_add(part.len());
            if line_bytes > crate::config::FRAME_MAX_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a transcript entry exceeds the native frame limit",
                )
                .into());
            }
            if part.last() == Some(&b'\n') {
                line_bytes = 0;
            }
        }
        snapshot.write_all(&page[..read])?;
        left -= read as u64;
    }
    let after = source.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(io::Error::other("the transcript changed while it was captured").into());
    }
    snapshot.seek(SeekFrom::Start(0))?;
    Ok(snapshot)
}

pub(super) enum Kind {
    Words {
        uuid: String,
        text: String,
        left_out: i64,
    },
    Event(Box<SessionEvent>),
}

pub(super) struct Row {
    pub(super) turn: i64,
    pub(super) at: Option<Time>,
    pub(super) kind: Kind,
}

/// A private captured source, one decoded frame and one bounded mapping batch.
pub(super) struct Rows {
    source: BufReader<File>,
    mapper: HistoryMapper,
    pending: VecDeque<Row>,
    deferred: Option<Json>,
    cancel: Arc<AtomicBool>,
    turn: i64,
    open: bool,
    interrupted: bool,
    last_stop: Option<String>,
    last_at: Option<Time>,
    closing: Option<u64>,
    close_at: Option<Time>,
    close_last: bool,
    eof: bool,
    failed: bool,
    pub(super) model: Option<String>,
}

impl Rows {
    pub(super) fn new(
        path: &Path,
        settings: MapperSettings,
        clock: Arc<dyn Clock>,
        close_last: bool,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, HistoryError> {
        let source = capture(path, &cancel, close_last)?;
        Self::from_source(source, settings, clock, close_last, cancel)
    }

    pub(super) fn for_record(
        path: &Path,
        clock: Arc<dyn Clock>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, HistoryError> {
        let mut source = capture(path, &cancel, true)?;
        let directory = super::directory_in(CancellableRead {
            input: &mut source,
            cancel: &cancel,
            line_bytes: 0,
        });
        check_cancel(&cancel)?;
        source.seek(SeekFrom::Start(0))?;
        Self::from_source(
            source,
            MapperSettings {
                directory: directory.clone(),
                cwd: directory,
                raw_events: false,
            },
            clock,
            true,
            cancel,
        )
    }

    fn from_source(
        source: File,
        settings: MapperSettings,
        clock: Arc<dyn Clock>,
        close_last: bool,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, HistoryError> {
        Ok(Self {
            source: BufReader::with_capacity(PAGE_BYTES, source),
            mapper: HistoryMapper::new(settings, clock, Arc::clone(&cancel))?,
            pending: VecDeque::new(),
            deferred: None,
            cancel,
            turn: 0,
            open: false,
            interrupted: false,
            last_stop: None,
            last_at: None,
            closing: None,
            close_at: None,
            close_last,
            eof: false,
            failed: false,
            model: None,
        })
    }

    pub(super) fn version(&mut self) -> io::Result<Option<String>> {
        self.source.seek(SeekFrom::Start(0))?;
        let version = super::version_in_reader(CancellableRead {
            input: &mut self.source,
            cancel: &self.cancel,
            line_bytes: 0,
        });
        check_cancel(&self.cancel)?;
        self.source.seek(SeekFrom::Start(0))?;
        Ok(version)
    }

    fn entry(&mut self) -> Result<Option<Json>, HistoryError> {
        loop {
            let mut line = Vec::new();
            loop {
                check_cancel(&self.cancel)?;
                let page = self.source.fill_buf()?;
                if page.is_empty() {
                    break;
                }
                let end = page.iter().position(|byte| *byte == b'\n');
                let count = end.map_or(page.len(), |end| end + 1);
                if line.len().saturating_add(count) > crate::config::FRAME_MAX_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a transcript entry exceeds the native frame limit",
                    )
                    .into());
                }
                line.extend_from_slice(&page[..count]);
                self.source.consume(count);
                if end.is_some() {
                    break;
                }
            }
            if line.is_empty() {
                return Ok(None);
            }
            if let Ok(entry) = serde_json::from_slice(&line) {
                return Ok(Some(entry));
            }
        }
    }

    fn events(&mut self, events: Vec<SessionEvent>, at: Option<Time>) {
        for event in events {
            if matches!(
                event,
                SessionEvent::ItemStarted(_)
                    | SessionEvent::ItemUpdated(_)
                    | SessionEvent::ItemCompleted(_)
                    | SessionEvent::TurnCompleted(_)
            ) {
                self.pending.push_back(Row {
                    turn: self.turn,
                    at,
                    kind: Kind::Event(Box::new(event)),
                });
            }
        }
    }

    fn advance(&mut self) -> Result<Option<Row>, HistoryError> {
        loop {
            check_cancel(&self.cancel)?;
            if let Some(row) = self.pending.pop_front() {
                return Ok(Some(row));
            }
            if let Some(mut cursor) = self.closing {
                if let Some(events) = self.mapper.close_step(self.interrupted, &mut cursor)? {
                    self.closing = Some(cursor);
                    self.events(events, self.close_at);
                    continue;
                }
                let events = self.mapper.end_turn(self.interrupted);
                self.events(events, self.close_at);
                self.closing = None;
                self.open = false;
                continue;
            }
            let entry = match self.deferred.take() {
                Some(entry) => Some(entry),
                None if self.eof => None,
                None => self.entry()?,
            };
            let Some(entry) = entry else {
                self.eof = true;
                if self.open
                    && (self.close_last
                        || self.interrupted
                        || matches!(
                            self.last_stop.as_deref(),
                            Some("end_turn" | "stop_sequence")
                        ))
                {
                    self.closing = Some(0);
                    self.close_at = self.last_at;
                    continue;
                }
                return Ok(None);
            };
            if entry
                .get("isSidechain")
                .and_then(Json::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let at = entry_time(&entry).or(self.last_at);
            if entry.get("type").and_then(Json::as_str) == Some("user")
                && prompt_text(&entry).is_some()
                && self.open
            {
                self.deferred = Some(entry);
                self.closing = Some(0);
                self.close_at = at;
                continue;
            }
            self.last_at = at;
            match entry.get("type").and_then(Json::as_str) {
                Some("user") => {
                    if let Some(text) = prompt_text(&entry) {
                        let uuid = entry
                            .get("uuid")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        self.turn = self.turn.saturating_add(1);
                        self.mapper
                            .begin_turn(uuid.parse().unwrap_or_else(|_| orch_types::new_id()))?;
                        self.open = true;
                        self.interrupted = false;
                        self.last_stop = None;
                        self.pending.push_back(Row {
                            turn: self.turn,
                            at,
                            kind: Kind::Words {
                                uuid,
                                text,
                                left_out: not_words(&entry),
                            },
                        });
                    } else if self.open {
                        self.interrupted |= entry
                            .pointer("/message/content/0/text")
                            .and_then(Json::as_str)
                            .is_some_and(|text| text.starts_with("[Request interrupted"));
                        let events = self.mapper.on_frame(&Frame::from_value(json!({ "type":"user", "message":entry.get("message"), "tool_use_result":entry.get("toolUseResult"), "parent_tool_use_id":null })))?;
                        self.events(events, at);
                    }
                }
                Some("assistant") => {
                    if let Some(model) = entry
                        .pointer("/message/model")
                        .and_then(Json::as_str)
                        .filter(|name| !name.is_empty() && !name.starts_with('<'))
                    {
                        self.model = Some(model.to_owned());
                    }
                    if self.open {
                        self.last_stop = entry
                            .pointer("/message/stop_reason")
                            .and_then(Json::as_str)
                            .map(str::to_owned)
                            .or(self.last_stop.take());
                        let events = self.mapper.on_frame(&Frame::from_value(json!({ "type":"assistant", "message":entry.get("message"), "parent_tool_use_id":null })))?;
                        self.events(events, at);
                    }
                }
                _ => {}
            }
        }
    }

    #[cfg(test)]
    pub(super) fn retained_rows(&self) -> usize {
        self.pending.len() + self.mapper.retained_rows()
    }
}

impl Iterator for Rows {
    type Item = Result<Row, HistoryError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.advance() {
            Ok(row) => row.map(Ok),
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}
