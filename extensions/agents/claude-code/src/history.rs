//! Transcript export, import and fork between homes; arrival from another harness by summary.
//!
//! The harness keeps a session as a file of JSON lines under its home, `projects/<directory>/<session id>.jsonl`,
//! and the transcripts of the sub-agents that session started in a directory beside it, named for the
//! session. The format of those lines is the harness's own and changes between versions, so this module
//! reads none of it except where it must, and never rewrites it: a session is exported as the files that
//! make it, whole, and imported by putting those files back. A recorded archive version must match the loaded
//! harness and be one this adapter drives or is checking; otherwise the conversation is carried another way.
//!
//! The archive is one file: a first line naming the format, a second line that lists the files (their paths
//! below the project directory, and their sizes), and then the bytes of the files in that order.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use std::sync::Arc;

use orch_agent_contract::discovery::SessionRecord;
use orch_agent_contract::events::SessionEvent;
use orch_agent_contract::history::{Fidelity, HistoryReport, ReportBuilder, SessionArchive};
use orch_types::{Clock, Json};

use crate::events::MapperSettings;

use crate::config::{MANAGED_PROJECT_DIR, SUPPORTED_RANGE};

mod archive;
mod reader;
mod record;
pub(crate) mod spool;

pub use record::Record;

/// The first line of an archive.
const ARCHIVE_MAGIC: &str = "orch-claude-code-archive-v1";

/// What went wrong with a transcript.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// The session has no transcript in this home.
    #[error("the harness has no transcript of session {0}")]
    NoTranscript(String),
    /// A file could not be read or written.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The archive is not one this adapter wrote.
    #[error("the archive is not a Claude Code archive: {0}")]
    NotAnArchive(String),
    /// The archive was made by a harness this adapter does not drive.
    #[error("the transcript was written by Claude Code {0}, outside {SUPPORTED_RANGE}")]
    OtherVersion(String),
    /// The archive belongs to a different build than the loaded harness.
    #[error("the transcript version {archive} does not match the loaded Claude Code {target}")]
    VersionMismatch {
        /// The version that wrote the transcript.
        archive: String,
        /// The version of the loaded harness.
        target: String,
    },
}

/// The directory in which a home keeps transcripts.
#[must_use]
pub fn projects_directory(home: &Path) -> PathBuf {
    home.join("projects")
}

/// The transcript file of a session in a home, wherever its working directory put it.
#[must_use]
pub fn find_transcript(home: &Path, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() || session_id.contains(['/', '\\']) || session_id.contains("..") {
        return None;
    }
    let managed = projects_directory(home)
        .join(MANAGED_PROJECT_DIR)
        .join(format!("{session_id}.jsonl"));
    if managed.is_file() {
        return Some(managed);
    }
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(projects_directory(home))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(&name))
        .find(|candidate| candidate.is_file())
}

fn version_in_reader(input: impl Read) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Version {
        version: Option<String>,
    }
    let mut input = BufReader::with_capacity(64 * 1024, input);
    loop {
        // Unknown fields, including large tool results, are skipped without being materialized.
        let entry = serde_json::Deserializer::from_reader(&mut input)
            .into_iter::<Version>()
            .next();
        match entry {
            Some(Ok(Version {
                version: Some(version),
            })) => return Some(version),
            Some(Ok(_)) => {}
            None => return None,
            Some(Err(_)) => {
                // A malformed line must not hide a later version declaration. Discard it in pages.
                loop {
                    let page = input.fill_buf().ok()?;
                    if page.is_empty() {
                        return None;
                    }
                    let newline = page.iter().position(|byte| *byte == b'\n');
                    let count = newline.map_or(page.len(), |end| end + 1);
                    input.consume(count);
                    if newline.is_some() {
                        break;
                    }
                }
            }
        }
    }
}

/// Writes a session of a home as an archive in `into`, using bounded manifest and copy pages.
pub async fn export_transcript(
    home: &Path,
    session_id: &str,
    into: &Path,
) -> Result<SessionArchive, HistoryError> {
    archive::export(home, session_id, into).await
}

/// Puts the files of an archive into a home and answers the id of the session they make.
pub async fn import_transcript(
    home: &Path,
    managed: bool,
    archive: &SessionArchive,
    target_version: &str,
) -> Result<String, HistoryError> {
    archive::import(home, managed, archive, target_version).await
}

fn uuid_like(text: &str) -> Option<uuid_check::Checked> {
    uuid_check::check(text)
}

/// A minimal check that an id is the shape of a UUID, so that it is safe as a file name.
mod uuid_check {
    pub struct Checked;
    pub fn check(text: &str) -> Option<Checked> {
        let groups: Vec<&str> = text.split('-').collect();
        let sizes = [8, 4, 4, 4, 12];
        (groups.len() == 5
            && groups.iter().zip(sizes).all(|(group, size)| {
                group.len() == size && group.bytes().all(|b| b.is_ascii_hexdigit())
            }))
        .then_some(Checked)
    }
}

/// A captured transcript yielding historical events in order, without retaining previous events.
pub struct Replay {
    rows: reader::Rows,
    /// How many completed items have been yielded so far.
    pub items: u32,
}

impl std::fmt::Debug for Replay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Replay")
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

impl Iterator for Replay {
    type Item = Result<SessionEvent, HistoryError>;
    fn next(&mut self) -> Option<Self::Item> {
        for row in self.rows.by_ref() {
            let row = match row {
                Ok(row) => row,
                Err(error) => return Some(Err(error)),
            };
            if let reader::Kind::Event(event) = row.kind {
                let event = *event;
                if matches!(event, SessionEvent::ItemCompleted(_)) {
                    self.items = self.items.saturating_add(1);
                }
                return Some(Ok(event));
            }
        }
        None
    }
}

/// Reads a captured transcript through the same item mapping, without replaying historical usage.
pub fn replay(
    transcript: &Path,
    settings: MapperSettings,
    clock: Arc<dyn Clock>,
) -> Result<Replay, HistoryError> {
    Ok(Replay {
        rows: reader::Rows::new(transcript, settings, clock, false, Arc::default())?,
        items: 0,
    })
}

/// Delivers historical events with bounded producer backpressure and cancellation between source pages.
pub async fn replay_into(
    transcript: &Path,
    settings: MapperSettings,
    clock: Arc<dyn Clock>,
    sink: orch_agent_contract::adapter::EventSink,
    thread: orch_types::Id,
    segment: orch_types::Id,
) -> Result<u32, HistoryError> {
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _guard = reader::CancelOnDrop(Arc::clone(&cancel));
    let path = transcript.to_path_buf();
    let (sender, mut events) =
        orch_agent_contract::flow::channel(|event: &(SessionEvent, usize)| event.1);
    let runtime = tokio::runtime::Handle::current();
    let work = tokio::task::spawn_blocking(move || {
        let mut replay = Replay {
            rows: reader::Rows::new(&path, settings, clock, false, cancel)?,
            items: 0,
        };
        for event in replay.by_ref() {
            let event = event?;
            let value = serde_json::to_value(&event).map_err(std::io::Error::other)?;
            let weight = orch_agent_contract::flow::json_weight(&value);
            drop(value);
            runtime
                .block_on(sender.send((event, weight)))
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "the host stopped consuming historical events",
                    )
                })?;
        }
        Ok::<_, HistoryError>(replay.items)
    });
    while let Some((event, _)) = events.recv().await {
        sink.session(thread, segment, event).await.map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "the host stopped consuming historical events",
            )
        })?;
    }
    work.await.map_err(std::io::Error::other)?
}

/// The moment a transcript entry states.
fn entry_time(entry: &Json) -> Option<orch_types::Time> {
    entry
        .get("timestamp")
        .and_then(Json::as_str)
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|at| orch_types::Time::new(at.with_timezone(&chrono::Utc)))
}

/// How many of what a person gave with a prompt were not words (pictures, documents), which are not carried.
fn not_words(entry: &Json) -> i64 {
    entry
        .pointer("/message/content")
        .and_then(Json::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| {
                    !matches!(
                        block.get("type").and_then(Json::as_str),
                        Some("text" | "tool_result")
                    )
                })
                .count()
        })
        .map_or(0, |count| i64::try_from(count).unwrap_or(i64::MAX))
}

/// What a transcript holds as a record, for a person who adopts the session: the model it ran with last, and
/// for each turn what the person said and the items the mapping makes of the rest, each where it began in the
/// transcript and under the name a resync gives the same item. A transcript that a version older than the range
/// this adapter drives wrote is refused; a newer one is read as far as its lines are known, as a newer harness
/// is run.
pub fn record(
    transcript: &Path,
    settings: MapperSettings,
    clock: Arc<dyn Clock>,
) -> Result<Record, HistoryError> {
    Record::build(transcript, Some(settings), clock, Arc::default())
}

/// The first working directory, skipping unrelated JSON fields without materializing their values.
fn directory_in(source: impl Read) -> String {
    #[derive(serde::Deserialize)]
    struct Directory {
        cwd: Option<String>,
    }
    let mut input = BufReader::with_capacity(reader::PAGE_BYTES, source);
    loop {
        match serde_json::Deserializer::from_reader(&mut input)
            .into_iter::<Directory>()
            .next()
        {
            Some(Ok(Directory {
                cwd: Some(directory),
            })) => return directory,
            Some(Ok(_)) => {}
            None => return String::new(),
            Some(Err(_)) => loop {
                let Ok(page) = input.fill_buf() else {
                    return String::new();
                };
                if page.is_empty() {
                    return String::new();
                }
                let newline = page.iter().position(|byte| *byte == b'\n');
                let count = newline.map_or(page.len(), |end| end + 1);
                input.consume(count);
                if newline.is_some() {
                    break;
                }
            },
        }
    }
}

fn read_record_with_cancel(
    transcript: &Path,
    session_id: &str,
    write: bool,
    into: &Path,
    clock: Arc<dyn Clock>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Result<SessionRecord, HistoryError> {
    Record::build(transcript, None, clock, cancel)?.finish(session_id, write, into)
}

/// Streams the bounded adoption record, leaving its source transcript unchanged.
pub fn read_record(
    transcript: &Path,
    session_id: &str,
    write: bool,
    into: &Path,
    clock: Arc<dyn Clock>,
) -> Result<SessionRecord, HistoryError> {
    read_record_with_cancel(transcript, session_id, write, into, clock, Arc::default())
}

/// Runs filesystem reconstruction outside the runtime and cancels it when the caller goes away.
pub async fn read_record_async(
    transcript: &Path,
    session_id: &str,
    write: bool,
    into: &Path,
    clock: Arc<dyn Clock>,
) -> Result<SessionRecord, HistoryError> {
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _guard = reader::CancelOnDrop(Arc::clone(&cancel));
    let (transcript, session, into) = (
        transcript.to_path_buf(),
        session_id.to_owned(),
        into.to_path_buf(),
    );
    tokio::task::spawn_blocking(move || {
        read_record_with_cancel(&transcript, &session, write, &into, clock, cancel)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// What an import of a whole transcript reports: it arrived as the harness keeps it.
#[must_use]
pub fn lossless_report(items: i64) -> HistoryReport {
    let mut report = ReportBuilder::new();
    report.keep_many("transcript_entry", items);
    report.finish(Fidelity::Lossless)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PORTABLE_ARCHIVE_CANDIDATE, TRANSCRIPT_FORMAT};
    use orch_agent_contract::discovery::{RECORD_FORMAT, RECORD_MAX_BYTES, RecordedEntry};
    use std::io::{BufWriter, Write};

    #[test]
    fn a_ten_billion_item_native_report_keeps_one_aggregate_instead_of_visiting_items() {
        const ITEMS: i64 = 10_000_000_000;
        let report = lossless_report(ITEMS);
        assert_eq!(report.fidelity, Fidelity::Lossless);
        assert_eq!(report.items_total, ITEMS);
        assert_eq!(report.by_kind.len(), 1);
        assert_eq!(report.by_kind["transcript_entry"].kept, ITEMS);
        assert!(report.dropped.is_empty());
        assert!(report.warnings.is_empty());
        for count in [0, -1] {
            let empty = lossless_report(count);
            assert_eq!(empty.items_total, 0);
            assert!(empty.by_kind.is_empty());
        }
    }

    #[test]
    fn a_million_historical_items_keep_one_mapping_batch_and_a_bounded_whole_turn_record() {
        const TURNS: usize = 1000;
        const ITEMS_PER_TURN: usize = 1000;
        // The release runner places TMPDIR on its storage-backed probe area.
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("million.jsonl");
        let mut output = BufWriter::new(std::fs::File::create(&path).unwrap());
        for turn in 0..TURNS {
            serde_json::to_writer(&mut output, &serde_json::json!({
                "type":"user", "uuid":format!("00000000-0000-4000-8000-{turn:012x}"),
                "version":PORTABLE_ARCHIVE_CANDIDATE, "cwd":"/work", "message":{"content":"continue"}
            })).unwrap();
            output.write_all(b"\n").unwrap();
            for item in 0..ITEMS_PER_TURN {
                serde_json::to_writer(
                    &mut output,
                    &serde_json::json!({
                        "type":"assistant", "message":{"id":format!("message-{turn}-{item}"),
                        "model":"historical-model", "content":[{"type":"text","text":"kept words"}],
                        "stop_reason":"end_turn", "usage":{"input_tokens":10_000_000}}
                    }),
                )
                .unwrap();
                output.write_all(b"\n").unwrap();
            }
        }
        output.flush().unwrap();
        drop(output);
        let original_size = std::fs::metadata(&path).unwrap().len();
        let settings = MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        };
        let mut replayed = replay(&path, settings.clone(), clock()).unwrap();
        let mut completed = 0;
        let mut turns = 0;
        let mut peak = 0;
        while let Some(event) = replayed.next() {
            match event.unwrap() {
                SessionEvent::ItemCompleted(_) => completed += 1,
                SessionEvent::TurnCompleted(_) => turns += 1,
                SessionEvent::UsageUpdated(_) => {
                    panic!("historical use must never be charged again")
                }
                _ => {}
            }
            peak = peak.max(replayed.rows.retained_rows());
        }
        assert_eq!(completed, TURNS * ITEMS_PER_TURN);
        assert_eq!(turns, TURNS);
        assert_eq!(usize::try_from(replayed.items).unwrap(), completed);
        assert!(
            peak <= 4,
            "only one mapping batch and its pending final answer: {peak}"
        );
        let mut recorded = record(&path, settings, clock()).unwrap();
        let taken = recorded
            .finish("million", true, &home.path().join("records"))
            .unwrap();
        assert!(taken.turns > 0 && taken.turns < i64::try_from(TURNS).unwrap());
        assert_eq!(
            taken.entries,
            taken.turns * i64::try_from(ITEMS_PER_TURN + 1).unwrap()
        );
        assert_eq!(
            taken.report.by_kind["agent_message"].kept,
            taken.turns * i64::try_from(ITEMS_PER_TURN).unwrap()
        );
        let archive = taken.file.unwrap();
        assert!(archive.size <= i64::try_from(RECORD_MAX_BYTES).unwrap());
        let mut count = 0;
        let mut first = None;
        let mut last = None;
        for line in BufReader::new(std::fs::File::open(archive.path).unwrap()).lines() {
            let entry: RecordedEntry = serde_json::from_str(&line.unwrap()).unwrap();
            first.get_or_insert(entry.turn());
            last = Some(entry.turn());
            count += 1;
        }
        assert_eq!(first, Some(i64::try_from(TURNS).unwrap() - taken.turns + 1));
        assert_eq!(last, Some(i64::try_from(TURNS).unwrap()));
        assert_eq!(count, taken.entries);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), original_size);
    }

    #[test]
    fn an_oversized_final_turn_is_left_out_whole_without_retaining_its_record() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("large-turn.jsonl");
        let mut output = BufWriter::new(std::fs::File::create(&path).unwrap());
        for (turn, text) in [(1, "earlier".to_owned()), (2, "x".repeat(RECORD_MAX_BYTES))] {
            serde_json::to_writer(&mut output, &serde_json::json!({"type":"user", "uuid":format!("00000000-0000-4000-8000-{turn:012x}"), "message":{"content":"words"}})).unwrap();
            output.write_all(b"\n").unwrap();
            serde_json::to_writer(&mut output, &serde_json::json!({"type":"assistant", "message":{"id":format!("message-{turn}"), "stop_reason":"end_turn", "content":[{"type":"text","text":text}]}})).unwrap();
            output.write_all(b"\n").unwrap();
        }
        output.flush().unwrap();
        drop(output);
        let taken =
            read_record(&path, "large", true, &home.path().join("records"), clock()).unwrap();
        assert_eq!((taken.turns, taken.entries), (0, 0));
        assert!(taken.file.is_none());
        assert!(
            taken
                .report
                .warnings
                .iter()
                .any(|warning| warning.starts_with("2 earlier turns"))
        );
        assert!(!home.path().join("records").exists());
    }

    #[test]
    fn cancelling_transcript_work_stops_before_another_mapping_batch() {
        let home = adopted_home();
        let path = find_transcript(home.path(), SESSION_ID).unwrap();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut rows = reader::Rows::new(
            &path,
            MapperSettings {
                directory: "/work".into(),
                cwd: "/work".into(),
                raw_events: false,
            },
            clock(),
            false,
            Arc::clone(&cancel),
        )
        .unwrap();
        rows.next().unwrap().unwrap();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let error = rows.next().unwrap().err().unwrap();
        assert!(
            matches!(error, HistoryError::Io(error) if error.kind() == std::io::ErrorKind::Interrupted)
        );
        assert!(rows.next().is_none());
    }

    #[test]
    fn historical_plan_changes_keep_one_start_and_the_final_state_without_a_growing_cache() {
        const TASKS: usize = 10_000;
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("plan.jsonl");
        let mut output = BufWriter::new(std::fs::File::create(&path).unwrap());
        writeln!(
            output,
            "{}",
            serde_json::json!({"type":"user", "message":{"content":"plan"}})
        )
        .unwrap();
        for task in 0..TASKS {
            writeln!(output, "{}", serde_json::json!({"type":"assistant", "message":{"id":format!("message-{task}"), "content":[{"type":"tool_use", "id":format!("tool-{task}"), "name":"TaskCreate", "input":{"subject":format!("task {task}")}}]}})).unwrap();
            writeln!(output, "{}", serde_json::json!({"type":"user", "message":{"content":[{"type":"tool_result", "tool_use_id":format!("tool-{task}"), "content":"created"}]}, "toolUseResult":{"task":{"id":format!("{task:08}")}}})).unwrap();
        }
        writeln!(output, "{}", serde_json::json!({"type":"assistant", "message":{"id":"final", "stop_reason":"end_turn", "content":[{"type":"text", "text":"planned"}]}})).unwrap();
        output.flush().unwrap();
        drop(output);
        let mut rows = reader::Rows::new(
            &path,
            MapperSettings {
                directory: "/work".into(),
                cwd: "/work".into(),
                raw_events: false,
            },
            clock(),
            false,
            Arc::default(),
        )
        .unwrap();
        let mut starts = 0;
        let mut updates = 0;
        let mut peak = 0;
        loop {
            let Some(row) = rows.next() else {
                break;
            };
            if let reader::Kind::Event(event) = row.unwrap().kind {
                match *event {
                    SessionEvent::ItemStarted(item)
                        if matches!(
                            item.content,
                            orch_agent_contract::events::ItemContent::Plan(_)
                        ) =>
                    {
                        starts += 1
                    }
                    SessionEvent::ItemUpdated(item) if item.native_ref.starts_with("plan:") => {
                        updates += 1;
                        let steps = item.payload["steps"].as_array().unwrap();
                        assert_eq!(steps.len(), TASKS);
                        assert_eq!(steps.first().unwrap()["text"], "task 0");
                        assert_eq!(steps.last().unwrap()["text"], format!("task {}", TASKS - 1));
                    }
                    _ => {}
                }
            }
            peak = peak.max(rows.retained_rows());
        }
        assert_eq!((starts, updates), (1, 1));
        assert!(peak <= 4);
    }

    const ID: &str = "11111111-1111-4111-8111-111111111111";

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn home_with_session(version: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("projects").join("-workspace");
        write(
            &project.join(format!("{ID}.jsonl")),
            &format!("{{\"type\":\"user\",\"version\":\"{version}\",\"sessionId\":\"{ID}\"}}\n"),
        );
        write(
            &project.join(ID).join("subagents").join("agent-a1.jsonl"),
            "{\"type\":\"user\"}\n",
        );
        home
    }

    #[test]
    fn a_session_is_found_by_its_id_wherever_its_directory_put_it_and_nothing_else_is() {
        let home = home_with_session("2.1.296");
        assert!(find_transcript(home.path(), ID).is_some());
        assert!(find_transcript(home.path(), "22222222-2222-4222-8222-222222222222").is_none());
        assert!(find_transcript(home.path(), "../../etc/passwd").is_none());
        assert!(find_transcript(home.path(), "").is_none());
    }

    #[tokio::test]
    async fn a_session_exported_and_imported_into_another_home_is_there_whole_with_its_sub_agents()
    {
        let source = home_with_session("2.1.296");
        let out = tempfile::tempdir().unwrap();
        let archive = export_transcript(source.path(), ID, out.path())
            .await
            .unwrap();
        assert_eq!(archive.format, TRANSCRIPT_FORMAT);
        let target = tempfile::tempdir().unwrap();
        let id = import_transcript(target.path(), true, &archive, "2.1.296")
            .await
            .unwrap();
        assert_eq!(id, ID);
        let kept = target.path().join("projects/orch");
        assert_eq!(
            std::fs::read_to_string(kept.join(format!("{ID}.jsonl"))).unwrap(),
            std::fs::read_to_string(find_transcript(source.path(), ID).unwrap()).unwrap()
        );
        assert!(kept.join(ID).join("subagents/agent-a1.jsonl").is_file());
        assert!(find_transcript(target.path(), ID).is_some());
    }

    #[tokio::test]
    async fn a_transcript_from_a_version_outside_the_range_is_refused() {
        let source = home_with_session("2.1.100");
        let out = tempfile::tempdir().unwrap();
        let archive = export_transcript(source.path(), ID, out.path())
            .await
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let error = import_transcript(target.path(), true, &archive, "2.1.296")
            .await
            .unwrap_err();
        assert!(matches!(error, HistoryError::OtherVersion(ref v) if v == "2.1.100"));
        assert!(
            !target.path().join("projects").exists(),
            "nothing was written"
        );
    }

    #[tokio::test]
    async fn an_archive_that_names_a_path_outside_the_session_is_refused() {
        let out = tempfile::tempdir().unwrap();
        let path = out.path().join("evil.orch-claude-code");
        let header = format!(
            "{{\"session_id\":\"{ID}\",\"harness_version\":\"2.1.296\",\"files\":[{{\"path\":\"../../escape\",\"size\":1}}]}}"
        );
        tokio::fs::write(&path, format!("{ARCHIVE_MAGIC}\n{header}\nx"))
            .await
            .unwrap();
        let archive = SessionArchive {
            format: TRANSCRIPT_FORMAT.into(),
            path: path.display().to_string(),
            size: 0,
        };
        let target = tempfile::tempdir().unwrap();
        assert!(matches!(
            import_transcript(target.path(), true, &archive, "2.1.296").await,
            Err(HistoryError::NotAnArchive(_))
        ));
    }

    #[tokio::test]
    async fn a_file_that_is_no_archive_is_refused() {
        let out = tempfile::tempdir().unwrap();
        let path = out.path().join("junk");
        tokio::fs::write(&path, "hello\nworld\n").await.unwrap();
        let archive = SessionArchive {
            format: TRANSCRIPT_FORMAT.into(),
            path: path.display().to_string(),
            size: 0,
        };
        assert!(matches!(
            import_transcript(out.path(), true, &archive, "2.1.296").await,
            Err(HistoryError::NotAnArchive(_))
        ));
    }

    #[tokio::test]
    async fn a_checked_candidate_transcript_requires_the_same_loaded_version() {
        let source = home_with_session(PORTABLE_ARCHIVE_CANDIDATE);
        let out = tempfile::tempdir().unwrap();
        let archive = export_transcript(source.path(), ID, out.path())
            .await
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let id = import_transcript(target.path(), true, &archive, PORTABLE_ARCHIVE_CANDIDATE)
            .await
            .unwrap();
        assert_eq!(id, ID);
        assert_eq!(
            std::fs::read_to_string(find_transcript(target.path(), ID).unwrap()).unwrap(),
            std::fs::read_to_string(find_transcript(source.path(), ID).unwrap()).unwrap()
        );
        let different = tempfile::tempdir().unwrap();
        assert!(matches!(
            import_transcript(different.path(), true, &archive, "2.1.296").await,
            Err(HistoryError::VersionMismatch { .. })
        ));
        assert!(!different.path().join("projects").exists());

        let old_source = home_with_session("2.1.296");
        let old_out = tempfile::tempdir().unwrap();
        let old_archive = export_transcript(old_source.path(), ID, old_out.path())
            .await
            .unwrap();
        let candidate_home = tempfile::tempdir().unwrap();
        assert!(matches!(
            import_transcript(
                candidate_home.path(),
                true,
                &old_archive,
                PORTABLE_ARCHIVE_CANDIDATE,
            )
            .await,
            Err(HistoryError::VersionMismatch { .. })
        ));
        assert!(!candidate_home.path().join("projects").exists());
    }

    /// A session as Claude Code 2.1.296 keeps it: two turns, a command and an edit, a picture given with
    /// words, and a line of a sub-agent's own conversation.
    const SESSION: &str = include_str!("../tests/fixtures/adopt/session.jsonl");
    const SESSION_ID: &str = "5b0c3e9a-1f2d-4c8b-9a7e-2d4f6b8c0e12";

    fn adopted_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        write(
            &home
                .path()
                .join("projects")
                .join("-work-web")
                .join(format!("{SESSION_ID}.jsonl")),
            SESSION,
        );
        home
    }

    fn clock() -> Arc<dyn Clock> {
        Arc::new(orch_types::ManualClock::new(
            chrono::DateTime::from_timestamp(1_791_800_000, 0).unwrap(),
        ))
    }

    fn kind_of(entry: &RecordedEntry) -> String {
        match entry {
            RecordedEntry::Said(_) => "said".to_owned(),
            RecordedEntry::Item(item) => match &item.content {
                orch_agent_contract::events::ItemContent::ToolCall(call) => {
                    serde_json::to_value(&call.tool).unwrap()["kind"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                }
                other => serde_json::to_value(other.kind())
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            },
        }
    }

    #[test]
    fn replay_restores_history_without_reemitting_its_old_token_use() {
        let home = adopted_home();
        let transcript = find_transcript(home.path(), SESSION_ID).unwrap();
        assert!(
            SESSION.contains("\"usage\""),
            "the fixture contains historical token reports"
        );
        let mut replayed = replay(
            &transcript,
            MapperSettings {
                directory: "/work/web".to_owned(),
                cwd: "/work/web".to_owned(),
                raw_events: false,
            },
            clock(),
        )
        .unwrap();
        let events = replayed.by_ref().collect::<Result<Vec<_>, _>>().unwrap();
        assert!(replayed.items > 0);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::TurnCompleted(_)))
                .count(),
            2
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SessionEvent::UsageUpdated(_))),
            "reading a transcript cannot charge its old turns again"
        );
    }

    #[test]
    fn a_transcript_s_record_holds_the_person_s_words_and_the_agent_s_steps_in_their_order() {
        let home = adopted_home();
        let transcript = find_transcript(home.path(), SESSION_ID).unwrap();
        let settings = MapperSettings {
            directory: "/work/web".to_owned(),
            cwd: "/work/web".to_owned(),
            raw_events: false,
        };
        let mut recorded = record(&transcript, settings, clock()).unwrap();
        let model = recorded.model.clone();
        let turns = recorded.selected_turns(RECORD_MAX_BYTES).unwrap();
        assert_eq!(
            model.as_deref(),
            Some("claude-sonnet-4-5"),
            "the model of its last answer"
        );
        assert_eq!(turns.len(), 2);
        let first: Vec<String> = turns[0].iter().map(kind_of).collect();
        assert_eq!(
            first,
            [
                "said",
                "agent_message",
                "command",
                "file_change",
                "agent_message"
            ],
            "what was asked, then each step where it began"
        );
        let RecordedEntry::Said(asked) = &turns[0][0] else {
            unreachable!("checked above");
        };
        assert_eq!(asked.text, "Fix the login: the button does nothing");
        assert_eq!(
            asked.native_ref,
            "said:00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            asked
                .at
                .map(|at| at.to_string())
                .as_deref()
                .map(|at| &at[..19]),
            Some("2026-10-10T09:00:00")
        );
        let RecordedEntry::Item(command) = &turns[0][2] else {
            unreachable!("checked above");
        };
        let orch_agent_contract::events::ItemContent::ToolCall(call) = &command.content else {
            unreachable!("checked above");
        };
        assert_eq!(call.raw.name, "Bash");
        assert!(
            call.raw
                .output
                .as_ref()
                .is_some_and(|output| output.inline.contains("onClick={undefined}")),
            "with what came back: {:?}",
            call.raw.output
        );
        let RecordedEntry::Said(second) = &turns[1][0] else {
            panic!(
                "the second turn begins with what was asked: {:?}",
                turns[1][0]
            );
        };
        assert_eq!(second.text, "Does it look right now?");
        assert_eq!(second.left_out, 1, "the picture is counted and not carried");
        assert!(
            !turns
                .concat()
                .iter()
                .any(|entry| serde_json::to_string(entry)
                    .unwrap()
                    .contains("a sub-agent's own task")),
            "a sub-agent's own conversation is not the session's"
        );
    }

    #[test]
    fn a_record_is_written_for_the_connector_and_reads_back_whole() {
        let home = adopted_home();
        let transcript = find_transcript(home.path(), SESSION_ID).unwrap();
        let into = home.path().join("records");
        let counted = read_record(&transcript, SESSION_ID, false, &into, clock()).unwrap();
        assert!(
            counted.file.is_none(),
            "nothing is written unless it is asked for"
        );
        assert_eq!(counted.turns, 2);
        let written = read_record(&transcript, SESSION_ID, true, &into, clock()).unwrap();
        let file = written.file.unwrap();
        assert_eq!(file.format, RECORD_FORMAT);
        let text = std::fs::read_to_string(&file.path).unwrap();
        assert_eq!(usize::try_from(file.size).unwrap(), text.len());
        let (entries, unreadable) = orch_agent_contract::discovery::read_record(&text);
        assert_eq!(unreadable, 0);
        assert_eq!(i64::try_from(entries.len()).unwrap(), written.entries);
        assert_eq!(written.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(written.report.by_kind["user_message"].kept, 2);
        assert_eq!(
            written.report.by_kind["attachment"].dropped, 1,
            "the picture is said to be left out"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unsupported_transcript_is_refused_before_temporary_storage_is_requested() {
        const SOURCE: &str = "ORCH_UNSUPPORTED_TRANSCRIPT_SOURCE";
        if let Some(path) = std::env::var_os(SOURCE) {
            let error = read_record(
                Path::new(&path),
                "unsupported",
                false,
                Path::new("."),
                clock(),
            )
            .unwrap_err();
            assert!(matches!(error, HistoryError::OtherVersion(version) if version == "1.0.0"));
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("unsupported.jsonl");
        std::fs::write(&path, "{\"version\":\"1.0.0\"}\n").unwrap();
        let unavailable = home.path().join("not-a-temporary-directory");
        std::fs::write(&unavailable, "a regular file").unwrap();
        // A separate process avoids changing the temporary directory of parallel tests.
        let output = std::process::Command::new("timeout").arg("30s").arg(std::env::current_exe().unwrap())
            .args(["--exact", "history::tests::an_unsupported_transcript_is_refused_before_temporary_storage_is_requested", "--test-threads=1"])
            .env(SOURCE, &path).env("TMPDIR", unavailable).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\"version\":\"1.0.0\"}\n"
        );
    }

    #[test]
    fn a_transcript_of_a_version_older_than_the_adapter_drives_is_not_read() {
        let home = tempfile::tempdir().unwrap();
        let path = home
            .path()
            .join("projects")
            .join("-w")
            .join(format!("{SESSION_ID}.jsonl"));
        write(&path, &SESSION.replace("2.1.296", "1.0.0"));
        let settings = MapperSettings {
            directory: "/work/web".to_owned(),
            cwd: "/work/web".to_owned(),
            raw_events: false,
        };
        assert!(matches!(
            record(&path, settings, clock()),
            Err(HistoryError::OtherVersion(version)) if version == "1.0.0"
        ));
    }
}
