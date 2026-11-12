//! Bounded exact tool-result lookups in one native thread's append-only history.
use super::{checked, failure, files, io_error, spool};
use crate::protocol::MAX_FRAME_BYTES;
use orch_agent_contract::adapter::AdapterError;
use orch_types::{Hash, Json};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

pub(crate) struct ResultItem {
    pub payload: Json,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Position {
    offset: u64,
    length: u64,
    digest: Hash,
}
impl Position {
    fn new(offset: u64, bytes: &[u8]) -> Result<Self, AdapterError> {
        Ok(Self {
            offset,
            length: u64::try_from(bytes.len()).map_err(|_| failure("history.too_large"))?,
            digest: Hash::of(bytes),
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Located {
    value: Position,
    context: Position,
    payload_digest: Hash,
    conflicting: bool,
}
#[derive(Debug, Clone)]
struct TurnContext {
    turn: String,
    position: Position,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    changed: Option<SystemTime>,
}

/// Positions, not a materialized history, live in the existing private disk map.
#[derive(Debug)]
pub(crate) struct Results {
    home: PathBuf,
    native: String,
    source: Option<PathBuf>,
    stamp: Option<Stamp>,
    data: Option<File>,
    positions: spool::Map<Located>,
    cancel: Arc<AtomicBool>,
    offset: u64,
    context: Option<TurnContext>,
    tail: Option<Position>,
}

struct Line {
    bytes: Option<Vec<u8>>,
    consumed: u64,
    ended: bool,
}
/// Reuse the native frame boundary for one record; the history itself has no size limit.
fn line(reader: &mut BufReader<File>, cancel: &AtomicBool) -> Result<Line, AdapterError> {
    let mut bytes = Some(Vec::new());
    let mut consumed = 0_u64;
    loop {
        spool::check(cancel).map_err(io_error)?;
        let chunk = reader.fill_buf().map_err(io_error)?;
        if chunk.is_empty() {
            return Ok(Line {
                bytes,
                consumed,
                ended: false,
            });
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(chunk.len(), |at| at + 1);
        if let Some(kept) = &mut bytes {
            if kept
                .len()
                .checked_add(take)
                .is_some_and(|length| length <= MAX_FRAME_BYTES)
            {
                kept.extend_from_slice(&chunk[..take]);
            } else {
                bytes = None;
            }
        }
        consumed = consumed
            .checked_add(u64::try_from(take).map_err(|_| failure("history.too_large"))?)
            .ok_or_else(|| failure("history.too_large"))?;
        reader.consume(take);
        if newline.is_some() {
            return Ok(Line {
                bytes,
                consumed,
                ended: true,
            });
        }
    }
}
fn json(bytes: &[u8]) -> Result<Json, AdapterError> {
    serde_json::from_slice(bytes).map_err(|_| failure("history.invalid_result"))
}
fn payload_digest(value: &Json) -> Result<Hash, AdapterError> {
    Ok(Hash::of(
        &serde_json::to_vec(value).map_err(|_| failure("history.invalid_result"))?,
    ))
}
fn key(turn: &str, call: &str) -> Result<String, AdapterError> {
    serde_json::to_string(&(turn, call)).map_err(|_| failure("history.invalid_result"))
}

impl Results {
    pub(crate) fn with_cancel(
        home: PathBuf,
        native: String,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, AdapterError> {
        Ok(Self {
            home,
            native,
            source: None,
            stamp: None,
            data: None,
            positions: spool::Map::with_cancel(Arc::clone(&cancel)).map_err(io_error)?,
            cancel,
            offset: 0,
            context: None,
            tail: None,
        })
    }

    fn actual_path(&self, wanted: &Path) -> Result<PathBuf, AdapterError> {
        let candidates = files(&self.home, wanted, &self.native)?;
        let first = checked(&self.home, wanted)?;
        let compressed = if wanted.extension().is_some_and(|part| part == "zst") {
            wanted.with_extension("")
        } else {
            PathBuf::from(format!("{}.zst", wanted.display()))
        };
        for path in std::iter::once(first)
            .chain(std::iter::once(compressed))
            .chain(candidates)
        {
            let path = checked(&self.home, &path)?;
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() => return Ok(path),
                Ok(_) => return Err(failure("history.identity_changed")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        Err(failure("history.path_unknown"))
    }

    /// Refresh one exact native path, scanning only appended records in the ordinary active file.
    pub(crate) fn refresh(&mut self, wanted: &Path) -> Result<(), AdapterError> {
        spool::check(&self.cancel).map_err(io_error)?;
        let path = self.actual_path(wanted)?;
        let source = File::open(&path).map_err(io_error)?;
        let metadata = source.metadata().map_err(io_error)?;
        let stamp = Stamp {
            bytes: metadata.len(),
            changed: metadata.modified().ok(),
        };
        let compressed = path.extension().is_some_and(|part| part == "zst");
        let reset = self.source.as_ref() != Some(&path)
            || self.data.is_none()
            || (!compressed && stamp.bytes < self.offset)
            || (compressed && self.stamp.as_ref() != Some(&stamp));
        if reset {
            self.positions = spool::Map::with_cancel(Arc::clone(&self.cancel)).map_err(io_error)?;
            self.offset = 0;
            self.context = None;
            self.tail = None;
        }
        if compressed {
            if reset {
                let mut decoded =
                    BufReader::new(zstd::stream::read::Decoder::new(source).map_err(io_error)?);
                let mut cache = tempfile::tempfile().map_err(io_error)?;
                loop {
                    spool::check(&self.cancel).map_err(io_error)?;
                    let bytes = decoded.fill_buf().map_err(io_error)?;
                    if bytes.is_empty() {
                        break;
                    }
                    cache.write_all(bytes).map_err(io_error)?;
                    let used = bytes.len();
                    decoded.consume(used);
                }
                cache.seek(SeekFrom::Start(0)).map_err(io_error)?;
                self.data = Some(cache);
            }
        } else {
            self.data = Some(source);
        }
        self.source = Some(path);
        self.stamp = Some(stamp);
        self.check_header()?;
        if let Some(tail) = self.tail.clone() {
            self.at(&tail)?;
        }
        self.scan()
    }

    fn reader(&self, offset: u64) -> Result<BufReader<File>, AdapterError> {
        let mut file = self
            .data
            .as_ref()
            .ok_or_else(|| failure("history.path_unknown"))?
            .try_clone()
            .map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        Ok(BufReader::new(file))
    }
    fn check_header(&self) -> Result<(), AdapterError> {
        let first = line(&mut self.reader(0)?, &self.cancel)?;
        let header = json(
            first
                .bytes
                .as_deref()
                .ok_or_else(|| failure("history.too_large"))?,
        )?;
        if header["type"] != "session_meta" || header["payload"]["id"] != self.native {
            return Err(failure("history.identity_changed"));
        }
        Ok(())
    }
    fn at(&self, position: &Position) -> Result<Json, AdapterError> {
        let size = usize::try_from(position.length).map_err(|_| failure("history.too_large"))?;
        if size > MAX_FRAME_BYTES {
            return Err(failure("history.too_large"));
        }
        let mut reader = self.reader(position.offset)?;
        let mut bytes = vec![0; size];
        reader.read_exact(&mut bytes).map_err(io_error)?;
        if Hash::of(&bytes) != position.digest {
            return Err(failure("history.identity_changed"));
        }
        json(&bytes)
    }
    fn scan(&mut self) -> Result<(), AdapterError> {
        let mut reader = self.reader(self.offset)?;
        loop {
            spool::check(&self.cancel).map_err(io_error)?;
            let found = line(&mut reader, &self.cancel)?;
            if found.consumed == 0 {
                break;
            }
            let Some(bytes) = found.bytes else {
                if !found.ended {
                    break;
                }
                self.offset = self
                    .offset
                    .checked_add(found.consumed)
                    .ok_or_else(|| failure("history.too_large"))?;
                self.context = None;
                self.tail = None;
                continue;
            };
            let parsed = match json(&bytes) {
                Ok(parsed) => parsed,
                Err(_) if !found.ended => break,
                Err(_) if bytes.iter().all(u8::is_ascii_whitespace) => {
                    self.offset = self
                        .offset
                        .checked_add(found.consumed)
                        .ok_or_else(|| failure("history.too_large"))?;
                    continue;
                }
                Err(_) => {
                    self.offset = self
                        .offset
                        .checked_add(found.consumed)
                        .ok_or_else(|| failure("history.too_large"))?;
                    self.context = None;
                    self.tail = None;
                    continue;
                }
            };
            let position = Position::new(self.offset, &bytes)?;
            self.offset = self
                .offset
                .checked_add(found.consumed)
                .ok_or_else(|| failure("history.too_large"))?;
            self.tail = Some(position.clone());
            match parsed["type"].as_str() {
                Some("turn_context") => {
                    self.context = parsed["payload"]["turn_id"]
                        .as_str()
                        .filter(|turn| !turn.is_empty())
                        .map(|turn| TurnContext {
                            turn: turn.into(),
                            position,
                        })
                }
                Some("response_item") if parsed["payload"]["type"] == "function_call_output" => {
                    let Some(context) = &self.context else {
                        continue;
                    };
                    let Some(call) = parsed["payload"]["call_id"]
                        .as_str()
                        .filter(|call| !call.is_empty())
                    else {
                        continue;
                    };
                    let key = key(&context.turn, call)?;
                    let digest = payload_digest(&parsed["payload"])?;
                    if let Some(mut previous) = self.positions.get(&key).map_err(io_error)? {
                        if previous.payload_digest != digest {
                            previous.conflicting = true;
                            self.positions.insert(&key, &previous).map_err(io_error)?;
                        }
                    } else {
                        self.positions
                            .insert(
                                &key,
                                &Located {
                                    value: position,
                                    context: context.position.clone(),
                                    payload_digest: digest,
                                    conflicting: false,
                                },
                            )
                            .map_err(io_error)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Returns the original native response item, never another turn's same-looking call id.
    #[cfg(test)]
    pub(crate) fn get(&mut self, turn: &str, call: &str) -> Result<Option<Json>, AdapterError> {
        Ok(self
            .get_with_model(turn, call)?
            .map(|result| result.payload))
    }
    pub(crate) fn get_with_model(
        &mut self,
        turn: &str,
        call: &str,
    ) -> Result<Option<ResultItem>, AdapterError> {
        let Some(record) = self.positions.get(&key(turn, call)?).map_err(io_error)? else {
            return Ok(None);
        };
        if record.conflicting {
            return Err(failure("history.ambiguous_result"));
        }
        self.check_header()?;
        let context = self.at(&record.context)?;
        if context["type"] != "turn_context" || context["payload"]["turn_id"] != turn {
            return Err(failure("history.identity_changed"));
        }
        let result = self.at(&record.value)?;
        let payload = &result["payload"];
        if result["type"] != "response_item"
            || payload["type"] != "function_call_output"
            || payload["call_id"] != call
            || payload_digest(payload)? != record.payload_digest
        {
            return Err(failure("history.identity_changed"));
        }
        let model = context
            .pointer("/payload/model")
            .and_then(Json::as_str)
            .filter(|model| !model.is_empty())
            .map(str::to_owned);
        Ok(Some(ResultItem {
            payload: payload.clone(),
            model,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs::OpenOptions;

    struct Fixture {
        home: tempfile::TempDir,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            let path = home
                .path()
                .join("sessions/2026/11/08/rollout-2026-11-08T00-00-00-native.jsonl");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let fixture = Self { home, path };
            fixture.append(json!({"type":"session_meta","payload":{"id":"native"}}));
            fixture
        }
        fn append(&self, value: Json) {
            let mut file = OpenOptions::new()
                .append(true)
                .create(true)
                .open(&self.path)
                .unwrap();
            serde_json::to_writer(&mut file, &value).unwrap();
            file.write_all(b"\n").unwrap();
        }
        fn turn(&self, turn: &str) {
            self.append(json!({"type":"turn_context","payload":{"turn_id":turn}}));
        }
        fn result(&self, call: &str, value: &str) {
            self.append(json!({"type":"response_item","payload":{"type":"function_call_output","call_id":call,"output":value}}));
        }
        fn index(&self) -> Results {
            Results::with_cancel(
                self.home.path().to_owned(),
                "native".into(),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap()
        }
    }
    #[test]
    fn reused_call_ids_stay_bound_to_the_exact_persisted_turn_across_refresh_and_cold_open() {
        let fixture = Fixture::new();
        fixture.turn("first");
        fixture.result("view", "first image");
        let mut index = fixture.index();
        index.refresh(&fixture.path).unwrap();
        fixture.turn("second");
        fixture.result("view", "second image");
        index.refresh(&fixture.path).unwrap();
        assert_eq!(
            index.get("first", "view").unwrap().unwrap()["output"],
            "first image"
        );
        assert_eq!(
            index.get("second", "view").unwrap().unwrap()["output"],
            "second image"
        );
        assert!(index.get("third", "view").unwrap().is_none());
        let mut cold = fixture.index();
        cold.refresh(&fixture.path).unwrap();
        assert_eq!(
            cold.get("second", "view").unwrap(),
            index.get("second", "view").unwrap()
        );
    }
    #[test]
    fn missing_or_malformed_context_cannot_borrow_the_previous_turn() {
        let fixture = Fixture::new();
        fixture.turn("first");
        fixture.result("known", "kept");
        fixture.append(json!({"type":"turn_context","payload":{}}));
        fixture.result("unknown", "not assigned");
        let mut index = fixture.index();
        index.refresh(&fixture.path).unwrap();
        assert!(index.get("first", "known").unwrap().is_some());
        assert!(index.get("first", "unknown").unwrap().is_none());
    }
    #[test]
    fn an_incomplete_append_is_not_indexed_and_a_later_complete_result_is_read_once_ready() {
        let fixture = Fixture::new();
        fixture.turn("one");
        let mut file = OpenOptions::new().append(true).open(&fixture.path).unwrap();
        file.write_all(br#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"view","output":"#).unwrap();
        let mut index = fixture.index();
        index.refresh(&fixture.path).unwrap();
        assert!(index.get("one", "view").unwrap().is_none());
        file.write_all(b"\"complete\"}}\n").unwrap();
        index.refresh(&fixture.path).unwrap();
        assert_eq!(
            index.get("one", "view").unwrap().unwrap()["output"],
            "complete"
        );
    }
    #[test]
    fn changed_context_or_result_bytes_cannot_be_returned_from_a_saved_position() {
        let fixture = Fixture::new();
        fixture.turn("one");
        fixture.result("view", "original");
        let mut index = fixture.index();
        index.refresh(&fixture.path).unwrap();
        let original = std::fs::read_to_string(&fixture.path).unwrap();
        std::fs::write(&fixture.path, original.replace("original", "tampered")).unwrap();
        assert!(index.get("one", "view").is_err());
    }
    #[test]
    fn conflicting_results_are_not_selected_and_identical_repeats_are_kept() {
        let fixture = Fixture::new();
        fixture.turn("one");
        fixture.result("same", "first");
        fixture.result("same", "first");
        fixture.result("ambiguous", "first");
        fixture.result("ambiguous", "second");
        let mut index = fixture.index();
        index.refresh(&fixture.path).unwrap();
        assert_eq!(
            index.get("one", "same").unwrap().unwrap()["output"],
            "first"
        );
        assert!(index.get("one", "ambiguous").is_err());
    }
    #[test]
    fn a_compressed_native_source_uses_the_same_header_context_and_result_checks() {
        let fixture = Fixture::new();
        fixture.turn("one");
        fixture.result("view", "compressed image");
        let compressed = PathBuf::from(format!("{}.zst", fixture.path.display()));
        let bytes = zstd::stream::encode_all(File::open(&fixture.path).unwrap(), 0).unwrap();
        std::fs::write(&compressed, bytes).unwrap();
        let mut index = fixture.index();
        index.refresh(&compressed).unwrap();
        assert_eq!(
            index.get("one", "view").unwrap().unwrap()["output"],
            "compressed image"
        );
        index.refresh(&compressed).unwrap();
        assert_eq!(
            index.get("one", "view").unwrap().unwrap()["output"],
            "compressed image"
        );
    }
    #[test]
    fn an_outside_path_and_another_sessions_header_are_never_tool_sources() {
        let fixture = Fixture::new();
        fixture.turn("one");
        fixture.result("view", "image");
        let other = tempfile::tempdir().unwrap();
        let path = other.path().join("rollout-native.jsonl");
        std::fs::write(&path, std::fs::read(&fixture.path).unwrap()).unwrap();
        let mut index = fixture.index();
        assert!(index.refresh(&path).is_err());
        let changed = std::fs::read_to_string(&fixture.path)
            .unwrap()
            .replace("\"id\":\"native\"", "\"id\":\"other\"");
        std::fs::write(&fixture.path, changed).unwrap();
        assert!(index.refresh(&fixture.path).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_not_an_image_result_source() {
        let fixture = Fixture::new();
        fixture.turn("one");
        fixture.result("view", "image");
        let link = fixture
            .path
            .parent()
            .unwrap()
            .join("rollout-2026-11-08T01-00-00-native.jsonl");
        std::os::unix::fs::symlink(&fixture.path, &link).unwrap();
        assert!(fixture.index().refresh(&link).is_err());
    }
}
