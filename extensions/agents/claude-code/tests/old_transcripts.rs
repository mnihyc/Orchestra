//! What the adapter does with a session when the program that wrote it and the program that runs now are not
//! the same build: adopting a session a person began under another build, and continuing an Orch thread under a
//! program that updated itself in between. The programs here only say their version; the transcripts are the
//! recorded session of the adoption cases, with the build that wrote them replaced where a case needs it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_agent_contract::adapter::{AgentAdapter, AgentStart, EventSink, EventStream};
use orch_agent_contract::discovery::SessionRecord;
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::history::ExportKind;
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::SessionMode;
use orch_plugin_contract::Secret;
use orch_types::{SystemClock, new_id};
use serde_json::json;

/// The recording of a session of two turns; the build that wrote it is [`RECORDED`].
const SESSION: &str = include_str!("fixtures/adopt/session.jsonl");
/// The build the recording was made with: older than the pinned one.
const RECORDED: &str = "2.1.296";
const SESSION_ID: &str = "5b0c3e9a-1f2d-4c8b-9a7e-2d4f6b8c0e12";

/// The build the adapter is pinned to, and one that came out after it.
const PINNED: &str = "2.1.322";
const NEWER: &str = "2.2.0";

/// The programs the tests start, written once and before any of them is run (a program that is being written
/// while another thread starts a process cannot be started).
struct Programs {
    _directory: tempfile::TempDir,
    pinned: PathBuf,
    newer: PathBuf,
}

fn programs() -> &'static Programs {
    static PROGRAMS: OnceLock<Programs> = OnceLock::new();
    PROGRAMS.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, version: &str| {
            let path = directory.path().join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho '{version} (Claude Code)'\n"),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path
        };
        Programs {
            pinned: write("pinned", PINNED),
            newer: write("newer", NEWER),
            _directory: directory,
        }
    })
}

struct Host {
    adapter: ClaudeCodeAdapter,
    home: tempfile::TempDir,
    /// The events of the session are not read here; the stream stays open until the test ends.
    _events: EventStream,
}

/// A home holding the recorded session as the build `written_by` wrote it, and an adapter started on the
/// program `running`.
async fn host(running: &Path, written_by: &str) -> Host {
    let home = tempfile::tempdir().unwrap();
    let transcript = home
        .path()
        .join("projects")
        .join("-work-web")
        .join(format!("{SESSION_ID}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, SESSION.replace(RECORDED, written_by)).unwrap();
    let (sink, events) = EventSink::channel();
    let mut adapter = ClaudeCodeAdapter::new(Arc::new(SystemClock));
    adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                provider_kind: None,
                agent_id: new_id(),
                harness_kind: "claude-code".into(),
                profile: AgentProfile::default(),
                home: Some(home.path().display().to_string()),
                provider_config: json!({}),
                loopback: Loopback::new(41_000, Secret::new("t")),
                binary: Some(running.display().to_string()),
            },
            sink,
        )
        .await
        .unwrap();
    Host {
        adapter,
        home,
        _events: events,
    }
}

fn said(record: &SessionRecord) -> String {
    record.report.warnings.join(" | ")
}

/// Adopting a session: the adapter's record of it (`agent.read_session`).
async fn adopt(
    host: &mut Host,
) -> Result<SessionRecord, orch_agent_contract::adapter::AdapterError> {
    host.adapter.read_session(SESSION_ID, false).await
}

/// The session as an Orch thread opens it again (an ordinary resume) and then reads its transcript back.
async fn resume_and_resync(
    host: &mut Host,
) -> Result<u32, orch_agent_contract::adapter::AdapterError> {
    let thread = new_id();
    let directory = host.home.path().display().to_string();
    let spec = common::fixture_spec(
        thread,
        new_id(),
        &directory,
        "session-key",
        SessionMode::Resume,
        Some(SESSION_ID),
    );
    let opened = host.adapter.open_session(spec).await?;
    assert!(opened.resumed, "the conversation the harness already holds");
    assert_eq!(opened.native_session_ref, SESSION_ID);
    host.adapter.resync_session(thread).await
}

// (a) A session begun under a build older than the pin.

#[tokio::test]
async fn a_session_begun_under_an_older_build_than_the_pin_is_adopted_with_its_record() {
    let mut host = host(&programs().pinned, RECORDED).await;
    let record = adopt(&mut host)
        .await
        .expect("a transcript of the same format written by an older build is read");
    assert_eq!(record.turns, 2);
    assert_eq!(record.report.by_kind["user_message"].kept, 2);
    assert!(
        said(&record).contains(RECORDED),
        "the build that wrote the session is told: {}",
        said(&record)
    );
}

#[tokio::test]
async fn a_session_begun_under_a_much_older_build_is_read_and_said_to_be_beyond_what_the_cases_cover()
 {
    let mut host = host(&programs().pinned, "2.1.100").await;
    let record = adopt(&mut host)
        .await
        .expect("an older build's transcript is read, not refused by its version");
    assert_eq!(record.turns, 2);
    let said = said(&record);
    assert!(
        said.contains("2.1.100") && said.contains(RECORDED),
        "the build that wrote it and the oldest build whose transcripts are covered are named: {said}"
    );
}

// (b) A session begun under a build newer than the pin.

#[tokio::test]
async fn a_session_begun_under_a_newer_build_than_the_pin_is_adopted_with_its_record() {
    let mut host = host(&programs().pinned, NEWER).await;
    let record = adopt(&mut host)
        .await
        .expect("a newer build's transcript is read");
    assert_eq!(record.turns, 2);
    assert!(said(&record).contains(NEWER), "{}", said(&record));
}

#[tokio::test]
async fn a_session_written_by_the_pinned_build_carries_no_note_of_its_writer() {
    let mut host = host(&programs().pinned, PINNED).await;
    let record = adopt(&mut host).await.unwrap();
    assert_eq!(record.turns, 2);
    assert!(said(&record).is_empty(), "{}", said(&record));
}

// (c) An Orch thread begun under the pin, continued by a program that updated itself.

#[tokio::test]
async fn a_thread_begun_under_the_pin_is_resumed_and_read_back_by_a_newer_program() {
    let mut host = host(&programs().newer, PINNED).await;
    let rows = resume_and_resync(&mut host)
        .await
        .expect("it resumes and is read back");
    assert!(rows > 0);
}

#[tokio::test]
async fn a_thread_begun_under_an_older_build_than_the_pin_is_resumed_and_read_back() {
    let mut host = host(&programs().pinned, RECORDED).await;
    let rows = resume_and_resync(&mut host)
        .await
        .expect("the older transcript is resumed by name, and read back");
    assert!(rows > 0);
}

#[tokio::test]
async fn a_thread_begun_under_an_older_build_is_resumed_and_read_back_by_a_newer_program() {
    let mut host = host(&programs().newer, RECORDED).await;
    let rows = resume_and_resync(&mut host)
        .await
        .expect("it resumes and is read back");
    assert!(rows > 0);
}

// What stays refused or is said: a row that is not a row.

#[tokio::test]
async fn a_row_of_the_transcript_that_cannot_be_read_is_left_out_and_said_with_its_place() {
    let mut host = host(&programs().pinned, PINNED).await;
    let transcript = host
        .home
        .path()
        .join("projects")
        .join("-work-web")
        .join(format!("{SESSION_ID}.jsonl"));
    let mut lines: Vec<String> = std::fs::read_to_string(&transcript)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    lines.insert(3, "{\"type\":\"user\",\"message\":".to_owned());
    std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
    let record = adopt(&mut host).await.unwrap();
    assert_eq!(record.turns, 2, "the rest is read");
    let said = said(&record);
    assert!(
        said.contains("row 4") && said.contains("not valid JSON"),
        "the row and the reason are named: {said}"
    );
}

// A session carried to another home as an archive of its transcript.

/// The archive of the recorded session as `exporter` makes it, written by `written_by`.
async fn exported(
    exporter: &Path,
    written_by: &str,
) -> orch_agent_contract::history::SessionArchive {
    let mut source = host(exporter, written_by).await;
    let thread = new_id();
    let directory = source.home.path().display().to_string();
    let spec = common::fixture_spec(
        thread,
        new_id(),
        &directory,
        "session-key",
        SessionMode::Resume,
        Some(SESSION_ID),
    );
    source.adapter.open_session(spec).await.unwrap();
    source
        .adapter
        .export_session(thread, ExportKind::Native, None)
        .await
        .unwrap()
}

async fn import(
    target: &mut Host,
    archive: orch_agent_contract::history::SessionArchive,
) -> Result<orch_agent_contract::session::SessionOpened, orch_agent_contract::adapter::AdapterError>
{
    let directory = target.home.path().display().to_string();
    let mut spec = common::fixture_spec(
        new_id(),
        new_id(),
        &directory,
        "session-key",
        SessionMode::ImportNative,
        None,
    );
    spec.archive = Some(archive);
    target.adapter.open_session(spec).await
}

#[tokio::test]
async fn an_archive_of_the_pinned_build_is_imported_by_the_pinned_build() {
    let archive = exported(&programs().pinned, PINNED).await;
    let mut target = host(&programs().pinned, PINNED).await;
    std::fs::remove_dir_all(target.home.path().join("projects")).unwrap();
    let opened = import(&mut target, archive).await.unwrap();
    assert_eq!(opened.native_session_ref, SESSION_ID);
}

#[tokio::test]
async fn an_archive_of_another_build_is_refused_as_unsupported_so_that_the_carry_goes_on_another_way()
 {
    use orch_agent_contract::adapter::AdapterError;
    for (exporter, written_by, importer) in [
        (&programs().pinned, RECORDED, &programs().pinned),
        (&programs().newer, NEWER, &programs().pinned),
        (&programs().pinned, PINNED, &programs().newer),
    ] {
        let archive = exported(exporter, written_by).await;
        let mut target = host(importer, PINNED).await;
        std::fs::remove_dir_all(target.home.path().join("projects")).unwrap();
        let refused = import(&mut target, archive).await.unwrap_err();
        assert!(
            matches!(refused, AdapterError::Unsupported(_)),
            "{written_by}: {refused:?}"
        );
    }
}
