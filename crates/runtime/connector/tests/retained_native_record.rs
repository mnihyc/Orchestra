//! A native record publishes only after its typed output dependencies are retained through the standing path.
mod common;
use async_trait::async_trait;
use common::Rig;
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::adapter::{
    AdapterError, AgentAdapter, AgentStart, AgentStarted, EventSink,
};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::discovery::{RECORD_FORMAT, RecordedEntry, SessionRecord, record_report};
use orch_agent_contract::history::{HistoryReport, SessionArchive};
use orch_agent_contract::session::{SessionOpened, SessionSpec, ThreadId, TurnStart};
use orch_connector_core::harnesses::AdapterRegistry;
use orch_link::commands::Progress;
use orch_link::content::{BlobEndpoint, PutRefusal};
use orch_link_contract::commands::{AgentReadSession, RecordRef};
use orch_types::{Hash, Json, new_id};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error>>;
struct Fixture {
    directory: PathBuf,
    bytes: Vec<u8>,
    report: HistoryReport,
    count: i64,
    image: Hash,
    raw: Hash,
    hidden: Hash,
}
struct RecordAdapter {
    inner: FakeAdapter,
    fixture: Arc<Fixture>,
}
#[async_trait]
impl AgentAdapter for RecordAdapter {
    fn kind(&self) -> &'static str {
        "fake"
    }
    fn output_directory(&self) -> Option<PathBuf> {
        Some(self.fixture.directory.join("outputs"))
    }
    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        self.inner.start(start, sink).await
    }
    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.inner.stop().await
    }
    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        self.inner.probe().await
    }
    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        self.inner.open_session(spec).await
    }
    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        self.inner.close_session(thread).await
    }
    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        self.inner.start_turn(turn).await
    }
    async fn read_session(
        &mut self,
        native: &str,
        write: bool,
    ) -> Result<SessionRecord, AdapterError> {
        if native != "fixture-native" {
            return Err(AdapterError::NotFound(native.into()));
        }
        let file = if write {
            let path = self.fixture.directory.join("native-record.jsonl");
            std::fs::write(&path, &self.fixture.bytes)
                .map_err(|error| AdapterError::HarnessError(error.to_string()))?;
            Some(SessionArchive {
                format: RECORD_FORMAT.into(),
                path: path.to_string_lossy().into_owned(),
                size: i64::try_from(self.fixture.bytes.len())
                    .map_err(|error| AdapterError::Internal(error.to_string()))?,
            })
        } else {
            None
        };
        Ok(SessionRecord {
            model: Some("fake-large".into()),
            effort: None,
            turns: 1,
            entries: self.fixture.count,
            file,
            report: self.fixture.report.clone(),
        })
    }
}
fn fixture(rig: &Rig, include_image: bool) -> Result<Arc<Fixture>, Box<dyn std::error::Error>> {
    let directory = rig.data.path().join("record-fixture");
    let outputs = directory.join("outputs");
    std::fs::create_dir_all(&outputs)?;
    let image_bytes = b"native returned image bytes";
    let raw_bytes = b"the complete native tool result";
    let hidden_bytes = b"a private opaque value is not an output reference";
    let (image, raw, hidden) = (
        Hash::of(image_bytes),
        Hash::of(raw_bytes),
        Hash::of(hidden_bytes),
    );
    if include_image {
        std::fs::write(outputs.join(image.to_string()), image_bytes)?;
    }
    std::fs::write(outputs.join(raw.to_string()), raw_bytes)?;
    std::fs::write(outputs.join(hidden.to_string()), hidden_bytes)?;
    let entries: Vec<RecordedEntry> = [
        json!({"kind":"image","blob":image,"media_type":"image/png","name":"native.png","caption":null}),
        json!({"kind":"mcp","server":"fixture","tool":"show","arguments":{"blob":hidden},"result":{"blob":hidden},"error":null}),
    ].into_iter().enumerate().map(|(index, tool)| serde_json::from_value(json!({
        "entry":"item","native_ref":format!("record-{index}"),"turn":1,"stream":"main","parent_native_ref":null,
        "status":"completed","at":null,"content":{"kind":"tool_call","payload":{
            "tool":tool,"title":"Native result","started_at":"2026-01-01T00:00:00Z","duration_ms":1,
            "raw":{"name":"native_result","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},
                "output":{"inline":"","total_len":raw_bytes.len(),"truncated":true,"blob":raw},"is_error":false}
        }}
    }))).collect::<Result<_,_>>()?;
    let mut bytes = Vec::new();
    for entry in &entries {
        serde_json::to_writer(&mut bytes, entry)?;
        bytes.push(b'\n');
    }
    let count = i64::try_from(entries.len())?;
    Ok(Arc::new(Fixture {
        directory,
        bytes,
        report: record_report(&[entries], 0),
        count,
        image,
        raw,
        hidden,
    }))
}
async fn start(rig: &mut Rig, fixture: Arc<Fixture>) {
    let mut registration = common::fake_registration();
    registration.factory = Arc::new(move |clock| {
        Box::new(RecordAdapter {
            inner: FakeAdapter::new(clock),
            fixture: Arc::clone(&fixture),
        })
    });
    rig.adapters = AdapterRegistry::new();
    // Registration is fixture setup; preserve an ordinary test failure without a suppressed lint.
    assert!(rig.adapters.register(registration).is_ok());
    rig.start().await;
    rig.agent_start(json!({})).await;
}
fn request(rig: &Rig, write: bool) -> AgentReadSession {
    AgentReadSession {
        provider_id: rig.provider,
        agent_id: rig.agent,
        native_session_ref: "fixture-native".into(),
        write,
    }
}

async fn read_session(rig: &Rig, write: bool) -> Result<Progress, Box<dyn std::error::Error>> {
    let command = new_id();
    // Authorize this export before it can run, independently of the standing output account.
    if write {
        rig.blobs.open_command(command);
    }
    rig.controller.issue_with_id(
        command,
        &rig.harness_key(),
        rig.deadline(),
        &request(rig, write),
    );
    Ok(tokio::time::timeout(rig.patience, rig.controller.wait_command(command)).await?)
}

fn succeeded(progress: Progress) -> Result<Json, Box<dyn std::error::Error>> {
    match progress {
        Progress::Succeeded(answer) => Ok(answer),
        other => Err(format!("the native record command did not succeed: {other:?}").into()),
    }
}

#[tokio::test]
async fn a_record_keeps_images_and_raw_outputs_before_publishing_and_ignores_opaque_hashes()
-> TestResult {
    let mut rig = Rig::new();
    let fixture = fixture(&rig, true)?;
    start(&mut rig, Arc::clone(&fixture)).await;
    let preview = succeeded(read_session(&rig, false).await?)?;
    assert!(preview["record"].is_null());
    assert!(rig.blobs.content(&fixture.image).is_none());
    let answer = succeeded(read_session(&rig, true).await?)?;
    let record: RecordRef = serde_json::from_value(answer["record"].clone())?;
    assert!(
        matches!(rig.link.head(new_id(), &record.blob).await,
            Err(PutRefusal::Refused(detail)) if detail == "the command is not open"),
        "authorizing the issued export never opens an unrelated command account"
    );
    assert_eq!(
        rig.blobs.content(&record.blob).as_deref(),
        Some(fixture.bytes.as_slice())
    );
    assert_eq!(
        rig.blobs.content(&fixture.image).as_deref(),
        Some(b"native returned image bytes".as_slice())
    );
    assert_eq!(
        rig.blobs.content(&fixture.raw).as_deref(),
        Some(b"the complete native tool result".as_slice())
    );
    assert!(
        rig.blobs.content(&fixture.hidden).is_none(),
        "opaque MCP JSON grants no cache access"
    );
    assert!(
        !fixture.directory.join("native-record.jsonl").exists(),
        "the uploaded export is still temporary"
    );
    assert!(
        fixture
            .directory
            .join("outputs")
            .join(fixture.image.to_string())
            .exists(),
        "retained outputs survive export cleanup"
    );
    if let Some(connector) = rig.connector.take() {
        connector.stop().await;
    }
    Ok(())
}
#[tokio::test]
async fn a_record_with_missing_typed_image_bytes_never_publishes_a_dangling_record() -> TestResult {
    let mut rig = Rig::new();
    let fixture = fixture(&rig, false)?;
    start(&mut rig, Arc::clone(&fixture)).await;
    let answer = read_session(&rig, true).await?;
    assert!(
        matches!(answer, Progress::Failed(ref error)
        if error.detail.starts_with("a file cannot be read:")),
        "{answer:?}"
    );
    assert!(rig.blobs.content(&Hash::of(&fixture.bytes)).is_none());
    assert!(rig.blobs.content(&fixture.image).is_none());
    assert!(
        !fixture.directory.join("native-record.jsonl").exists(),
        "a refused record's temporary export is removed too"
    );
    if let Some(connector) = rig.connector.take() {
        connector.stop().await;
    }
    Ok(())
}
