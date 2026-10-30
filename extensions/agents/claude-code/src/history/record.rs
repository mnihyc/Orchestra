//! Adoption records reconcile item slots on disk and stream the latest complete turns.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use orch_agent_contract::discovery::{
    RECORD_FORMAT, RECORD_MAX_BYTES, RecordReporter, RecordedEntry, RecordedItem, RecordedWords,
    SessionRecord,
};
use orch_agent_contract::events::{ItemStatus, SessionEvent};
use orch_agent_contract::history::SessionArchive;
use orch_types::Clock;

use super::HistoryError;
use super::reader::{Kind, PAGE_BYTES, Rows, check_cancel};
use super::spool::Map;
use crate::events::MapperSettings;

// Each slot/turn record is three integers; the payload itself is appended to a separate file.
const ROW_BYTES: u64 = 24;

fn read_row(file: &mut File, row: u64) -> io::Result<[u64; 3]> {
    file.seek(SeekFrom::Start(row * ROW_BYTES))?;
    let mut out = [0; 3];
    for number in &mut out {
        let mut bytes = [0; 8];
        file.read_exact(&mut bytes)?;
        *number = u64::from_le_bytes(bytes);
    }
    Ok(out)
}
fn write_row(file: &mut File, row: u64, numbers: [u64; 3]) -> io::Result<()> {
    file.seek(SeekFrom::Start(row * ROW_BYTES))?;
    for number in numbers {
        file.write_all(&number.to_le_bytes())?;
    }
    Ok(())
}

/// A file-backed reconstruction whose memory does not depend on the number of turns or items.
#[derive(Debug)]
pub struct Record {
    /// The model named by the last main-session assistant entry.
    pub model: Option<String>,
    turns: File,
    slots: File,
    values: File,
    pending: Map<u64>,
    turn_count: u64,
    slot_count: u64,
    end: u64,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy)]
struct Selection {
    first: u64,
    end: u64,
    turns: u64,
    left_out: u64,
}

impl Record {
    pub(super) fn build(
        path: &Path,
        settings: Option<MapperSettings>,
        clock: Arc<dyn Clock>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, HistoryError> {
        let mut rows = match settings {
            Some(settings) => Rows::new(path, settings, clock, true, Arc::clone(&cancel))?,
            None => Rows::for_record(path, clock, Arc::clone(&cancel))?,
        };
        if let Some(version) = rows.version()?
            && crate::descriptor::support_of(&version)
                == orch_agent_contract::descriptor::HarnessSupport::Unsupported
        {
            return Err(HistoryError::OtherVersion(version));
        }
        let mut record = Self {
            model: None,
            turns: tempfile::tempfile()?,
            slots: tempfile::tempfile()?,
            values: tempfile::tempfile()?,
            pending: Map::with_cancel(Arc::clone(&cancel))?,
            turn_count: 0,
            slot_count: 0,
            end: 0,
            cancel,
        };
        for row in rows.by_ref() {
            check_cancel(&record.cancel)?;
            let row = row?;
            match row.kind {
                Kind::Words {
                    uuid,
                    text,
                    left_out,
                } => {
                    write_row(
                        &mut record.turns,
                        record.turn_count,
                        [record.slot_count, 0, 0],
                    )?;
                    record.turn_count += 1;
                    record.append(&RecordedEntry::Said(RecordedWords {
                        native_ref: format!("said:{uuid}"),
                        turn: row.turn,
                        text,
                        left_out,
                        at: row.at,
                    }))?;
                }
                Kind::Event(event) => match *event {
                    SessionEvent::ItemStarted(started) => {
                        let native = started.native_ref.clone();
                        let slot = record.append(&RecordedEntry::Item(RecordedItem {
                            native_ref: started.native_ref,
                            turn: row.turn,
                            stream: started.stream,
                            parent_native_ref: started.parent_native_ref,
                            status: ItemStatus::Cancelled,
                            content: started.content,
                            at: row.at,
                        }))?;
                        record.pending.insert(&native, &slot)?;
                    }
                    SessionEvent::ItemCompleted(completed) => {
                        if let Some(slot) = record.pending.remove(&completed.native_ref)? {
                            let RecordedEntry::Item(mut item) = record.entry(slot)? else {
                                return Err(io::Error::other("an item slot is not an item").into());
                            };
                            item.content = serde_json::from_value(serde_json::json!({ "kind":item.content.kind(), "payload":completed.payload })).unwrap_or(item.content);
                            item.status = completed.status;
                            item.at = item.at.or(row.at);
                            record.replace(slot, &RecordedEntry::Item(item))?;
                        }
                    }
                    _ => {}
                },
            }
        }
        record.model = rows.model;
        Ok(record)
    }

    fn append(&mut self, entry: &RecordedEntry) -> Result<u64, HistoryError> {
        let slot = self.slot_count;
        self.slot_count += 1;
        let turn = u64::try_from(entry.turn()).map_err(io::Error::other)?;
        write_row(&mut self.slots, slot, [self.end, 0, turn])?;
        let mut row = read_row(&mut self.turns, turn - 1)?;
        row[1] += 1;
        write_row(&mut self.turns, turn - 1, row)?;
        self.replace(slot, entry)?;
        Ok(slot)
    }

    fn replace(&mut self, slot: u64, entry: &RecordedEntry) -> Result<(), HistoryError> {
        check_cancel(&self.cancel)?;
        let previous = read_row(&mut self.slots, slot)?;
        let offset = self.end;
        self.values.seek(SeekFrom::Start(offset))?;
        {
            let mut writer = BufWriter::with_capacity(PAGE_BYTES, &mut self.values);
            serde_json::to_writer(&mut writer, entry).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        self.end = self.values.stream_position()?;
        let length = self.end - offset;
        write_row(&mut self.slots, slot, [offset, length, previous[2]])?;
        let mut turn = read_row(&mut self.turns, previous[2] - 1)?;
        turn[2] = turn[2] - previous[1] + length;
        write_row(&mut self.turns, previous[2] - 1, turn)?;
        Ok(())
    }

    fn entry(&mut self, slot: u64) -> Result<RecordedEntry, HistoryError> {
        let row = read_row(&mut self.slots, slot)?;
        self.values.seek(SeekFrom::Start(row[0]))?;
        let input = (&mut self.values).take(row[1]);
        serde_json::from_reader(BufReader::with_capacity(PAGE_BYTES, input))
            .map_err(|error| io::Error::other(error).into())
    }

    fn select(&mut self, max_bytes: usize) -> Result<Selection, HistoryError> {
        let limit = u64::try_from(max_bytes).map_err(io::Error::other)?;
        let mut bytes = 0;
        let mut turns = 0;
        let mut first = self.slot_count;
        for ordinal in (0..self.turn_count).rev() {
            check_cancel(&self.cancel)?;
            let row = read_row(&mut self.turns, ordinal)?;
            if row[2] > limit - bytes {
                break;
            }
            bytes += row[2];
            turns += 1;
            first = row[0];
        }
        Ok(Selection {
            first,
            end: self.slot_count,
            turns,
            left_out: self.turn_count - turns,
        })
    }

    /// Streams the bounded whole-turn suffix, retaining the original ordinals and report semantics.
    pub fn finish(
        &mut self,
        _session_id: &str,
        write: bool,
        into: &Path,
    ) -> Result<SessionRecord, HistoryError> {
        let selection = self.select(RECORD_MAX_BYTES)?;
        let mut report = RecordReporter::default();
        let mut output = if write && selection.first < selection.end {
            std::fs::create_dir_all(into)?;
            Some(BufWriter::with_capacity(
                PAGE_BYTES,
                tempfile::Builder::new()
                    .prefix("record-")
                    .suffix(".jsonl")
                    .tempfile_in(into)?,
            ))
        } else {
            None
        };
        let mut size = 0_u64;
        for slot in selection.first..selection.end {
            check_cancel(&self.cancel)?;
            let entry = self.entry(slot)?;
            report.push(&entry);
            if let Some(writer) = &mut output {
                serde_json::to_writer(&mut *writer, &entry).map_err(io::Error::other)?;
                writer.write_all(b"\n")?;
            }
            size += read_row(&mut self.slots, slot)?[1];
        }
        let file = if let Some(mut writer) = output {
            writer.flush()?;
            let temporary = writer.into_inner().map_err(|error| error.into_error())?;
            let (_file, path) = temporary.keep().map_err(std::io::Error::other)?;
            Some(SessionArchive {
                format: RECORD_FORMAT.to_owned(),
                path: path.to_string_lossy().into_owned(),
                size: i64::try_from(size).map_err(io::Error::other)?,
            })
        } else {
            None
        };
        Ok(SessionRecord {
            model: self.model.clone(),
            effort: None,
            turns: i64::try_from(selection.turns).map_err(io::Error::other)?,
            entries: i64::try_from(selection.end - selection.first).map_err(io::Error::other)?,
            file,
            report: report.finish(i64::try_from(selection.left_out).map_err(io::Error::other)?),
        })
    }

    #[cfg(test)]
    pub(super) fn selected_turns(
        &mut self,
        max_bytes: usize,
    ) -> Result<Vec<Vec<RecordedEntry>>, HistoryError> {
        let selection = self.select(max_bytes)?;
        let mut turns: Vec<Vec<RecordedEntry>> = Vec::new();
        let mut previous = None;
        for slot in selection.first..selection.end {
            let entry = self.entry(slot)?;
            if previous != Some(entry.turn()) {
                previous = Some(entry.turn());
                turns.push(Vec::new());
            }
            if let Some(turn) = turns.last_mut() {
                turn.push(entry);
            }
        }
        Ok(turns)
    }
}
