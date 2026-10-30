//! Native ingress, real guarded selection and source/effect ordering, without vendor-history claims.

#[path = "../../../tests/native_continuations/mod.rs"]
mod native;

use super::*;
use crate::test_common as common;
use crate::tools::CallContext;
use bytes::Bytes;
use orch_agent_contract::history::NativePrefixSource;
use orch_api_contract::threads::{
    ContentPart, ThreadContinuationAccepted, ThreadPromptVersionCreateBody, TokenUsage,
};
use orch_authz::{Caller, Principal};
use orch_blob_contract::store::{BlobStore, read_all, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_link::commands::{CommandContext, CommandHandler};
use orch_link_contract::{
    commands::{
        Command, CommandError, CommandKind, CommandResult, ErrorCode, OrchTool, OrchToolResult,
        TurnStart,
    },
    events::{Event, OrchToolCall, TurnOutcome, UsageUpdated},
    streams::Stream,
};
use orch_types::{Hash, Json, new_id};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, PoisonError},
};
use tokio::sync::{Semaphore, oneshot};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Point {
    Prepared,
    Read,
    Native,
    NativeDisplay,
    NativePrepared,
    NativeCompleted,
    AnswerRetained,
}
struct Gate {
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}
type GateKey = (Id, String, Point);
static GATES: OnceLock<Mutex<HashMap<GateKey, Gate>>> = OnceLock::new();
type OwnedTasks = HashMap<(Id, String), Option<tokio::task::JoinHandle<()>>>;
static OWNED_TASKS: OnceLock<Mutex<OwnedTasks>> = OnceLock::new();

fn expect_task(thread: Id, call: &str) {
    assert!(
        OWNED_TASKS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((thread, call.into()), None)
            .is_none()
    );
}

pub(crate) fn track_task(identity: (Id, String), task: tokio::task::JoinHandle<()>) {
    if let Some(slot) = OWNED_TASKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_mut(&identity)
    {
        assert!(
            slot.is_none(),
            "only the explicitly owned first native task is interrupted"
        );
        *slot = Some(task);
    }
}

async fn interrupt_task(thread: Id, call: &str) -> Proof {
    let task = OWNED_TASKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(thread, call.into()))
        .flatten()
        .ok_or("the actual admitted tool task was not owned")?;
    task.abort();
    assert!(task.await.is_err_and(|error| error.is_cancelled()));
    Ok(())
}

fn gate(thread: Id, call: &str, point: Point) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached, waiting) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    let old = GATES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            (thread, call.into(), point),
            Gate {
                reached,
                release: resume,
            },
        );
    assert!(old.is_none(), "each test phase owns its exact call once");
    (waiting, release)
}

/// Only a named test call pauses; neither SQL locks nor pool leases enter this boundary.
pub(crate) async fn pause(call: &CallContext, point: Point) {
    pause_key((call.thread, call.call_ref.clone(), point)).await;
}

async fn pause_key(key: GateKey) {
    let gate = GATES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&key);
    if let Some(gate) = gate {
        let _ = gate.reached.send(());
        let _ = gate.release.await;
    }
}

pub(crate) async fn native_admitted(thread: Id, answer: Id, kind: CommandKind) {
    pause_key((thread, answer.to_string(), Point::Native)).await;
    if kind == CommandKind::DisplayOpen {
        pause_key((thread, answer.to_string(), Point::NativeDisplay)).await;
    }
}

pub(crate) async fn prepared_native(thread: Id, answer: Id) {
    pause_key((thread, answer.to_string(), Point::NativePrepared)).await;
}

pub(crate) async fn completed_native(thread: Id) {
    if let Ok(answer) = super::SOURCE.try_with(|source| source.answer) {
        pause_key((thread, answer.to_string(), Point::NativeCompleted)).await;
    }
}

pub(crate) async fn answer_retained(thread: Id, answer: Id) {
    pause_key((thread, answer.to_string(), Point::AnswerRetained)).await;
}

async fn reached(phase: &str, signal: oneshot::Receiver<()>) -> Proof {
    match tokio::time::timeout(common::PATIENCE, signal).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(format!(
            "source-effect phase {phase}: the owned task ended before reaching its boundary"
        )
        .into()),
        Err(_) => Err(format!(
            "source-effect phase {phase}: the original native proof budget elapsed"
        )
        .into()),
    }
}

/// Model an already-running native operation that cannot immediately stop its current I/O.
/// The real command server still owns its original environment key and durable journal.
struct UnjoinedWrite {
    inner: Arc<orch_testkit::doubles::machine::ScriptedHandler>,
    store: Arc<LocalBlobStore>,
    bytes: Arc<Mutex<Option<(Hash, Bytes)>>>,
    hold: bool,
    entered: Semaphore,
    release: Semaphore,
}

#[async_trait::async_trait]
impl CommandHandler for UnjoinedWrite {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        if command.kind() == CommandKind::FsWrite {
            self.entered.add_permits(1);
            if self.hold {
                self.release
                    .acquire()
                    .await
                    .map_err(|_| {
                        CommandError::new(ErrorCode::Internal, "the owned write phase ended")
                    })?
                    .forget();
            }
            let Command::FsWrite(body) = &command else {
                unreachable!("the variant was checked")
            };
            // The native peer obtains its bytes from the actual admitted store only now,
            // after any cancellation/GC phase. Oracle bytes held by the test are not its input.
            let found = self.store.get(&body.blob, None).await.map_err(|_| {
                CommandError::new(ErrorCode::NotFound, "the admitted native blob is absent")
            })?;
            let bytes = read_all(found.body).await.map_err(|_| {
                CommandError::new(ErrorCode::Internal, "the admitted blob could not be read")
            })?;
            if Hash::of(&bytes) != body.blob {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the admitted blob bytes changed",
                ));
            }
            *self.bytes.lock().unwrap_or_else(PoisonError::into_inner) = Some((body.blob, bytes));
        }
        self.inner.handle(context, command).await
    }
}

struct StoredNativeInput(Arc<LocalBlobStore>);

#[async_trait::async_trait]
impl orch_envhost::blobs::BlobSource for StoredNativeInput {
    async fn get(
        &self,
        hash: Hash,
        target: &mut dyn orch_envhost::blobs::BlobWriter,
    ) -> Result<i64, orch_envhost::blobs::BlobError> {
        use orch_envhost::blobs::BlobError;
        let found = self
            .0
            .get(&hash, None)
            .await
            .map_err(|_| BlobError::Refused("the admitted content is absent".into()))?;
        let bytes = read_all(found.body)
            .await
            .map_err(|_| BlobError::Local("the admitted content could not be read".into()))?;
        if Hash::of(&bytes) != hash {
            return Err(BlobError::HashMismatch);
        }
        let size = i64::try_from(bytes.len())
            .map_err(|_| BlobError::Local("the synthetic content is too large".into()))?;
        let offset = target
            .received()
            .await
            .map_err(|error| BlobError::Local(error.to_string()))?;
        let begin = usize::try_from(offset).map_err(|_| BlobError::HashMismatch)?;
        if begin > bytes.len() {
            return Err(BlobError::HashMismatch);
        }
        target
            .write(offset, bytes.slice(begin..))
            .await
            .map_err(|error| BlobError::Local(error.to_string()))?;
        if target
            .hash()
            .await
            .map_err(|error| BlobError::Local(error.to_string()))?
            != hash
        {
            return Err(BlobError::HashMismatch);
        }
        Ok(size)
    }
}

/// The actual directory host retires its attachment; only the surrounding unrelated
/// harness commands remain scripted. The command server owns ordering and journaling.
struct AttachedPeer {
    inner: Arc<orch_testkit::doubles::machine::ScriptedHandler>,
    host: orch_envhost::environment::EnvHost,
    blobs: StoredNativeInput,
    entered: Semaphore,
    release: Semaphore,
    writes: std::sync::atomic::AtomicUsize,
    detached: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl CommandHandler for AttachedPeer {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        use std::sync::atomic::Ordering;
        match &command {
            Command::FsWrite(body) => {
                self.entered.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .map_err(|_| {
                        CommandError::new(ErrorCode::Internal, "the owned native write gate ended")
                    })?
                    .forget();
                let result = self
                    .host
                    .fs_write(body, &self.blobs)
                    .await
                    .map_err(|error| error.command_error())?;
                self.writes.fetch_add(1, Ordering::SeqCst);
                Ok(CommandResult::FsWrite(result))
            }
            Command::EnvDetach(body) => {
                self.host
                    .env_detach(body)
                    .await
                    .map_err(|error| error.command_error())?;
                self.detached.store(true, Ordering::SeqCst);
                Ok(CommandResult::EnvDetach(
                    orch_link_contract::commands::Empty {},
                ))
            }
            _ => self.inner.handle(context, command).await,
        }
    }
}

struct Scene {
    world: common::World,
    thread: Id,
    start: TurnStart,
    input: Id,
    source: NativePrefixSource,
    document: Id,
    path: String,
    created: OrchToolResult,
    store: Arc<LocalBlobStore>,
    bytes: Bytes,
    blob: Hash,
    source_file_at: Time,
    directory: tempfile::TempDir,
}

impl Scene {
    async fn new() -> Proof<Self> {
        Self::configured(false).await
    }

    async fn configured(display: bool) -> Proof<Self> {
        let world = common::World::new().await;
        let mut descriptor = common::descriptor();
        descriptor["capabilities"]["native_input_prefix"] = json!("turn_start");
        descriptor["capabilities"]["orch_tools"] = json!("mcp");
        descriptor["capabilities"]["mcp"] =
            json!({"stdio":true,"http":true,"reload":false,"strict":true});
        let typed: orch_agent_contract::descriptor::AgentDescriptor =
            serde_json::from_value(descriptor.clone())?;
        sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
            .bind(world.agent)
            .bind(descriptor)
            .execute(world.pool.pool())
            .await?;
        let reported = typed.clone();
        world.machine.handler.on(CommandKind::AgentStart, move |_| {
            Ok(CommandResult::AgentStart(
                orch_link_contract::commands::AgentStartResult {
                    reused: false,
                    harness_version: "1.0.0".into(),
                    descriptor: reported.clone(),
                },
            ))
        });
        let thread = world.thread("Native source tools").await;
        let directory = tempfile::tempdir()?;
        tokio::fs::create_dir_all(directory.path().join("out")).await?;
        let bytes = Bytes::from_static(b"original\0source\xffbytes");
        tokio::fs::write(directory.path().join("out/result.bin"), &bytes).await?;
        let blob = Hash::of(&bytes);
        let size = i64::try_from(bytes.len())?;
        let store = Arc::new(LocalBlobStore::open(directory.path().join("blobs")).await?);
        store
            .put(&blob, Some(size), stream_of(vec![bytes.clone()]))
            .await?;
        world.engine().attach_blobs(store.clone());
        let environment = world.environment;
        let at = world.clock.time();
        world
            .machine
            .handler
            .on(CommandKind::FsStat, move |command| {
                let Command::FsStat(body) = command else {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "wrong file command",
                    ));
                };
                if body.environment_id != environment
                    || body.mount.is_some()
                    || !matches!(body.path.as_str(), "" | "web" | "web/out/result.bin")
                {
                    return Err(CommandError::new(
                        ErrorCode::NotFound,
                        "the synthetic environment has no such entry",
                    ));
                }
                let dir = body.path != "web/out/result.bin";
                Ok(CommandResult::FsStat(
                    orch_api_contract::workspace::FileStat {
                        entry: orch_api_contract::workspace::DirEntry {
                            name: if dir { "web" } else { "result.bin" }.into(),
                            path: body.path.clone(),
                            kind: if dir {
                                orch_api_contract::workspace::DirEntryKind::Dir
                            } else {
                                orch_api_contract::workspace::DirEntryKind::File
                            },
                            size: if dir { 0 } else { size },
                            mtime: at,
                            mode: 0o600,
                            link_target: None,
                            cached: false,
                        },
                        hash: (!dir).then_some(blob),
                        media_type: "application/octet-stream".into(),
                        is_text: false,
                    },
                ))
            });
        let file = directory.path().join("out/result.bin");
        world
            .machine
            .handler
            .on(CommandKind::FsFetch, move |command| {
                let Command::FsFetch(body) = command else {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "wrong file command",
                    ));
                };
                if body.environment_id != environment
                    || body.path != "web/out/result.bin"
                    || body.mount.is_some()
                    || !body.max_bytes.is_some_and(|limit| limit >= size)
                {
                    return Err(CommandError::new(
                        ErrorCode::NotFound,
                        "the synthetic environment has no such file",
                    ));
                }
                let actual = std::fs::read(&file).map_err(|_| {
                    CommandError::new(ErrorCode::NotFound, "the synthetic source file is absent")
                })?;
                if Hash::of(&actual) != blob {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the synthetic source file changed",
                    ));
                }
                Ok(CommandResult::FsFetch(
                    orch_link_contract::commands::FsFetchResult {
                        hash: blob,
                        size,
                        mtime_ns: at.get().timestamp_nanos_opt().ok_or_else(|| {
                            CommandError::new(
                                ErrorCode::Internal,
                                "synthetic timestamp out of range",
                            )
                        })?,
                    },
                ))
            });
        world
            .machine
            .handler
            .on(CommandKind::FsRecord, move |command| {
                let Command::FsRecord(body) = command else {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "wrong record command",
                    ));
                };
                if body.environment_id != environment || body.directory != "web" {
                    return Err(CommandError::new(
                        ErrorCode::NotFound,
                        "the synthetic environment has no such directory",
                    ));
                }
                Ok(CommandResult::FsRecord(
                    orch_link_contract::source::FsRecordResult {
                        state: orch_link_contract::source::RecordState::Unavailable,
                        tree_ref: None,
                        repository_head: None,
                        manifest_blob: None,
                        files: 0,
                        bytes: 0,
                        expires_at: None,
                        reason: Some("not_ready".into()),
                    },
                ))
            });
        let probe: orch_link_contract::session::MachineFacts = serde_json::from_value(json!({
            "hostname":"synthetic-source-peer","os_version":"synthetic","cpus":1,"memory_bytes":1024,
            "harnesses":[],"adapters":[],"plugins":[],"policy":null,
            "rates":{"up_bps":0,"down_bps":0,"round_trip_ms":0},"clock_offset_ms":0}))?;
        world
            .machine
            .handler
            .on(CommandKind::MachineProbe, move |_| {
                Ok(CommandResult::MachineProbe(probe.clone()))
            });
        if display {
            declare_display(&world, directory.path()).await?;
        }
        let (start, submitted) = Self::authored_turn(&world, thread).await?;
        if display {
            let attached: bool = sqlx::query_scalar(
                "SELECT COALESCE(e.status='running' AND m.online
                AND e.attached_session=COALESCE(m.session_started_at,m.last_seen_at)
                AND e.facts->>'workspace_root'=e.workspace_root,false)
                FROM environments e JOIN machines m ON m.id=e.machine_id WHERE e.id=$1",
            )
            .bind(world.environment)
            .fetch_one(world.pool.pool())
            .await?;
            assert!(
                attached,
                "the baseline must use the actual native attach in this machine session"
            );
            assert_eq!(world.began(CommandKind::EnvAttach), 1);
            assert_eq!(world.began(CommandKind::ExecStart), 1);
        }
        let segment = world.segment_of(thread).await;
        let native_ref: String = world
            .scalar(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment,
            )
            .await;
        let source = NativePrefixSource {
            thread_id: thread,
            segment_id: segment,
            native_session_ref: native_ref,
        };
        native::install(
            &world.machine.handler,
            world.agent,
            world.provider,
            source.clone(),
            start.turn_id,
            typed,
        );
        let created=Self::ask(&world,&source,start.turn_id,"current-create",None,OrchTool::EditDocument,
            json!({"command":"create","path":"docs/source-fence.md","markdown":"Before the genuine current call."})).await?;
        assert!(
            created.error.is_none(),
            "the correctly declared current native call must create its document: {:?}",
            created.error
        );
        let value = created
            .result
            .as_ref()
            .ok_or("current create result missing")?;
        let document = serde_json::from_value(value["id"].clone())?;
        let path = value["path"]
            .as_str()
            .ok_or("created document path missing")?
            .to_owned();
        Ok(Self {
            world,
            thread,
            start,
            input: submitted.input.id,
            source,
            document,
            path,
            created,
            store,
            bytes,
            blob,
            source_file_at: at,
            directory,
        })
    }

    async fn authored_turn(
        world: &common::World,
        thread: Id,
    ) -> Proof<(TurnStart, crate::inputs::Submitted)> {
        let submitted = world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued("Original actual authored input"),
            )
            .await?;
        let start = match tokio::time::timeout(common::PATIENCE, async {
            loop {
                if let Some(start) = world.turn_starts(thread).first().cloned() {
                    break start;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        {
            Ok(start) => start,
            Err(_) => {
                let state = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    Self::baseline_state(world, thread),
                )
                .await
                .ok()
                .and_then(Result::ok);
                let began = [
                    CommandKind::AgentStart,
                    CommandKind::SessionOpen,
                    CommandKind::SessionOpenFenced,
                    CommandKind::TurnStart,
                    CommandKind::TurnStartFenced,
                    CommandKind::EnvAttach,
                    CommandKind::ExecStart,
                ]
                .map(|kind| (kind.as_str(), world.began(kind)));
                return Err(format!("source-effect baseline did not reach its first native turn within the unchanged budget; state/attachment gates={state:?}; command kinds/counts={began:?}").into());
            }
        };
        world.say(thread, common::turn_began(start.turn_id)).await;
        world
            .say(thread, common::message_began(start.turn_id, "m"))
            .await;
        world
            .say(thread, common::message_grew("m", 0, "Working"))
            .await;
        world.wait_turn(start.turn_id, "running").await;
        world.wait_state(thread, "running").await;
        Ok((start, submitted))
    }

    /// Enumerated gates only: no free native error text, paths, SQL or input bodies.
    async fn baseline_state(world: &common::World, thread: Id) -> Proof<Json> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('state',t.state,'reason',t.blocked_reason,'step',t.starting_step,
            'detail_class',CASE WHEN t.blocked_detail IS NULL THEN NULL
              WHEN t.blocked_detail LIKE '%script has no answer for env.attach%' THEN 'env.attach.unsupported'
              WHEN t.blocked_detail LIKE '%script has no answer for exec.start%' THEN 'exec.start.unsupported'
              WHEN t.blocked_detail LIKE '%machine is offline%' THEN 'machine.offline'
              WHEN t.blocked_detail LIKE '%no machine%' THEN 'machine.absent' ELSE 'other' END,
            'environment_status',e.status,'current_attachment',COALESCE(e.attached_session=COALESCE(m.session_started_at,m.last_seen_at),false),
            'directory_descriptor',m.facts#>>'{environment_descriptor,lifecycle}'='attach',
            'native_executor',EXISTS(SELECT 1 FROM jsonb_array_elements(e.facts->'executors') x
                WHERE x->>'kind'='orch-exec' AND x->>'available'='true'))
            FROM threads t LEFT JOIN environments e ON e.id=t.environment_id LEFT JOIN machines m ON m.id=e.machine_id WHERE t.node_id=$1")
            .bind(thread).fetch_one(world.pool.pool()).await?)
    }

    /// A person's actual creation, rather than this agent's intrinsically exportable document.
    async fn shared_download_target(&mut self) -> Proof {
        use crate::document_tree::{self, Actor, Directory};
        use orch_db::queries::access::{PageKey, testing as grants};
        let id = new_id();
        let mut tx = self.world.pool.begin().await?;
        let principal = Principal::load(
            &mut tx,
            Caller::session(self.world.ada),
            self.world.clock.time(),
        )
        .await?;
        let created = document_tree::create_text_in(
            &mut tx,
            Actor::Person(&principal),
            Directory::NodeDocs(self.world.topic),
            id,
            "Admission target",
            Some("admission-target.md"),
            self.world.clock.time(),
        )
        .await?;
        assert!(created.created);
        assert_eq!(created.document.created_by, Some(self.world.ada));
        assert_eq!(created.document.created_by_thread, None);
        for permission in ["doc.view", "doc.edit"] {
            grants::add_entry(
                &mut tx,
                &PageKey::new("document", id, "live"),
                "any",
                permission,
                ("agents", Some(self.thread), None),
                None,
                self.world.clock.now(),
            )
            .await?;
        }
        orch_db::queries::access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
        let written = Self::ask(
            &self.world,
            &self.source,
            self.start.turn_id,
            "prepare-shared-target",
            None,
            OrchTool::EditDocument,
            json!({"command":"rewrite","path":format!("id:{id}"),"seq":created.document.doc_seq,
                "markdown":"Person-owned material shared for this guarded download."}),
        )
        .await?;
        assert_eq!(
            written.error.as_ref().map(|error| error.code.as_str()),
            None,
            "the current agent keeps its explicit Edit grant"
        );
        self.document = id;
        self.path = written
            .result
            .as_ref()
            .and_then(|body| body["path"].as_str())
            .ok_or("the actual shared document path is absent")?
            .into();
        let mut conn = self.world.pool.acquire().await?;
        let resolved = self
            .world
            .engine()
            .resolve(&mut conn, self.thread, &self.path, true)
            .await?
            .map_err(|_| "the independently readable shared target is absent")?;
        assert!(
            !crate::tools::download::may_take_away(&resolved),
            "Read and Edit do not grant this person's document Export"
        );
        Ok(())
    }

    async fn publish(
        world: &common::World,
        source: &NativePrefixSource,
        turn: Id,
        call: &str,
        attempt: Option<Id>,
        tool: OrchTool,
        arguments: Json,
    ) {
        // No prepare/open helper: this is the exact already-admitted recorded native stream,
        // including when its turn is terminal and another continuation now runs.
        world
            .machine
            .publish(
                &Stream::Session {
                    thread_id: source.thread_id,
                    segment_id: source.segment_id,
                },
                &Event::OrchToolCall(OrchToolCall {
                    turn_id: turn,
                    call_ref: call.into(),
                    attempt,
                    tool,
                    arguments,
                }),
            )
            .await;
    }

    async fn answer(
        world: &common::World,
        thread: Id,
        call: &str,
        attempt: Option<Id>,
    ) -> Proof<OrchToolResult> {
        let canonical = crate::ids::tool_result(thread, call);
        let id = attempt.map_or(canonical, |attempt| {
            crate::ids::derived(
                "orch.tool_result.attempt",
                &[canonical.as_bytes(), attempt.as_bytes()],
            )
        });
        let waited = tokio::time::timeout(common::PATIENCE, async {
            loop {
                // The retry may finish first and own the actual canonical delivery.
                // A separate attempt command exists only when that canonical winner
                // carries another attempt. Never accept a mismatched old delivery.
                for delivery in
                    [id, canonical]
                        .into_iter()
                        .take(if id == canonical { 1 } else { 2 })
                {
                    if let Some(body) = world
                        .engine()
                        .inner
                        .commands
                        .retained_body(delivery)
                        .await?
                    {
                        let body: OrchToolResult = serde_json::from_value(body)?;
                        if body.thread_id != thread || body.call_ref != call {
                            return Err(EngineError::ToolCallIdentity);
                        }
                        if attempt.is_none() || body.attempt == attempt {
                            return Ok(body);
                        }
                    }
                }
                tokio::time::sleep(common::config().sweep_every).await;
            }
        })
        .await;
        match waited {
            Ok(answer) => answer.map_err(Into::into),
            Err(_) => {
                let observed=tokio::time::timeout(std::time::Duration::from_secs(2),async {
                    let engine=world.engine();
                    let canonical_body=engine.inner.commands.retained_body(canonical).await.ok().flatten();
                    let requested_body=if id==canonical {canonical_body.clone()} else {engine.inner.commands.retained_body(id).await.ok().flatten()};
                    let matches=|body:&Option<Json>|body.as_ref().is_some_and(|body|body["thread_id"]==json!(thread)
                        && body["call_ref"].as_str()==Some(call) && (attempt.is_none() || body["attempt"]==json!(attempt)));
                    let mut conn=world.pool.acquire().await.ok()?;
                    let source:Option<(bool,bool)>=sqlx::query_as("SELECT t.completed,
                        COALESCE(t.turn_id=h.current_turn_id AND t.segment_id=h.current_segment_id AND t.continuation_id=h.active_continuation_id,false)
                        FROM thread_tool_calls t JOIN threads h ON h.node_id=t.thread_id WHERE t.answer_id=$1")
                        .bind(canonical).fetch_optional(&mut *conn).await.ok()?;
                    // Eight covers display/browser/profile plus their readback stages;
                    // show only kinds/status, never endpoints, arguments, errors or bodies.
                    let rows:Vec<(Id,String,String)>=sqlx::query_as("SELECT c.id,c.kind,c.state FROM thread_tool_commands e
                        JOIN commands c ON c.id=e.command_id WHERE e.answer_id=$1 ORDER BY c.created_at,c.id LIMIT 8")
                        .bind(canonical).fetch_all(&mut *conn).await.ok()?;
                    drop(conn);
                    let mut stages=Vec::new();
                    for (id,kind,stored) in rows {
                        let actual=engine.inner.commands.status(id).await.ok().flatten().map(|status|match status.outcome {
                            Some(Outcome::Succeeded(_))=>"succeeded",Some(Outcome::Failed(_))=>"failed",
                            Some(Outcome::Cancelled)=>"cancelled",Some(Outcome::Expired)=>"expired",None=>"pending"});
                        stages.push(json!({"kind":kind,"stored":stored,"actual":actual}));
                    }
                    Some(json!({"canonical_retained":canonical_body.is_some(),"canonical_matches_attempt":matches(&canonical_body),
                        "requested_retained":requested_body.is_some(),"requested_matches_attempt":matches(&requested_body),
                        "completed_and_current_source":source,"native_stages":stages}))
                }).await.ok().flatten();
                Err(format!("source-effect answer phase {call}: the original native proof budget elapsed; bounded delivery/source/stage facts={observed:?}").into())
            }
        }
    }

    async fn ask(
        world: &common::World,
        source: &NativePrefixSource,
        turn: Id,
        call: &str,
        attempt: Option<Id>,
        tool: OrchTool,
        arguments: Json,
    ) -> Proof<OrchToolResult> {
        Self::publish(world, source, turn, call, attempt, tool, arguments).await;
        Self::answer(world, source.thread_id, call, attempt).await
    }

    async fn end_original(&self) -> Proof {
        let usage = TokenUsage {
            input_tokens: 111,
            total_tokens: 111,
            ..Default::default()
        };
        self.world
            .say_in(
                self.thread,
                self.source.segment_id,
                Event::UsageUpdated(UsageUpdated {
                    turn_id: self.start.turn_id,
                    turn: usage,
                    thread: usage,
                    context_window: Some(100_000),
                    context_used: Some(111),
                    report: None,
                }),
            )
            .await;
        self.world
            .say_in(
                self.thread,
                self.source.segment_id,
                common::turn_ended(self.start.turn_id, TurnOutcome::Completed),
            )
            .await;
        self.world.wait_turn(self.start.turn_id, "completed").await;
        common::eventually(
            "the original native turn and immutable accounting settle",
            || async {
                let mut conn = match self.world.pool.acquire().await {
                    Ok(conn) => conn,
                    Err(_) => return false,
                };
                !threads::turn_end_work_pending(&mut conn, self.thread)
                    .await
                    .unwrap_or(true)
            },
        )
        .await;
        Ok(())
    }

    async fn request_selection(&self, client: &str) -> Proof<ThreadContinuationAccepted> {
        let engine = self.world.engine();
        let mut tx = self.world.pool.begin().await?;
        let current = threads::thread(&mut tx, self.thread)
            .await?
            .ok_or("source thread missing")?;
        let principal = Principal::load(
            &mut tx,
            Caller::session(self.world.ada),
            self.world.clock.time(),
        )
        .await?;
        Ok(engine
            .create_prompt_version_guarded(
                tx,
                principal,
                self.thread,
                self.input,
                ThreadPromptVersionCreateBody {
                    client_id: client.into(),
                    source_continuation_id: current.active_continuation_id,
                    content: vec![ContentPart::Text {
                        text: "New selected prompt, never the old tool's context".into(),
                    }],
                    expected_active_continuation_id: current.active_continuation_id,
                    expected_execution_revision: current.execution_revision,
                    expected_queue_revision: current.queue_revision,
                    active_turn: None,
                },
            )
            .await?)
    }

    async fn selected(&self, accepted: &ThreadContinuationAccepted) -> Proof<NativePrefixSource> {
        let next = self.world.turn_start(self.thread, 1).await;
        self.world
            .answer(
                self.thread,
                &next,
                "selected-native-answer",
                "Only the selected native turn runs",
            )
            .await;
        self.world.wait_turn(next.turn_id, "running").await;
        common::eventually(
            "the selected native answer reaches its completed item",
            || async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND turn_id=$2
                AND native_ref='selected-native-answer' AND status='completed')",
                )
                .bind(self.thread)
                .bind(next.turn_id)
                .fetch_one(self.world.pool.pool())
                .await
                .unwrap_or(false)
            },
        )
        .await;
        common::eventually("the actual continuation job commits and ends", || async {
            sqlx::query_scalar::<_, bool>("SELECT state='succeeded' FROM jobs WHERE id=$1")
                .bind(accepted.operation_id)
                .fetch_one(self.world.pool.pool())
                .await
                .unwrap_or(false)
        })
        .await;
        let (active, segment, native_ref): (Id, Id, String) = sqlx::query_as(
            "SELECT h.active_continuation_id,s.id,s.native_session_ref
            FROM threads h JOIN segments s ON s.id=h.current_segment_id WHERE h.node_id=$1",
        )
        .bind(self.thread)
        .fetch_one(self.world.pool.pool())
        .await?;
        assert_eq!(active, accepted.continuation_id);
        assert_ne!(segment, self.source.segment_id);
        self.world
            .engine()
            .submit_input(
                &Caller::session(self.world.ada),
                self.thread,
                common::queued("Retain the selected queue unchanged"),
            )
            .await?;
        Ok(NativePrefixSource {
            thread_id: self.thread,
            segment_id: segment,
            native_session_ref: native_ref,
        })
    }

    async fn snapshot(&self) -> Proof<Json> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('thread',to_jsonb(h),
          'inputs',(SELECT jsonb_agg(to_jsonb(i) ORDER BY i.id) FROM thread_inputs i WHERE i.thread_id=h.node_id),
          'document',(SELECT to_jsonb(d) FROM documents d WHERE d.id=$2),
          'blocks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM document_blocks b WHERE b.document_id=$2),
          'files',(SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM document_files f WHERE f.document_id=$2),
          'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY v.version) FROM document_versions v WHERE v.document_id=$2),
          'usage',(SELECT jsonb_agg(to_jsonb(u) ORDER BY u.id) FROM usage_events u WHERE u.thread_id=h.node_id))
          FROM threads h WHERE h.node_id=$1")
            .bind(self.thread).bind(self.document).fetch_one(self.world.pool.pool()).await?)
    }

    async fn files_unchanged(&self, before: [usize; 4]) -> Proof {
        assert_eq!(
            tokio::fs::read(self.directory.path().join("out/result.bin"))
                .await?
                .as_slice(),
            self.bytes.as_ref()
        );
        assert_eq!(
            read_all(self.store.get(&self.blob, None).await?.body).await?,
            self.bytes
        );
        assert_eq!(
            [
                CommandKind::FsWrite,
                CommandKind::FsMaterialize,
                CommandKind::FsRestore,
                CommandKind::FsRecord
            ]
            .map(|kind| self.world.began(kind)),
            before
        );
        Ok(())
    }

    fn file_commands(&self) -> [usize; 4] {
        [
            CommandKind::FsWrite,
            CommandKind::FsMaterialize,
            CommandKind::FsRestore,
            CommandKind::FsRecord,
        ]
        .map(|kind| self.world.began(kind))
    }
}

#[tokio::test]
async fn fresh_old_native_calls_refuse_while_completed_answers_replay_and_the_current_call_still_writes()
-> Proof {
    let scene = Scene::new().await?;
    scene.end_original().await?;
    let accepted = scene.request_selection("fresh-tool-source").await?;
    let selected = scene.selected(&accepted).await?;
    let before = scene.snapshot().await?;
    let files = scene.file_commands();
    let seq: i64 = scene
        .world
        .scalar("SELECT doc_seq FROM documents WHERE id=$1", scene.document)
        .await;
    let refused=Scene::ask(&scene.world,&scene.source,scene.start.turn_id,"late-fresh-edit",None,OrchTool::EditDocument,
        json!({"command":"rewrite","path":scene.path,"markdown":"A late source must not write this.","seq":seq})).await?;
    assert_eq!(
        refused.error.as_ref().map(|error| error.code.as_str()),
        Some("changed")
    );
    assert!(
        refused
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("no longer selected or running"))
    );
    assert_eq!(refused.result, None);
    assert_eq!(
        scene.snapshot().await?,
        before,
        "old ingress cannot change document/files, queue, current state or lifetime usage"
    );
    let attempt = new_id();
    let replay=Scene::ask(&scene.world,&scene.source,scene.start.turn_id,"current-create",Some(attempt),OrchTool::EditDocument,
        json!({"command":"create","path":"docs/source-fence.md","markdown":"Before the genuine current call."})).await?;
    let mut expected = scene.created.clone();
    expected.attempt = Some(attempt);
    assert_eq!(
        replay, expected,
        "the genuinely completed canonical answer is unchanged apart from its retry envelope"
    );
    assert_eq!(
        Scene::answer(&scene.world, scene.thread, "current-create", None).await?,
        scene.created
    );
    assert_eq!(scene.snapshot().await?, before);
    scene.files_unchanged(files).await?;
    // Late facts still pass the same real report admission, while the original terminal
    // accounting receipt remains frozen and is not charged to the selected history.
    let observed = TokenUsage {
        input_tokens: 999,
        total_tokens: 999,
        ..Default::default()
    };
    scene
        .world
        .machine
        .publish(
            &Stream::Session {
                thread_id: scene.thread,
                segment_id: scene.source.segment_id,
            },
            &Event::UsageUpdated(UsageUpdated {
                turn_id: scene.start.turn_id,
                turn: observed,
                thread: observed,
                context_window: Some(100_000),
                context_used: Some(999),
                report: None,
            }),
        )
        .await;
    common::eventually(
        "the old native usage observation is accepted without rebilling",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT usage->>'total_tokens'='999' FROM turns WHERE id=$1",
            )
            .bind(scene.start.turn_id)
            .fetch_one(scene.world.pool.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let mut factual = before.clone();
    let previous_seq = before["thread"]["next_seq"]
        .as_i64()
        .ok_or("the actual source sequence is absent")?;
    // The actual turn's totals and its context detail each receive their own sequence.
    // Neither stamp publishes selected thread state or changes the immutable ledger.
    factual["thread"]["next_seq"] = json!(previous_seq + 2);
    assert_eq!(
        scene.snapshot().await?,
        factual,
        "one real old-turn usage observation stamps its totals and context detail only, without changing selected state or lifetime accounting"
    );
    let old = threads::turn(&mut *scene.world.pool.acquire().await?, scene.start.turn_id)
        .await?
        .ok_or("the actual original turn is absent")?;
    assert_eq!(old.usage, Some(serde_json::to_value(observed)?));
    assert_eq!(
        old.usage_detail
            .as_ref()
            .and_then(|detail| detail["context_used"].as_u64()),
        Some(999)
    );
    let ledger: i64 = scene
        .world
        .count(
            "SELECT count(*) FROM usage_events WHERE turn_id=$1",
            scene.start.turn_id,
        )
        .await;
    assert_eq!(ledger, 1);
    let current_turn: Id = scene
        .world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            scene.thread,
        )
        .await;
    let written=Scene::ask(&scene.world,&selected,current_turn,"valid-selected-edit",None,OrchTool::EditDocument,
        json!({"command":"rewrite","path":scene.path,"markdown":"A legitimate current call writes.","seq":seq})).await?;
    assert!(
        written.error.is_none(),
        "the source fence cannot disable legitimate selected work: {:?}",
        written.error
    );
    assert_ne!(scene.snapshot().await?["document"], before["document"]);
    let words: String = scene
        .world
        .scalar(
            "SELECT string_agg(text,E'\\n' ORDER BY position,id) FROM document_blocks
        WHERE document_id=$1 AND removed_at IS NULL",
            scene.document,
        )
        .await;
    assert_eq!(words, "A legitimate current call writes.");
    assert_eq!(
        scene.world.turn_starts(scene.thread).len(),
        2,
        "only the two explicitly authored inputs start a turn"
    );
    scene.world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_retained_native_answer_recovers_its_unfinished_receipt_after_the_answer_owner_is_lost()
-> Proof {
    let scene = Scene::new().await?;
    let call = "answer-before-receipt-checkpoint";
    let canonical = crate::ids::tool_result(scene.thread, call);
    let (retained, _release) = gate(scene.thread, &canonical.to_string(), Point::AnswerRetained);
    expect_task(scene.thread, call);
    let seq: i64 = scene
        .world
        .scalar("SELECT doc_seq FROM documents WHERE id=$1", scene.document)
        .await;
    Scene::publish(&scene.world,&scene.source,scene.start.turn_id,call,None,OrchTool::EditDocument,
        json!({"command":"rewrite","path":scene.path,"markdown":"The one completed write before receipt recovery.","seq":seq})).await;
    reached(
        "the real canonical answer retained before its receipt checkpoint",
        retained,
    )
    .await?;
    let answer = Scene::answer(&scene.world, scene.thread, call, None).await?;
    assert!(answer.error.is_none());
    assert!(
        !scene
            .world
            .scalar::<bool>(
                "SELECT completed FROM thread_tool_calls WHERE answer_id=$1",
                canonical
            )
            .await
    );
    let (machine, command) = scene
        .world
        .commands
        .issued_commands("orch.tool_result")
        .into_iter()
        .find(|(_, command)| command.id == canonical)
        .ok_or("the exact issued canonical answer is absent")?;
    assert_eq!(command.body, serde_json::to_value(&answer)?);
    assert_eq!(command.key, SerialKey::Thread(scene.thread).wire());
    interrupt_task(scene.thread, call).await?;
    assert!(
        !scene
            .world
            .scalar::<bool>(
                "SELECT completed FROM thread_tool_calls WHERE answer_id=$1",
                canonical
            )
            .await
    );
    // The common controller supplier retains commands in its owned journal, not SQL.
    // Reconstruct only the production dispatcher's storage half from that actual issued
    // command, preserving every field; no synthetic answer or successful effect is invented.
    let mut conn = scene.world.pool.acquire().await?;
    assert!(
        link::insert_command(
            &mut conn,
            &link::NewCommand {
                id: command.id,
                machine_id: machine,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.key.as_deref(),
                deadline_at: command.deadline.get()
            },
            scene.world.clock.now()
        )
        .await?
    );
    drop(conn);
    let before = scene.snapshot().await?;
    common::eventually(
        "normal recovery rejoins the retained canonical body without another tool execution",
        || async {
            scene
                .world
                .scalar::<bool>(
                    "SELECT completed FROM thread_tool_calls WHERE answer_id=$1",
                    canonical,
                )
                .await
        },
    )
    .await;
    assert_eq!(
        scene.snapshot().await?,
        before,
        "receipt recovery changes no document, selected state, queue or lifetime ledger"
    );
    let replay=Scene::ask(&scene.world,&scene.source,scene.start.turn_id,call,None,OrchTool::EditDocument,
        json!({"command":"rewrite","path":scene.path,"markdown":"The one completed write before receipt recovery.","seq":seq})).await?;
    assert_eq!(replay, answer);
    assert_eq!(scene.snapshot().await?, before);
    let saved = link::command(&mut *scene.world.pool.acquire().await?, canonical)
        .await?
        .ok_or("the canonical queue body was lost")?;
    assert_eq!(
        (saved.body, saved.serial_key, saved.deadline_at),
        (command.body, command.key, command.deadline.get())
    );
    assert_eq!(
        scene
            .world
            .commands
            .issued_commands("orch.tool_result")
            .into_iter()
            .filter(|(_, command)| command.id == canonical)
            .count(),
        1
    );
    scene.world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn selection_after_complete_external_preparation_refuses_the_old_write_before_its_document_transaction()
-> Proof {
    let scene = Scene::new().await?;
    let (prepared, release) = gate(scene.thread, "prepared-old-attachment", Point::Prepared);
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "prepared-old-attachment",
        None,
        OrchTool::EditDocument,
        json!({"command":"attach","path":scene.path,"file":"out/result.bin","name":"result.bin"}),
    )
    .await;
    reached("complete external file preparation", prepared).await?;
    assert_eq!(
        scene.world.began(CommandKind::FsFetch),
        1,
        "the actual native bounded file read has completed before this phase"
    );
    let pin: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(scene.blob.as_bytes().as_slice())
        .fetch_one(scene.world.pool.pool())
        .await?;
    assert_eq!(
        pin, 0,
        "preparation cannot attach or pin the bytes as document content"
    );
    scene.end_original().await?;
    let accepted = scene.request_selection("prepared-effect-race").await?;
    scene.selected(&accepted).await?;
    let before = scene.snapshot().await?;
    let files = scene.file_commands();
    release
        .send(())
        .map_err(|_| "the prepared tool task disappeared")?;
    let answer = Scene::answer(&scene.world, scene.thread, "prepared-old-attachment", None).await?;
    assert_eq!(
        answer.error.as_ref().map(|error| error.code.as_str()),
        Some("changed")
    );
    assert_eq!(answer.result, None);
    assert_eq!(
        scene.snapshot().await?,
        before,
        "selection wins before the write: no document/history/file/queue/state change can commit"
    );
    let files_count: i64 = scene
        .world
        .count(
            "SELECT count(*) FROM document_files WHERE document_id=$1",
            scene.document,
        )
        .await;
    assert_eq!(files_count, 0);
    scene.files_unchanged(files).await?;
    scene.world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_read_answer_cannot_cross_the_selection_boundary_or_return_another_suffix() -> Proof {
    let scene = Scene::new().await?;
    let (read, release) = gate(scene.thread, "held-old-read", Point::Read);
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "held-old-read",
        None,
        OrchTool::ReadShared,
        json!({"path":format!("id:{}",scene.thread)}),
    )
    .await;
    reached("read result before final source check", read).await?;
    scene.end_original().await?;
    let accepted = scene.request_selection("read-effect-race").await?;
    scene.selected(&accepted).await?;
    let before = scene.snapshot().await?;
    release
        .send(())
        .map_err(|_| "the held native read disappeared")?;
    let answer = Scene::answer(&scene.world, scene.thread, "held-old-read", None).await?;
    assert_eq!(
        answer.error.as_ref().map(|error| error.code.as_str()),
        Some("changed")
    );
    assert_eq!(
        answer.result, None,
        "an old call returns neither another suffix nor a mixture of history cuts"
    );
    assert_eq!(scene.snapshot().await?, before);
    scene.world.shutdown().await;
    Ok(())
}

async fn download_peer(
    scene: &Scene,
    hold: bool,
) -> Proof<(Arc<UnjoinedWrite>, std::path::PathBuf, Id)> {
    use orch_db::queries::access::{PageKey, testing as access};
    use orch_link_contract::commands::ExpectedHash;
    use std::io::Write;
    let native_bytes = Arc::new(Mutex::new(None::<(Hash, Bytes)>));
    let native = Arc::new(UnjoinedWrite {
        inner: scene.world.machine.handler.clone(),
        store: scene.store.clone(),
        bytes: native_bytes.clone(),
        hold,
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    scene.world.machine.link.serve(native.clone());
    let export = access::add_entry(
        &mut *scene.world.pool.acquire().await?,
        &PageKey::new("document", scene.document, "live"),
        "any",
        "doc.export",
        ("agents", Some(scene.thread), None),
        None,
        scene.world.clock.now(),
    )
    .await?;
    scene
        .world
        .engine()
        .attach_parcels(Arc::new(crate::parcels::WorkspaceParcels::new(
            &scene.world.engine(),
        )));
    let expected = native_bytes.clone();
    let environment = scene.world.environment;
    let at = scene.world.clock.time();
    let destination = scene.directory.path().join("copied-doc.md");
    let written = destination.clone();
    scene
        .world
        .machine
        .handler
        .on(CommandKind::FsWrite, move |command| {
            let Command::FsWrite(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native write",
                ));
            };
            if body.environment_id != environment
                || body.path != "web/copied-doc.md"
                || !matches!(body.expected_hash.as_ref(), Some(ExpectedHash::Absent))
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the synthetic write has another target or overwrite rule",
                ));
            }
            let (hash, bytes) = expected
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::Internal,
                        "the exact queued download bytes were not supplied",
                    )
                })?;
            if hash != body.blob || Hash::of(&bytes) != hash {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the queued blob identity changed",
                ));
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&written)
                .map_err(|_| {
                    CommandError::new(ErrorCode::Conflict, "the synthetic target already exists")
                })?;
            file.write_all(&bytes).map_err(|_| {
                CommandError::new(ErrorCode::Internal, "the synthetic native write failed")
            })?;
            Ok(CommandResult::FsWrite(
                orch_api_contract::workspace::FileStat {
                    entry: orch_api_contract::workspace::DirEntry {
                        name: "copied-doc.md".into(),
                        path: body.path.clone(),
                        kind: orch_api_contract::workspace::DirEntryKind::File,
                        size: i64::try_from(bytes.len()).map_err(|_| {
                            CommandError::new(
                                ErrorCode::Internal,
                                "synthetic file exceeds integer range",
                            )
                        })?,
                        mtime: at,
                        mode: 0o600,
                        link_target: None,
                        cached: false,
                    },
                    hash: Some(hash),
                    media_type: "text/markdown".into(),
                    is_text: true,
                },
            ))
        });
    let inspect = destination.clone();
    let source = scene.directory.path().join("out/result.bin");
    let source_at = scene.source_file_at;
    scene
        .world
        .machine
        .handler
        .on(CommandKind::FsStat, move |command| {
            let Command::FsStat(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native readback",
                ));
            };
            if body.environment_id != environment || body.mount.is_some() {
                return Err(CommandError::new(
                    ErrorCode::NotFound,
                    "another environment or mount",
                ));
            }
            let is_source = body.path == "web/out/result.bin";
            let is_file = body.path == "web/copied-doc.md" || is_source;
            if !is_file && !matches!(body.path.as_str(), "" | "web") {
                return Err(CommandError::new(ErrorCode::NotFound, "no synthetic entry"));
            }
            let bytes = if is_file {
                std::fs::read(if is_source { &source } else { &inspect })
                    .map_err(|_| CommandError::new(ErrorCode::NotFound, "no synthetic file"))?
            } else {
                Vec::new()
            };
            Ok(CommandResult::FsStat(
                orch_api_contract::workspace::FileStat {
                    entry: orch_api_contract::workspace::DirEntry {
                        name: if is_source {
                            "result.bin"
                        } else if is_file {
                            "copied-doc.md"
                        } else {
                            "web"
                        }
                        .into(),
                        path: body.path.clone(),
                        kind: if is_file {
                            orch_api_contract::workspace::DirEntryKind::File
                        } else {
                            orch_api_contract::workspace::DirEntryKind::Dir
                        },
                        size: i64::try_from(bytes.len()).map_err(|_| {
                            CommandError::new(ErrorCode::Internal, "synthetic size")
                        })?,
                        mtime: if is_source { source_at } else { at },
                        mode: 0o600,
                        link_target: None,
                        cached: false,
                    },
                    hash: is_file.then(|| Hash::of(&bytes)),
                    media_type: if is_source {
                        "application/octet-stream"
                    } else {
                        "text/markdown"
                    }
                    .into(),
                    is_text: !is_source,
                },
            ))
        });
    Ok((native, destination, export))
}

async fn admitted_download(cancel_live: bool) -> Proof {
    let scene = Scene::new().await?;
    let (native, destination, _export) = download_peer(&scene, cancel_live).await?;
    let environment = scene.world.environment;
    let call = "durably-admitted-download";
    let canonical = crate::ids::tool_result(scene.thread, call);
    let (admitted, release) = gate(scene.thread, &canonical.to_string(), Point::Native);
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        None,
        OrchTool::DownloadShared,
        json!({"path":format!("id:{}",scene.document),"save_to":"copied-doc.md"}),
    )
    .await;
    reached("durable native admission before issue", admitted).await?;
    let (id,source_turn,source_segment,source_continuation,answer,deadline,created,body):
        (Id,Id,Id,Id,Id,chrono::DateTime<chrono::Utc>,chrono::DateTime<chrono::Utc>,Json)=sqlx::query_as(
        "SELECT c.id,e.turn_id,e.segment_id,e.continuation_id,e.answer_id,c.deadline_at,c.created_at,c.body
         FROM thread_tool_commands e JOIN commands c ON c.id=e.command_id WHERE e.thread_id=$1 AND e.answer_id=$2")
        .bind(scene.thread).bind(canonical).fetch_one(scene.world.pool.pool()).await?;
    let owner: Id = scene
        .world
        .scalar(
            "SELECT continuation_id FROM segments WHERE id=$1",
            scene.source.segment_id,
        )
        .await;
    assert_eq!(
        (source_turn, source_segment, source_continuation, answer),
        (
            scene.start.turn_id,
            scene.source.segment_id,
            owner,
            canonical
        )
    );
    assert_eq!(
        deadline - created,
        chrono::Duration::seconds(i64::from(CommandKind::FsWrite.deadline_secs()))
    );
    assert!(
        scene
            .world
            .engine()
            .inner
            .commands
            .status(id)
            .await?
            .is_none(),
        "the queue row and source hold must commit before handing work to the native command supplier"
    );
    assert_eq!(scene.world.machine.handler.runs_of(id), 0);
    let hash: Hash = serde_json::from_value(body["blob"].clone())?;
    let bytes = read_all(scene.store.get(&hash, None).await?.body).await?;
    assert_eq!(Hash::of(&bytes), hash);
    assert!(
        native
            .bytes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none(),
        "the native byte source must not be preloaded before admission/GC is tested"
    );
    let pins_before: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(scene.world.pool.pool())
        .await?;
    assert_eq!(
        pins_before, 1,
        "the admitted command is the rendered blob's first owner"
    );
    let mut conn = scene.world.pool.acquire().await?;
    let cutoff = scene.world.clock.now() + chrono::Duration::seconds(1);
    assert!(
        !orch_db::queries::storage::unreferenced(&mut conn, cutoff, common::config().sweep_batch)
            .await?
            .iter()
            .any(|(candidate, _)| candidate == &hash)
    );
    assert!(
        !orch_db::queries::storage::forget_blob(&mut conn, &hash, cutoff).await?,
        "zero-grace collection cannot forget an admitted command's still-required input"
    );
    drop(conn);
    let independent = if cancel_live {
        use orch_db::queries::{documents, storage};
        let created=Scene::ask(&scene.world,&scene.source,scene.start.turn_id,"independent-blob-owner",None,OrchTool::EditDocument,
            json!({"command":"create","path":"docs/independent-owner.md","markdown":"An independent attachment owner."})).await?;
        assert!(created.error.is_none());
        let document: Id = serde_json::from_value(
            created.result.ok_or("independent document missing")?["id"].clone(),
        )?;
        let file = new_id();
        let snapshot = new_id();
        let mut tx = scene.world.pool.begin().await?;
        // These are the ordinary document/snapshot storage suppliers, not a second tool hold.
        documents::add_file(
            &mut tx,
            &documents::NewFile {
                id: file,
                document,
                blob: hash,
                name: "independent.md",
                media_type: "text/markdown",
                size: i64::try_from(bytes.len())?,
                is_text: true,
                added_by: Some(scene.world.ada),
                added_by_thread: None,
                at: scene.world.clock.now(),
            },
        )
        .await?;
        assert_eq!(storage::pin_blobs(&mut tx, &[hash]).await?, 1);
        let generation: i32 = sqlx::query_scalar("SELECT generation FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(&mut *tx)
            .await?;
        storage::begin_snapshot(
            &mut tx,
            &storage::NewSnapshot {
                id: snapshot,
                thread_id: Some(scene.thread),
                environment_id: environment,
                env_generation: generation,
                label: "Independent retained snapshot",
                selection: &json!({"include":[],"exclude":[]}),
                created_by: Some(scene.world.ada),
                at: scene.world.clock.now(),
            },
        )
        .await?;
        assert_eq!(storage::pin_snapshot(&mut tx, snapshot, &[hash]).await?, 1);
        assert!(
            storage::end_snapshot(
                &mut tx,
                snapshot,
                &storage::SnapshotEnd {
                    state: "complete",
                    file_count: 1,
                    total_bytes: i64::try_from(bytes.len())?,
                    missing_count: 0,
                    manifest_blob: None,
                    at: scene.world.clock.now()
                }
            )
            .await?
        );
        tx.commit().await?;
        Some((document, file, snapshot))
    } else {
        None
    };
    scene.end_original().await?;
    common::eventually("the genuine original end-file observation finishes before the native write is issued",||async {
        sqlx::query_scalar::<_,bool>("SELECT NOT files_end_due AND NOT files_work_pending AND end_work IS NULL FROM turns WHERE id=$1")
            .bind(scene.start.turn_id).fetch_one(scene.world.pool.pool()).await.unwrap_or(false)
    }).await;
    let accepted = scene
        .request_selection("queued-native-effect-fence")
        .await?;
    common::eventually("the real continuation job is waiting at quiescence for the admitted native effect",||async {
        sqlx::query_scalar::<_,bool>("SELECT state='running' AND progress->>'phase'='quiesce'
            AND progress->>'message'='Waiting for the original native turn, files and accounting to finish.' FROM jobs WHERE id=$1")
            .bind(accepted.operation_id).fetch_one(scene.world.pool.pool()).await.unwrap_or(false)
    }).await;
    assert_eq!(
        scene.world.began(CommandKind::SessionPrefixOpen),
        0,
        "quiescence cannot miss an admitted effect before dispatch"
    );
    let current: Id = scene
        .world
        .scalar(
            "SELECT active_continuation_id FROM threads WHERE node_id=$1",
            scene.thread,
        )
        .await;
    assert_eq!(current, owner);
    assert!(!destination.exists());
    release
        .send(())
        .map_err(|_| "the native admission task disappeared")?;
    let result = if cancel_live {
        common::eventually(
            "the actual native write starts and retains its environment key",
            || async { native.entered.available_permits() == 1 },
        )
        .await;
        assert!(scene.world.engine().inner.commands.cancel(id).await?);
        assert_eq!(
            scene
                .world
                .engine()
                .inner
                .commands
                .status(id)
                .await?
                .and_then(|info| info.outcome),
            Some(Outcome::Cancelled)
        );
        let result = Scene::answer(&scene.world, scene.thread, call, None).await?;
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("unavailable")
        );
        // Mirror the actual controller cancellation in this commands-double's DB row,
        // solely to exercise the production GC predicate, not to invent native completion.
        orch_db::queries::link::finish_command(
            &mut *scene.world.pool.acquire().await?,
            id,
            &orch_db::queries::link::Ending::Cancelled,
            scene.world.clock.now(),
        )
        .await?;
        common::eventually("cancellation retains the hold and admits its same-key read-only completion fence",||async {
            sqlx::query_scalar::<_,bool>("SELECT fence_id IS NOT NULL AND NOT settled FROM thread_tool_commands WHERE command_id=$1")
                .bind(id).fetch_one(scene.world.pool.pool()).await.unwrap_or(false)
        }).await;
        let fence: Id = scene
            .world
            .scalar(
                "SELECT fence_id FROM thread_tool_commands WHERE command_id=$1",
                id,
            )
            .await;
        common::eventually(
            "the native peer accepts the completion fence behind the still-running old write",
            || async {
                scene
                    .world
                    .engine()
                    .inner
                    .commands
                    .status(fence)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|info| info.stage == crate::commands::Stage::Accepted)
            },
        )
        .await;
        let saved_fence =
            orch_db::queries::link::command(&mut *scene.world.pool.acquire().await?, fence)
                .await?
                .ok_or("fence missing")?;
        assert_eq!(saved_fence.kind, CommandKind::MachineProbe.as_str());
        assert_eq!(
            saved_fence.serial_key(),
            Some(environment.to_string().as_str())
        );
        assert_eq!(
            saved_fence.deadline_at - saved_fence.created_at,
            chrono::Duration::seconds(i64::from(CommandKind::MachineProbe.deadline_secs()))
        );
        let entry: orch_link::journal::Entry = serde_json::from_slice(
            &tokio::fs::read(
                scene
                    .world
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{id}.json")),
            )
            .await?,
        )?;
        assert!(matches!(entry.step, orch_link::journal::Step::Started));
        assert_eq!(
            scene.world.machine.handler.runs_of(fence),
            0,
            "the read cannot pass a native write that still owns its environment key"
        );
        assert!(!destination.exists());
        assert_eq!(scene.world.began(CommandKind::SessionPrefixOpen), 0);
        orch_db::queries::link::purge_finished(
            &mut *scene.world.pool.acquire().await?,
            scene.world.clock.now() + chrono::Duration::days(1),
            common::config().sweep_batch,
        )
        .await?;
        assert!(
            orch_db::queries::link::command(&mut *scene.world.pool.acquire().await?, id)
                .await?
                .is_some(),
            "GC cannot erase an unresolved old effect merely because its controller reports cancellation"
        );
        assert!(
            !scene
                .world
                .scalar::<bool>(
                    "SELECT settled FROM thread_tool_commands WHERE command_id=$1",
                    id
                )
                .await
        );
        let mut conn = scene.world.pool.acquire().await?;
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT pin_count FROM blobs WHERE hash=$1")
                .bind(hash.as_bytes().as_slice())
                .fetch_one(&mut *conn)
                .await?,
            3
        );
        assert!(!orch_db::queries::storage::forget_blob(&mut conn, &hash, cutoff).await?);
        assert_eq!(
            read_all(scene.store.get(&hash, None).await?.body).await?,
            bytes,
            "normal GC and controller cancellation must leave the actual native input available"
        );
        drop(conn);
        native.release.add_permits(1);
        common::eventually("the same-key native read confirms that the cancelled controller's actual write has drained",||async {
            scene.world.engine().inner.commands.status(fence).await.ok().flatten()
                .is_some_and(|info|matches!(info.outcome,Some(Outcome::Succeeded(_))))
        }).await;
        assert_eq!(
            scene
                .world
                .engine()
                .inner
                .commands
                .status(id)
                .await?
                .and_then(|info| info.outcome),
            Some(Outcome::Cancelled),
            "the proof must not replace the original controller outcome with invented success"
        );
        result
    } else {
        let result = Scene::answer(&scene.world, scene.thread, call, None).await?;
        assert!(
            result.error.is_none(),
            "a durable admitted effect keeps its exact outcome: {:?}",
            result.error
        );
        assert_eq!(
            result
                .result
                .as_ref()
                .and_then(|value| value["status"].as_str()),
            Some("ready")
        );
        result
    };
    assert_eq!(result.thread_id, scene.thread);
    scene.selected(&accepted).await?;
    assert_eq!(scene.world.machine.handler.runs_of(id), 1);
    assert_eq!(
        tokio::fs::read(&destination).await?.as_slice(),
        bytes.as_ref()
    );
    let saved = orch_db::queries::link::command(&mut *scene.world.pool.acquire().await?, id)
        .await?
        .ok_or("queued command disappeared")?;
    assert_eq!(
        (saved.deadline_at, saved.body),
        (deadline, body),
        "selection cannot replace the original native budget or input"
    );
    let pending: i64 = scene
        .world
        .count(
            "SELECT count(*) FROM thread_tool_commands WHERE thread_id=$1 AND NOT settled",
            scene.thread,
        )
        .await;
    assert_eq!(
        pending, 0,
        "only proven native completion releases this admission"
    );
    reap(&scene.world.engine(), Some(scene.thread)).await?;
    reap(&scene.world.engine(), Some(scene.thread)).await?;
    let mut conn = scene.world.pool.acquire().await?;
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(
        pins,
        if independent.is_some() { 2 } else { 0 },
        "settlement releases exactly its own input pin, once"
    );
    let refs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_tool_command_blobs WHERE command_id=$1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    assert_eq!(refs, 0);
    if let Some((document, file, snapshot)) = independent {
        assert_eq!(
            orch_db::queries::documents::file(&mut conn, document, file)
                .await?
                .ok_or("independent attachment lost")?
                .blob,
            hash
        );
        assert_eq!(
            orch_db::queries::storage::snapshot_pins(&mut conn, snapshot).await?,
            vec![hash]
        );
    } else {
        let candidates = orch_db::queries::storage::unreferenced(
            &mut conn,
            scene.world.clock.now() + chrono::Duration::seconds(1),
            common::config().sweep_batch,
        )
        .await?;
        assert!(
            candidates.iter().any(|(candidate, _)| candidate == &hash),
            "only proven completion returns the blob to ordinary grace/collection"
        );
    }
    drop(conn);
    assert_eq!(scene.world.began(CommandKind::SessionPrefixOpen), 1);
    if !cancel_live {
        let changed = b"Explicitly changed current workspace bytes\0\xff";
        tokio::fs::write(&destination, changed).await?;
        let selected_turn: Id = scene
            .world
            .scalar(
                "SELECT current_turn_id FROM threads WHERE node_id=$1",
                scene.thread,
            )
            .await;
        let selected_segment: Id = scene
            .world
            .scalar(
                "SELECT current_segment_id FROM threads WHERE node_id=$1",
                scene.thread,
            )
            .await;
        let native_ref: String = scene
            .world
            .scalar(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                selected_segment,
            )
            .await;
        let current = NativePrefixSource {
            thread_id: scene.thread,
            segment_id: selected_segment,
            native_session_ref: native_ref,
        };
        let refused = Scene::ask(
            &scene.world,
            &current,
            selected_turn,
            "new-source-identical-download",
            None,
            OrchTool::DownloadShared,
            json!({"path":format!("id:{}",scene.document),"save_to":"copied-doc.md"}),
        )
        .await?;
        assert_eq!(
            refused.error.as_ref().map(|error| error.code.as_str()),
            Some("exists"),
            "a new source must inspect the actual destination instead of inheriting the old Ready result"
        );
        assert_eq!(tokio::fs::read(&destination).await?, changed);
        assert_eq!(
            scene.world.began(CommandKind::FsWrite),
            2,
            "the two logical calls have distinct native write identities"
        );
        let b_answer = crate::ids::tool_result(scene.thread, "new-source-identical-download");
        let b_command: Id = scene
            .world
            .scalar(
                "SELECT command_id FROM thread_tool_commands WHERE answer_id=$1",
                b_answer,
            )
            .await;
        assert_ne!(b_command, id);
        assert_eq!(scene.world.machine.handler.runs_of(b_command), 1);
        // A separate deliberate current call after actual removal gets its own write, not
        // the first call's old Ready answer or the preceding current call's Conflict.
        assert_eq!(tokio::fs::read(&destination).await?, changed);
        tokio::fs::remove_file(&destination).await?;
        let copied = Scene::ask(
            &scene.world,
            &current,
            selected_turn,
            "new-source-removed-download",
            None,
            OrchTool::DownloadShared,
            json!({"path":format!("id:{}",scene.document),"save_to":"copied-doc.md"}),
        )
        .await?;
        assert!(copied.error.is_none());
        assert_eq!(
            copied
                .result
                .as_ref()
                .and_then(|value| value["status"].as_str()),
            Some("ready")
        );
        assert_eq!(
            tokio::fs::read(&destination).await?.as_slice(),
            bytes.as_ref()
        );
        let c_answer = crate::ids::tool_result(scene.thread, "new-source-removed-download");
        let c_command: Id = scene
            .world
            .scalar(
                "SELECT command_id FROM thread_tool_commands WHERE answer_id=$1",
                c_answer,
            )
            .await;
        assert_ne!(c_command, id);
        assert_ne!(c_command, b_command);
        assert_eq!(scene.world.machine.handler.runs_of(c_command), 1);
        assert_eq!(scene.world.began(CommandKind::FsWrite), 3);
        assert_eq!(
            Scene::answer(&scene.world, scene.thread, call, None).await?,
            result,
            "the old call's exact completed answer remains its own no-new-work replay"
        );
    }
    scene.world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_durable_native_admission_holds_selection_through_issue_and_its_actual_outcome() -> Proof
{
    admitted_download(false).await
}

#[tokio::test]
async fn controller_cancellation_cannot_release_a_live_native_write_or_its_gc_pin_before_same_key_readback()
-> Proof {
    admitted_download(true).await
}

async fn declare_display(world: &common::World, owned: &std::path::Path) -> Proof {
    use orch_driver_contract::descriptor::{DriverDescriptor, EnvironmentDataStorage};
    let root = owned.join("connected-workspace");
    tokio::fs::create_dir_all(root.join("web/out")).await?;
    tokio::fs::hard_link(
        owned.join("out/result.bin"),
        root.join("web/out/result.bin"),
    )
    .await?;
    let home = owned.join("connected-home");
    tokio::fs::create_dir_all(&home).await?;
    let workspace = root
        .to_str()
        .ok_or("the owned connected workspace is not UTF-8")?
        .to_owned();
    let vnc = json!({"servers":[{"name":"synthetic-vnc","path":"/synthetic/vnc","version":"1"}],"window_manager":null});
    let browser = json!({"programs":[{"name":"chromium","path":"/synthetic/chromium","version":"1"}],
        "window":true,"virtual_window":true,"playwright_cli":null,"sandbox":true});
    let report: orch_link_contract::events::EnvironmentFacts = serde_json::from_value(json!({
        "os":"linux","arch":"x86_64","shell":{"name":"sh","path":"/bin/sh"},"user":"synthetic-agent",
        "home":home.to_str().ok_or("the owned connected home is not UTF-8")?,"workspace_root":workspace,
        "path_style":"posix","case_sensitive":true,"executors":[{"kind":"orch-exec","version":"0.1.0",
            "available":true,"reason":null,"sandbox_modes":["read_only","workspace","open"],"tool_sets":["plain"]}],
        "mcp_found":[],"index":"live","mounts":[],"private_views":false,"suspend":false,"git":false,"generation":1,
        "vnc":vnc,"browser":browser}))?;
    let environment = world.environment;
    let expected = workspace.clone();
    let directory = root.clone();
    // Install both native producers before publishing placement; keep_connected may
    // immediately attach a directory in the already connected machine's current session.
    world
        .machine
        .handler
        .on(CommandKind::EnvAttach, move |command| {
            let Command::EnvAttach(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native attachment",
                ));
            };
            if body.environment_id != environment
                || body.workspace_root.as_deref() != Some(expected.as_str())
                || body.data_sources.is_some()
                || !std::fs::metadata(&directory).is_ok_and(|metadata| metadata.is_dir())
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the native attach does not name the owned existing directory",
                ));
            }
            Ok(CommandResult::EnvAttach(report.clone()))
        });
    world
        .machine
        .handler
        .on(CommandKind::ExecStart, move |command| {
            let Command::ExecStart(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native executor",
                ));
            };
            if body.environment_id != environment || body.executor != "orch-exec" {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "the declared executor is absent",
                ));
            }
            Ok(CommandResult::ExecStart(
                orch_link_contract::commands::ExecStartResult {
                    service: "exec:orch-exec".into(),
                    version: "0.1.0".into(),
                    tool_sets: vec!["plain".into()],
                },
            ))
        });
    let descriptor = DriverDescriptor::registered_directories(
        "source-fixture",
        EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    );
    let mut tx = world.pool.begin().await?;
    let before: Json = sqlx::query_scalar("SELECT facts->'policy' FROM machines WHERE id=$1")
        .bind(world.machine_id)
        .fetch_one(&mut *tx)
        .await?;
    // This fixture has one real connected machine and no physical unit runtime.
    // Report only the native feature exercised here through its ordinary physical supplier.
    sqlx::query("UPDATE environments SET placement='connected',template_id=NULL,vnc=true,browser=true,workspace_root=$2,
        facts=facts || jsonb_build_object('vnc',$3::jsonb,'browser',$4::jsonb) WHERE id=$1")
        .bind(world.environment).bind(&workspace).bind(&vnc).bind(&browser)
        .execute(&mut *tx).await?;
    // Add the two physical facilities and offer only the executor this environment
    // actually reports. Fresh installed defaults would erase the admitted harness.
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts || jsonb_build_object('environment_descriptor',$2::jsonb),'{policy}',
        (facts->'policy') || jsonb_build_object('environment.vnc',true,'environment.browser',true,
            'exec.offers',jsonb_build_array('orch-exec'),'environment.roots',jsonb_build_array($3::text))) WHERE id=$1")
        .bind(world.machine_id).bind(serde_json::to_value(descriptor)?).bind(&workspace).execute(&mut *tx).await?;
    let machine =
        orch_db::queries::resources::thread_engine::machine_place(&mut tx, world.machine_id)
            .await?
            .ok_or("the actual physical fixture machine is absent")?;
    let mut expected = before;
    expected["environment.vnc"] = json!(true);
    expected["environment.browser"] = json!(true);
    expected["exec.offers"] = json!(["orch-exec"]);
    expected["environment.roots"] = json!([workspace]);
    assert_eq!(
        machine.facts["policy"], expected,
        "the display fixture preserves every unrelated physical policy byte"
    );
    assert!(
        crate::binding::machine_has_harness(&machine, "fake"),
        "the first native turn still needs its installed, locally allowed harness"
    );
    assert_eq!(
        crate::binding::offers_of(&mut tx, world.environment).await?,
        vec!["orch-exec".to_owned()]
    );
    let supported:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM environments e,
        jsonb_array_elements(e.facts->'executors') x WHERE e.id=$1 AND x->>'kind'='orch-exec' AND x->>'available'='true')")
        .bind(world.environment).fetch_one(&mut *tx).await?;
    assert!(
        supported,
        "the offered executor must have actual current native facts"
    );
    tx.commit().await?;
    let screens = Arc::new(Mutex::new(HashMap::<Id, String>::new()));
    let opened = screens.clone();
    world
        .machine
        .handler
        .on(CommandKind::DisplayOpen, move |command| {
            let Command::DisplayOpen(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong display command",
                ));
            };
            if !matches!(body.geometry.as_str(), "1024x768" | "1280x800") {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "another reserved geometry",
                ));
            }
            opened
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(body.display_id, "127.0.0.1:5991".into());
            Ok(CommandResult::DisplayOpen(
                orch_link_contract::commands::DisplayOpenResult {
                    service: format!("display:{}", body.display_id),
                    endpoint: "127.0.0.1:5991".into(),
                    x_display: ":91".into(),
                },
            ))
        });
    let looked: orch_link_contract::commands::ScreenLookResult = serde_json::from_value(
        json!({"browser":browser,
        "vnc":{"servers":[{"name":"synthetic-vnc","path":"/synthetic/vnc","version":"1"}],"window_manager":null},
        "profiles":[],"browsers":[],"displays":[]}),
    )?;
    world.machine.handler.on(CommandKind::ScreenLook, move |_| {
        let mut found = looked.clone();
        found.displays = screens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(
                |(id, endpoint)| orch_link_contract::browser::ScreenLookDisplay {
                    display_id: *id,
                    endpoint: endpoint.clone(),
                },
            )
            .collect();
        Ok(CommandResult::ScreenLook(found))
    });
    world
        .machine
        .handler
        .on(CommandKind::BrowserOpen, move |command| {
            let Command::BrowserOpen(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong browser command",
                ));
            };
            if body.profile != "source-window" || body.display.is_none() {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "another browser reservation",
                ));
            }
            Ok(CommandResult::BrowserOpen(
                orch_link_contract::browser::BrowserOpenResult {
                    service: format!("browser:{}", body.browser_id),
                    endpoint: "http://127.0.0.1:9291".into(),
                    program: "synthetic chromium".into(),
                    started: true,
                    running: None,
                },
            ))
        });
    Ok(())
}

async fn display_rejoin(restart: bool) -> Proof {
    let scene = Scene::configured(true).await?;
    let call = if restart {
        "display-native-before-answer"
    } else {
        "display-overlapping-retry"
    };
    let canonical = crate::ids::tool_result(scene.thread, call);
    let point = if restart {
        Point::NativeCompleted
    } else {
        Point::Native
    };
    let (paused, release) = gate(scene.thread, &canonical.to_string(), point);
    let mut release = Some(release);
    if restart {
        expect_task(scene.thread, call);
    }
    let args = json!({"command":"open","name":"One owned synthetic screen","geometry":"1024x768"});
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        None,
        OrchTool::UseDisplay,
        args.clone(),
    )
    .await;
    reached(
        "display durable native stage before canonical answer",
        paused,
    )
    .await?;
    let mut conn = scene.world.pool.acquire().await?;
    let (id,body,deadline,created):(Id,Json,chrono::DateTime<chrono::Utc>,chrono::DateTime<chrono::Utc>)=
        sqlx::query_as("SELECT c.id,c.body,c.deadline_at,c.created_at FROM thread_tool_commands e JOIN commands c ON c.id=e.command_id
            WHERE e.answer_id=$1 AND c.kind='display.open'")
            .bind(canonical).fetch_one(&mut *conn).await?;
    let display: Id = serde_json::from_value(body["display_id"].clone())?;
    assert_eq!(
        deadline - created,
        chrono::Duration::seconds(i64::from(CommandKind::DisplayOpen.deadline_secs()))
    );
    let row = orch_db::queries::displays::get(&mut conn, display)
        .await?
        .ok_or("the native reservation is absent")?;
    assert_eq!(row.state, "opening");
    assert_eq!(row.thread_id, Some(scene.thread));
    assert!(
        scene
            .world
            .engine()
            .inner
            .commands
            .retained_body(canonical)
            .await?
            .is_none()
    );
    drop(conn);
    if restart {
        assert!(matches!(
            scene
                .world
                .engine()
                .inner
                .commands
                .status(id)
                .await?
                .and_then(|info| info.outcome),
            Some(Outcome::Succeeded(_))
        ));
        let entry: orch_link::journal::Entry = serde_json::from_slice(
            &tokio::fs::read(
                scene
                    .world
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{id}.json")),
            )
            .await?,
        )?;
        assert!(
            matches!(entry.step, orch_link::journal::Step::Done { .. }),
            "the actual native success must be durable before the owner disappears"
        );
        interrupt_task(scene.thread, call).await?;
        drop(release.take());
        assert!(
            scene
                .world
                .engine()
                .inner
                .commands
                .retained_body(canonical)
                .await?
                .is_none(),
            "no test may manufacture the absent canonical answer"
        );
        scene.world.restart_api().await;
        assert_eq!(
            scene
                .world
                .scalar::<Id>(
                    "SELECT current_turn_id FROM threads WHERE node_id=$1",
                    scene.thread
                )
                .await,
            scene.start.turn_id
        );
    }
    let attempt = new_id();
    let retry = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        Some(attempt),
        OrchTool::UseDisplay,
        args,
    )
    .await?;
    assert!(
        retry.error.is_none(),
        "rejoining the admitted reservation must finish its own native result: {:?}",
        retry.error
    );
    assert_eq!(
        retry
            .result
            .as_ref()
            .and_then(|value| value["display"].as_str())
            .map(str::to_owned),
        Some(display.to_string())
    );
    if !restart {
        release
            .take()
            .ok_or("the original display gate is absent")?
            .send(())
            .map_err(|_| "the original overlapping display owner vanished")?;
    }
    let original = Scene::answer(&scene.world, scene.thread, call, None).await?;
    let mut expected = original.clone();
    expected.attempt = Some(attempt);
    assert_eq!(retry, expected);
    let mut conn = scene.world.pool.acquire().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM display_sessions WHERE thread_id=$1")
            .bind(scene.thread)
            .fetch_one(&mut *conn)
            .await?,
        1,
        "one logical call reserves exactly one screen"
    );
    assert_eq!(
        orch_db::queries::displays::get(&mut conn, display)
            .await?
            .ok_or("the joined display is absent")?
            .state,
        "open"
    );
    let retained = link::command(&mut conn, id)
        .await?
        .ok_or("native display command lost")?;
    assert_eq!((retained.body, retained.deadline_at), (body, deadline));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM thread_tool_commands WHERE answer_id=$1 AND command_id=$2"
        )
        .bind(canonical)
        .bind(id)
        .fetch_one(&mut *conn)
        .await?,
        1
    );
    drop(conn);
    assert_eq!(
        scene.world.machine.handler.runs_of(id),
        1,
        "neither an overlapping carry nor an API restart may reopen the same screen"
    );
    assert_eq!(scene.world.began(CommandKind::DisplayOpen), 1);
    assert_eq!(
        scene.world.turn_starts(scene.thread).len(),
        1,
        "retrying native completion cannot author another input or turn"
    );
    scene.world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_native_display_result_survives_owner_loss_before_canonical_answer_and_rejoins_the_original_reservation()
-> Proof {
    display_rejoin(true).await
}

#[tokio::test]
async fn an_overlapping_retained_call_rejoins_one_pending_display_command_and_reservation() -> Proof
{
    display_rejoin(false).await
}

#[tokio::test]
async fn an_overlapping_browser_call_rejoins_its_original_window_reservation_and_each_native_stage()
-> Proof {
    let scene = Scene::configured(true).await?;
    let call = "browser-retained-window";
    let canonical = crate::ids::tool_result(scene.thread, call);
    let (paused, release) = gate(scene.thread, &canonical.to_string(), Point::NativeDisplay);
    let args = json!({"command":"open","profile":"source-window","preset":"window"});
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        None,
        OrchTool::UseBrowser,
        args.clone(),
    )
    .await;
    reached(
        "browser owns its virtual window before the first display issue",
        paused,
    )
    .await?;
    let mut conn = scene.world.pool.acquire().await?;
    let (browser, display): (Id, Id) = sqlx::query_as(
        "SELECT id,display_id FROM browser_sessions WHERE thread_id=$1 AND state='opening'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    let (command, deadline): (Id, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT c.id,c.deadline_at FROM thread_tool_commands e
        JOIN commands c ON c.id=e.command_id WHERE e.answer_id=$1 AND c.kind='display.open'",
    )
    .bind(canonical)
    .fetch_one(&mut *conn)
    .await?;
    let reserved = orch_db::queries::displays::get(&mut conn, display)
        .await?
        .ok_or("the browser's actual reserved screen is absent")?;
    assert_eq!(reserved.geometry.as_deref(), Some("1280x800"));
    assert_eq!(reserved.state, "opening");
    drop(conn);
    assert!(
        scene
            .world
            .engine()
            .inner
            .commands
            .retained_body(canonical)
            .await?
            .is_none()
    );
    let attempt = new_id();
    let joined = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        Some(attempt),
        OrchTool::UseBrowser,
        args,
    )
    .await?;
    assert!(
        joined.error.is_none(),
        "the same call must join its prepared screen/browser rather than waiting for itself: {:?}",
        joined.error
    );
    assert_eq!(
        joined
            .result
            .as_ref()
            .and_then(|body| body["browser"].as_str())
            .map(str::to_owned),
        Some(browser.to_string())
    );
    release
        .send(())
        .map_err(|_| "the original browser owner vanished")?;
    let original = Scene::answer(&scene.world, scene.thread, call, None).await?;
    let mut expected = original;
    expected.attempt = Some(attempt);
    assert_eq!(joined, expected);
    let mut conn = scene.world.pool.acquire().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM display_sessions WHERE thread_id=$1")
            .bind(scene.thread)
            .fetch_one(&mut *conn)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM browser_sessions WHERE thread_id=$1")
            .bind(scene.thread)
            .fetch_one(&mut *conn)
            .await?,
        1
    );
    assert_eq!(
        orch_db::queries::browsers::get(&mut conn, browser)
            .await?
            .ok_or("the retained browser is absent")?
            .display_id,
        Some(display)
    );
    let saved = link::command(&mut conn, command)
        .await?
        .ok_or("the retained window command is absent")?;
    assert_eq!(saved.deadline_at, deadline);
    assert_eq!(saved.body["display_id"], json!(display));
    drop(conn);
    assert_eq!(scene.world.machine.handler.runs_of(command), 1);
    assert_eq!(scene.world.began(CommandKind::DisplayOpen), 1);
    assert_eq!(scene.world.began(CommandKind::BrowserOpen), 1);
    assert_eq!(scene.world.turn_starts(scene.thread).len(), 1);
    scene.world.shutdown().await;
    Ok(())
}

#[derive(Clone, Copy)]
enum TargetChange {
    None,
    Export,
    Attachment,
    AdmittedExport,
}

async fn target_admission(change: TargetChange) -> Proof {
    use orch_db::queries::{access, documents, storage};
    let mut scene = Scene::new().await?;
    scene.shared_download_target().await?;
    let (_native, destination, export) = download_peer(&scene, false).await?;
    let mut path = format!("id:{}", scene.document);
    let mut file = None;
    if matches!(change, TargetChange::Attachment) {
        let attached=Scene::ask(&scene.world,&scene.source,scene.start.turn_id,"admission-attachment",None,OrchTool::EditDocument,
            json!({"command":"attach","path":scene.path,"file":"out/result.bin","name":"original-file.bin"})).await?;
        assert_eq!(
            attached.error.as_ref().map(|error| error.code.as_str()),
            None,
            "the original attachment must complete through its actual source-file Stat/Fetch before the replacement race"
        );
        let read = Scene::ask(
            &scene.world,
            &scene.source,
            scene.start.turn_id,
            "admission-discovery",
            None,
            OrchTool::ReadShared,
            json!({"path":scene.path}),
        )
        .await?;
        let body = read
            .result
            .ok_or("the actual document discovery is absent")?;
        let written = body["entry"]["path"]
            .as_str()
            .ok_or("the actual canonical path is absent")?;
        let parent = written
            .rsplit_once('/')
            .ok_or("the actual document parent is absent")?
            .0;
        let directory = format!("{parent}/{}/", crate::written::files_beside(written));
        let listed = Scene::ask(
            &scene.world,
            &scene.source,
            scene.start.turn_id,
            "admission-file-discovery",
            None,
            OrchTool::ListShared,
            json!({"path":directory,"sort":"name"}),
        )
        .await?;
        let listing = listed
            .result
            .ok_or("the actual attachment listing is absent")?;
        let entries = listing["entries"]
            .as_array()
            .ok_or("the actual attachment entries are absent")?;
        assert_eq!(entries.len(), 1);
        path = entries[0]["path"]
            .as_str()
            .ok_or("the discovered attachment path is absent")?
            .into();
        file = Some(
            documents::file_named(
                &mut *scene.world.pool.acquire().await?,
                scene.document,
                "original-file.bin",
            )
            .await?
            .ok_or("the actual attached file is absent")?
            .file
            .id,
        );
    }
    let call = match change {
        TargetChange::None => "valid-target-admission",
        TargetChange::Export => "revoked-target-admission",
        TargetChange::Attachment => "replaced-target-admission",
        TargetChange::AdmittedExport => "already-admitted-target",
    };
    let canonical = crate::ids::tool_result(scene.thread, call);
    let point = if matches!(change, TargetChange::AdmittedExport) {
        Point::Native
    } else {
        Point::NativePrepared
    };
    let (prepared, release) = gate(scene.thread, &canonical.to_string(), point);
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        None,
        OrchTool::DownloadShared,
        json!({"path":path,"save_to":"copied-doc.md"}),
    )
    .await;
    reached(
        "actual external blob preparation before final target admission",
        prepared,
    )
    .await?;
    assert!(!destination.exists());
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    let admitted: i64 = scene
        .world
        .count(
            "SELECT count(*) FROM thread_tool_commands WHERE answer_id=$1",
            canonical,
        )
        .await;
    assert_eq!(
        admitted,
        if matches!(change, TargetChange::AdmittedExport) {
            1
        } else {
            0
        }
    );
    if matches!(change, TargetChange::Export | TargetChange::AdmittedExport) {
        let mut tx = scene.world.pool.begin().await?;
        assert!(
            access::remove_entry(&mut tx, export, scene.world.ada, scene.world.clock.now()).await?
        );
        access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
    } else if let Some(file) = file {
        let replacement = Bytes::from_static(b"replacement bytes\0\xfe");
        let hash = Hash::of(&replacement);
        scene
            .store
            .put(
                &hash,
                Some(i64::try_from(replacement.len())?),
                stream_of(vec![replacement.clone()]),
            )
            .await?;
        let mut tx = scene.world.pool.begin().await?;
        documents::lock(&mut tx, scene.document)
            .await?
            .ok_or("the attachment's real owner is absent")?;
        storage::register_blob(
            &mut tx,
            &hash,
            i64::try_from(replacement.len())?,
            scene.world.clock.now(),
        )
        .await?;
        storage::pin_blobs(&mut tx, &[hash]).await?;
        assert!(
            documents::replace_file(
                &mut tx,
                &documents::NewFile {
                    id: file,
                    document: scene.document,
                    blob: hash,
                    name: "original-file.bin",
                    media_type: "application/octet-stream",
                    size: i64::try_from(replacement.len())?,
                    is_text: false,
                    added_by: Some(scene.world.ada),
                    added_by_thread: None,
                    at: scene.world.clock.now()
                }
            )
            .await?
        );
        storage::unpin_blobs(&mut tx, &[scene.blob], scene.world.clock.now()).await?;
        documents::touch(&mut tx, scene.document)
            .await?
            .ok_or("the actual attachment owner cannot be stamped")?;
        tx.commit().await?;
    }
    let before = scene.snapshot().await?;
    release
        .send(())
        .map_err(|_| "the actual download preparation owner vanished")?;
    let answer = Scene::answer(&scene.world, scene.thread, call, None).await?;
    if matches!(change, TargetChange::Export | TargetChange::Attachment) {
        assert_eq!(
            answer.error.as_ref().map(|error| error.code.as_str()),
            Some(if matches!(change, TargetChange::Export) {
                "not_downloadable"
            } else {
                "changed"
            })
        );
        assert_eq!(answer.result, None);
        assert!(!destination.exists());
        assert_eq!(
            scene
                .world
                .count(
                    "SELECT count(*) FROM thread_tool_commands WHERE answer_id=$1",
                    canonical
                )
                .await,
            0
        );
        assert_eq!(
            scene.world.began(CommandKind::FsWrite),
            0,
            "current target refusal must precede any native effect admission"
        );
        assert_eq!(
            scene.snapshot().await?,
            before,
            "a target refusal cannot rewrite the original or selected queue/document/history/state"
        );
        assert!(
            !answer
                .error
                .as_ref()
                .ok_or("the truthful target refusal is absent")?
                .message
                .contains("original-file.bin")
        );
        let readable = Scene::ask(
            &scene.world,
            &scene.source,
            scene.start.turn_id,
            "read-after-target-refusal",
            None,
            OrchTool::ReadShared,
            json!({"path":format!("id:{}",scene.document)}),
        )
        .await?;
        assert!(
            readable.error.is_none(),
            "removing Export or replacing bytes must not remove independent Read"
        );
    } else {
        assert!(answer.error.is_none());
        assert_eq!(
            answer
                .result
                .as_ref()
                .and_then(|body| body["status"].as_str()),
            Some("ready")
        );
        assert_eq!(scene.world.began(CommandKind::FsWrite), 1);
        let command: Id = scene
            .world
            .scalar(
                "SELECT command_id FROM thread_tool_commands WHERE answer_id=$1",
                canonical,
            )
            .await;
        let body = link::command(&mut *scene.world.pool.acquire().await?, command)
            .await?
            .ok_or("the admitted target command disappeared")?
            .body;
        let hash: Hash = serde_json::from_value(body["blob"].clone())?;
        assert_eq!(Hash::of(&tokio::fs::read(&destination).await?), hash);
        assert_eq!(scene.world.machine.handler.runs_of(command), 1);
    }
    assert_eq!(
        scene
            .world
            .scalar::<Id>(
                "SELECT current_turn_id FROM threads WHERE node_id=$1",
                scene.thread
            )
            .await,
        scene.start.turn_id
    );
    assert_eq!(scene.world.turn_starts(scene.thread).len(), 1);
    scene.world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn removing_only_export_after_blob_preparation_refuses_before_native_download_admission()
-> Proof {
    target_admission(TargetChange::Export).await
}

#[tokio::test]
async fn replacing_the_exact_attachment_after_blob_preparation_refuses_before_native_download_admission()
-> Proof {
    target_admission(TargetChange::Attachment).await
}

#[tokio::test]
async fn unchanged_current_target_is_admitted_after_real_blob_preparation() -> Proof {
    target_admission(TargetChange::None).await
}

#[tokio::test]
async fn export_revocation_after_valid_durable_admission_does_not_undo_its_original_native_effect()
-> Proof {
    target_admission(TargetChange::AdmittedExport).await
}

#[test]
fn source_generation_retention_runs_with_default_worker_stacks_even_if_the_suite_stack_is_larger() {
    common::on_default_stack(
        "tools::source::tests::a_changed_directory_generation_retains_its_old_hold_until_real_detach_and_the_original_native_key_drain",
    );
}

#[tokio::test]
async fn a_changed_directory_generation_retains_its_old_hold_until_real_detach_and_the_original_native_key_drain()
-> Proof {
    use orch_envhost::{
        blobs::MemoryBlobs,
        environment::{EnvHost, Gates, HostSettings, Reports},
    };
    use orch_link_contract::commands::{EnvAttach, EnvDetach, IndexMode};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let scene = Scene::new().await?;
    let (_unused, _destination, _export) = download_peer(&scene, false).await?;
    let root = scene.directory.path().join("retiring-workspace");
    tokio::fs::create_dir_all(root.join("web")).await?;
    let mut settings = HostSettings::connected(
        scene.directory.path().join("retiring-host"),
        vec![scene.directory.path().to_owned()],
    );
    settings.gates = Gates::OPEN;
    settings.generation = 1;
    let (reports, mut reported) = Reports::channel(256);
    let drain = tokio::spawn(async move { while reported.recv().await.is_some() {} });
    let host = EnvHost::new(
        settings,
        scene.world.clock.clone(),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    let environment = scene.world.environment;
    host.env_attach(
        &EnvAttach {
            environment_id: environment,
            workspace_root: Some(root.display().to_string()),
            resources: None,
            data_generation: None,
            storage: None,
            data_sources: None,
            ignore: Vec::new(),
            index: IndexMode::Live,
            mounts: None,
            variables: Default::default(),
            secrets: Default::default(),
        },
        &Default::default(),
    )
    .await?;
    let peer = Arc::new(AttachedPeer {
        inner: scene.world.machine.handler.clone(),
        host,
        blobs: StoredNativeInput(scene.store.clone()),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        writes: AtomicUsize::new(0),
        detached: AtomicBool::new(false),
    });
    scene.world.machine.link.serve(peer.clone());
    let call = "retiring-real-attachment";
    let canonical = crate::ids::tool_result(scene.thread, call);
    let (admitted, release) = gate(scene.thread, &canonical.to_string(), Point::Native);
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        None,
        OrchTool::DownloadShared,
        json!({"path":format!("id:{}",scene.document),"save_to":"copied-doc.md"}),
    )
    .await;
    reached("real directory native admission before issue", admitted).await?;
    let id: Id = scene
        .world
        .scalar(
            "SELECT command_id FROM thread_tool_commands WHERE answer_id=$1",
            canonical,
        )
        .await;
    let original = link::command(&mut *scene.world.pool.acquire().await?, id)
        .await?
        .ok_or("the original admitted write is absent")?;
    let hash: Hash = serde_json::from_value(original.body["blob"].clone())?;
    scene.end_original().await?;
    release
        .send(())
        .map_err(|_| "the real directory admission owner vanished")?;
    common::eventually(
        "the real host write owns the original environment key",
        || async { peer.entered.available_permits() == 1 },
    )
    .await;
    let detach_id = new_id();
    scene
        .world
        .engine()
        .inner
        .commands
        .issue(Outgoing::new(
            detach_id,
            scene.world.machine_id,
            SerialKey::Environment(environment),
            &EnvDetach {
                environment_id: environment,
            },
        )?)
        .await?;
    common::eventually(
        "the real detach is durably accepted behind that write",
        || async {
            scene
                .world
                .engine()
                .inner
                .commands
                .status(detach_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|info| info.stage == crate::commands::Stage::Accepted)
        },
    )
    .await;
    assert!(scene.world.engine().inner.commands.cancel(id).await?);
    assert_eq!(
        Scene::answer(&scene.world, scene.thread, call, None)
            .await?
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("unavailable")
    );
    // Reproduce the real metadata transition which used to hide the still-live old
    // hold from native_hold. It is deliberately not used as native completion proof.
    sqlx::query("UPDATE environments SET generation=generation+1,machine_id=NULL WHERE id=$1")
        .bind(environment)
        .execute(scene.world.pool.pool())
        .await?;
    reap(&scene.world.engine(), Some(scene.thread)).await?;
    let mut conn = scene.world.pool.acquire().await?;
    let (settled, fence): (bool, Option<Id>) =
        sqlx::query_as("SELECT settled,fence_id FROM thread_tool_commands WHERE command_id=$1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    assert!(
        !settled,
        "neither a changed generation/machine nor controller cancellation proves the write ended"
    );
    let fence=fence.ok_or("the original immutable hold must remain discoverable after its current environment changes")?;
    let read = link::command(&mut conn, fence)
        .await?
        .ok_or("the old-key completion read is absent")?;
    assert_eq!(read.machine_id, scene.world.machine_id);
    assert_eq!(read.serial_key(), original.serial_key());
    assert_eq!(read.kind, CommandKind::MachineProbe.as_str());
    assert!(
        !orch_db::queries::storage::forget_blob(
            &mut conn,
            &hash,
            scene.world.clock.now() + chrono::Duration::seconds(1)
        )
        .await?
    );
    drop(conn);
    assert!(!peer.detached.load(Ordering::SeqCst));
    assert!(peer.host.environment(environment).is_ok());
    assert_eq!(scene.world.machine.handler.runs_of(fence), 0);
    assert!(!root.join("web/copied-doc.md").exists());
    peer.release.add_permits(1);
    common::eventually(
        "the actual host detach and the ordinary old-key completion read both finish",
        || async {
            peer.detached.load(Ordering::SeqCst)
                && scene
                    .world
                    .engine()
                    .inner
                    .commands
                    .status(fence)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|info| matches!(info.outcome, Some(Outcome::Succeeded(_))))
        },
    )
    .await;
    assert!(
        peer.host.environment(environment).is_err(),
        "the environment is actually absent, not just marked changed in metadata"
    );
    assert_eq!(peer.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        Hash::of(&tokio::fs::read(root.join("web/copied-doc.md")).await?),
        hash
    );
    let entry: orch_link::journal::Entry = serde_json::from_slice(
        &tokio::fs::read(
            scene
                .world
                .machine
                .data_dir()
                .join("journal")
                .join(format!("{detach_id}.json")),
        )
        .await?,
    )?;
    let orch_link::journal::Step::Done { result } = entry.step else {
        return Err("the actual detach did not durably finish".into());
    };
    assert_eq!(result.id, detach_id);
    assert!(
        result.outcome.is_ok(),
        "retirement must be an actual successful native result"
    );
    reap(&scene.world.engine(), Some(scene.thread)).await?;
    reap(&scene.world.engine(), Some(scene.thread)).await?;
    assert!(
        scene
            .world
            .scalar::<bool>(
                "SELECT settled FROM thread_tool_commands WHERE command_id=$1",
                id
            )
            .await
    );
    assert_eq!(
        scene
            .world
            .engine()
            .inner
            .commands
            .status(id)
            .await?
            .and_then(|info| info.outcome),
        Some(Outcome::Cancelled)
    );
    let retained = link::command(&mut *scene.world.pool.acquire().await?, id)
        .await?
        .ok_or("the retained original command disappeared")?;
    assert_eq!(
        (retained.body, retained.deadline_at),
        (original.body, original.deadline_at)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(hash.as_bytes().as_slice())
            .fetch_one(scene.world.pool.pool())
            .await?,
        0
    );
    assert_eq!(scene.world.began(CommandKind::SessionPrefixOpen), 0);
    assert_eq!(scene.world.turn_starts(scene.thread).len(), 1);
    scene.world.shutdown().await;
    drain.abort();
    let _ = drain.await;
    Ok(())
}
