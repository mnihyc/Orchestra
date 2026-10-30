//! Production dispatcher admission with the actual command server and its file-backed journal.
//! The transport forwards exact journal results through an authenticated test gateway cut;
//! it does not claim TLS/enrollment or vendor model-history proof. The selection submodule
//! composes the same dispatcher cut with an actual guarded continuation job.

use orch_types::Id;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, PoisonError},
};
use tokio::sync::oneshot;

mod selection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Phase {
    Adoption,
    Wire,
    Entry,
}
struct Gate {
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}
type Gates = HashMap<(Id, Phase), Gate>;
static GATES: OnceLock<Mutex<Gates>> = OnceLock::new();

async fn pause(id: Id, phase: Phase) {
    let gate = GATES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(id, phase));
    if let Some(gate) = gate {
        let _ = gate.reached.send(());
        let _ = gate.release.await;
    }
}

pub(crate) async fn adoption_read(link: &super::Link, id: Id) {
    pause(id, Phase::Adoption).await;
    selection::adoption_read(link, id).await;
}
pub(crate) async fn wire_checked(id: Id) {
    pause(id, Phase::Wire).await;
}
pub(crate) async fn entry_loaded(machine: Id) {
    pause(machine, Phase::Entry).await;
}

use super::{
    BackplaneFrame, Db, Down, DownFrame, GatewayConn, Link, Outbound, ServerConfig, UpFrame,
};
use crate::dispatch::{CommandOutcome, Dispatcher, StoredCommand};
use orch_db::queries::{hierarchy, link as rows, resources};
use orch_link::{
    commands::{CommandContext, CommandHandler, CommandServer, ServerOptions},
    journal::{Journal, Step},
};
use orch_link_contract::{
    commands::{
        ApprovalMode, Cmd, CmdAck, Command, CommandError, CommandKind, CommandResult, Empty,
        ErrorCode, ExpectedHash, FsWrite, MachineProbe, SandboxMode, SerialKey, SessionContext,
        TurnStart,
    },
    session::{MachineFacts, Role},
};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Clock, Hash, ManualClock, Time, new_id};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
// The existing backplane source proofs use this same native acknowledgement watchdog.
const WAIT: Duration = Duration::from_secs(30);

fn gate(id: Id, phase: Phase) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached, waiting) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    assert!(
        GATES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                (id, phase),
                Gate {
                    reached,
                    release: resume
                }
            )
            .is_none()
    );
    (waiting, release)
}

async fn within<T>(work: impl std::future::Future<Output = T>) -> Proof<T> {
    tokio::time::timeout(WAIT, work)
        .await
        .map_err(|_| "the existing native acknowledgement watchdog elapsed".into())
}

async fn until(mut condition: impl FnMut() -> bool) -> Proof {
    within(async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

struct Native {
    // Native background finalization retains its actual journal directory even if
    // a failed assertion drops the outer fixture before the final disk write.
    _directory: Arc<tempfile::TempDir>,
    environment: Id,
    payload: PathBuf,
    destination: PathBuf,
    clock: Arc<dyn Clock>,
    expected_path: String,
    blobs: Option<Arc<dyn orch_blob_contract::store::BlobStore>>,
    hold: bool,
    entered: AtomicBool,
    release: Semaphore,
    reads: AtomicUsize,
    writes: AtomicUsize,
    probes: AtomicUsize,
    starts: AtomicUsize,
    wire: Mutex<Vec<Cmd>>,
}

#[async_trait::async_trait]
impl CommandHandler for Native {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        if !matches!(
            &command,
            Command::SessionContext(_)
                | Command::FsStat(_)
                | Command::FsRecord(_)
                | Command::TurnStart(_)
        ) && context.key.as_deref() != Some(self.environment.to_string().as_str())
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "another native environment queue",
            ));
        }
        match command {
            Command::FsWrite(body) => {
                if body.environment_id != self.environment
                    || body.path != self.expected_path
                    || body.expected_hash != Some(ExpectedHash::Absent)
                {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "another native destination or overwrite rule",
                    ));
                }
                self.entered.store(true, Ordering::SeqCst);
                if self.hold {
                    self.release
                        .acquire()
                        .await
                        .map_err(|_| {
                            CommandError::new(ErrorCode::Internal, "the owned native gate ended")
                        })?
                        .forget();
                }
                // A real already-started I/O operation may not stop immediately with its token.
                // Read actual bytes only now, after the admission/barrier interleaving.
                let bytes = if let Some(store) = &self.blobs {
                    let found = store.get(&body.blob, None).await.map_err(|_| {
                        CommandError::new(ErrorCode::NotFound, "the actual admitted blob is absent")
                    })?;
                    orch_blob_contract::store::read_all(found.body)
                        .await
                        .map_err(|_| {
                            CommandError::new(
                                ErrorCode::Internal,
                                "the actual admitted bytes could not be read",
                            )
                        })?
                        .to_vec()
                } else {
                    tokio::fs::read(&self.payload).await.map_err(|_| {
                        CommandError::new(ErrorCode::NotFound, "the native input is absent")
                    })?
                };
                self.reads.fetch_add(1, Ordering::SeqCst);
                if Hash::of(&bytes) != body.blob {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the native input changed",
                    ));
                }
                use std::io::Write;
                let mut target = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&self.destination)
                    .map_err(|_| {
                        CommandError::new(ErrorCode::Conflict, "the destination is present")
                    })?;
                target.write_all(&bytes).map_err(|_| {
                    CommandError::new(ErrorCode::Internal, "the actual write failed")
                })?;
                self.writes.fetch_add(1, Ordering::SeqCst);
                Ok(CommandResult::FsWrite(
                    orch_api_contract::workspace::FileStat {
                        entry: orch_api_contract::workspace::DirEntry {
                            name: body.path.rsplit('/').next().unwrap_or_default().into(),
                            path: body.path,
                            kind: orch_api_contract::workspace::DirEntryKind::File,
                            size: i64::try_from(bytes.len()).map_err(|_| {
                                CommandError::new(
                                    ErrorCode::TooLarge,
                                    "the synthetic input is too large",
                                )
                            })?,
                            mtime: self.clock.time(),
                            mode: 0o600,
                            link_target: None,
                            cached: false,
                        },
                        hash: Some(body.blob),
                        media_type: "application/octet-stream".into(),
                        is_text: false,
                    },
                ))
            }
            Command::MachineProbe(_) => {
                self.probes.fetch_add(1, Ordering::SeqCst);
                let facts:MachineFacts=serde_json::from_value(json!({"hostname":"owned-admission-peer","os_version":"synthetic",
                    "cpus":1,"memory_bytes":1024,"harnesses":[],"adapters":[],"plugins":[],"policy":null,
                    "rates":{"up_bps":0,"down_bps":0,"round_trip_ms":0},"clock_offset_ms":0}))
                    .map_err(|_|CommandError::new(ErrorCode::Internal,"the owned facts have an invalid shape"))?;
                Ok(CommandResult::MachineProbe(facts))
            }
            Command::TurnStart(body) => {
                if context.key.as_deref() != Some(body.thread_id.to_string().as_str()) {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "another native thread queue",
                    ));
                }
                self.starts.fetch_add(1, Ordering::SeqCst);
                Ok(CommandResult::TurnStart(Empty {}))
            }
            Command::SessionContext(body) => {
                if context.key.as_deref() != Some(body.thread_id.to_string().as_str()) {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "another observation queue",
                    ));
                }
                // This component peer has no model session. The actual native answer
                // must truthfully say that no context can be counted here.
                Ok(CommandResult::SessionContext(
                    orch_link_contract::commands::SessionContextResult { fill: None },
                ))
            }
            Command::FsStat(body) => {
                if body.environment_id != self.environment
                    || body.mount.is_some()
                    || !matches!(body.path.as_str(), "" | "web")
                {
                    return Err(CommandError::new(
                        ErrorCode::NotFound,
                        "the declared directory is absent",
                    ));
                }
                let directory = self._directory.path().join(&body.path);
                if !tokio::fs::metadata(directory)
                    .await
                    .is_ok_and(|metadata| metadata.is_dir())
                {
                    return Err(CommandError::new(
                        ErrorCode::NotFound,
                        "the owned directory is absent",
                    ));
                }
                Ok(CommandResult::FsStat(
                    orch_api_contract::workspace::FileStat {
                        entry: orch_api_contract::workspace::DirEntry {
                            name: body.path.clone(),
                            path: body.path,
                            kind: orch_api_contract::workspace::DirEntryKind::Dir,
                            size: 0,
                            mtime: self.clock.time(),
                            mode: 0o700,
                            link_target: None,
                            cached: false,
                        },
                        hash: None,
                        media_type: "inode/directory".into(),
                        is_text: false,
                    },
                ))
            }
            Command::FsRecord(body) => {
                if body.environment_id != self.environment || body.directory != "web" {
                    return Err(CommandError::new(
                        ErrorCode::NotFound,
                        "the declared source directory is absent",
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
            }
            _ => Err(CommandError::new(
                ErrorCode::Unsupported,
                "no other native command is declared by this peer",
            )),
        }
    }
}

struct Peer {
    stop: CancellationToken,
    task: Option<JoinHandle<Proof>>,
    link: Link,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.cancel();
        self.link.shutdown();
    }
}

impl Peer {
    async fn join(mut self) -> Proof {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            within(task).await???;
        }
        Ok(())
    }
}

/// One ordered gateway cut. Acknowledgements are forwarded only after the actual
/// native journal accepted that command; terminal results are its exact durable CmdRes.
fn peer(
    link: Link,
    machine: Id,
    server: CommandServer,
    native: Arc<Native>,
    delay_cancel: bool,
) -> (Peer, oneshot::Receiver<()>) {
    let stop = CancellationToken::new();
    let ended = stop.clone();
    let (ready, waiting) = oneshot::channel();
    let owned_link = link.clone();
    let task = tokio::spawn(async move {
        let (out, mut incoming) = mpsc::unbounded_channel();
        let gateway = Arc::new(GatewayConn {
            generation: 1,
            out,
            closed: CancellationToken::new(),
            stats: Mutex::new(None),
            started_at: link.inner.clock.time(),
        });
        let session = new_id();
        link.attach(&gateway, machine, session, None).await;
        ready
            .send(())
            .map_err(|_| "the owner stopped before its gateway attached")?;
        let mut results = JoinSet::<Proof>::new();
        loop {
            let message =
                tokio::select! {()=ended.cancelled()=>break,message=incoming.recv()=>message};
            let Some(Outbound::Frame(frame)) = message else {
                return Err("the proof gateway received a non-frame or closed unexpectedly".into());
            };
            let BackplaneFrame::Down(Down { machine_id, frame }) = *frame else {
                return Err("the proof gateway received an undeclared backplane frame".into());
            };
            if machine_id != machine {
                return Err("the proof gateway received another machine's command".into());
            }
            match frame {
                DownFrame::Cmd(command) => {
                    let id = command.id;
                    native
                        .wire
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(command.clone());
                    server.on_cmd(command).await;
                    let server = server.clone();
                    let link = link.clone();
                    let gateway = gateway.clone();
                    results.spawn(async move {
                        within(async {
                            let mut accepted = false;
                            loop {
                                if let Some(entry) = server.entry(id) {
                                    if !accepted {
                                        link.up(
                                            &gateway,
                                            machine,
                                            session,
                                            UpFrame::CmdAck(CmdAck { id }),
                                        )
                                        .await;
                                        accepted = true;
                                    }
                                    if let Step::Done { result } = entry.step {
                                        link.up(
                                            &gateway,
                                            machine,
                                            session,
                                            UpFrame::CmdRes(result),
                                        )
                                        .await;
                                        break;
                                    }
                                }
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                        })
                        .await?;
                        Ok(())
                    });
                }
                DownFrame::CmdCancel(cancel) => {
                    if delay_cancel {
                        until(|| native.entered.load(Ordering::SeqCst)).await?;
                    }
                    server.on_cancel(cancel).await;
                }
                _ => {
                    return Err(
                        "the native admission proof received an undeclared down-frame kind".into(),
                    );
                }
            }
        }
        while let Some(result) = results.join_next().await {
            result??;
        }
        until(|| server.in_hand() == 0 && server.publications() == 0).await?;
        gateway.closed.cancel();
        Ok(())
    });
    (
        Peer {
            stop,
            task: Some(task),
            link: owned_link,
        },
        waiting,
    )
}

struct Fixture {
    test: TestDb,
    link: Link,
    dispatcher: Dispatcher,
    machine: Id,
    environment: Id,
    clock: ManualClock,
    native: Arc<Native>,
    server: CommandServer,
    peers: Vec<Peer>,
    _directory: Arc<tempfile::TempDir>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.link.shutdown();
        self.native.release.add_permits(1);
        for peer in &self.peers {
            peer.stop.cancel();
        }
    }
}

impl Fixture {
    async fn new(hold: bool) -> Proof<Self> {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let clock = clock::manual();
        let link = Link::detached(
            db,
            Arc::new(clock.clone()),
            Arc::new(ServerConfig::default()),
        )?;
        let machine = resources::testing::insert_machine(
            &mut *test.pool.acquire().await?,
            "owned-native-admission",
            "connected",
        )
        .await?;
        sqlx::query("UPDATE machines SET roles=ARRAY['environment','agent'] WHERE id=$1")
            .bind(machine)
            .execute(&test.pool)
            .await?;
        let directory = Arc::new(tempfile::tempdir()?);
        let payload = directory.path().join("payload.bin");
        tokio::fs::write(&payload, b"original owned input\0\xff").await?;
        let environment = new_id();
        let mut conn = test.pool.acquire().await?;
        let owner = resources::testing::insert_user(&mut conn, "admission-owner").await?;
        let home = new_id();
        hierarchy::testing::insert_node(&mut conn, home, "org", None, "Admission", owner).await?;
        sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,placement,home_node_id,workspace_root,machine_id,generation,status)
            VALUES($1,'Owned admission directory',$2,$3,'connected',$3,$4,$5,1,'running')")
            .bind(environment).bind(owner).bind(home).bind(directory.path().to_str().ok_or("the owned directory is not UTF-8")?)
            .bind(machine).execute(&mut *conn).await?;
        drop(conn);
        let native = Arc::new(Native {
            _directory: directory.clone(),
            environment,
            payload,
            destination: directory.path().join("source.bin"),
            clock: Arc::new(clock.clone()),
            expected_path: "source.bin".into(),
            blobs: None,
            hold,
            entered: AtomicBool::new(false),
            release: Semaphore::new(0),
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            probes: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            wire: Mutex::new(Vec::new()),
        });
        let journal = Journal::open(
            &directory.path().join("journal"),
            Duration::from_secs(24 * 3600),
            clock.time(),
        )?;
        let server = CommandServer::new(
            journal,
            Arc::new(clock.clone()),
            ServerOptions {
                roles: vec![Role::Environment, Role::Agent],
                connected: true,
            },
        );
        server.serve(native.clone());
        Ok(Self {
            test,
            dispatcher: Dispatcher::new(link.clone()),
            link,
            machine,
            environment,
            clock,
            native,
            server,
            peers: Vec::new(),
            _directory: directory,
        })
    }

    fn connect(&mut self, delay_cancel: bool) -> oneshot::Receiver<()> {
        let (peer, ready) = peer(
            self.link.clone(),
            self.machine,
            self.server.clone(),
            self.native.clone(),
            delay_cancel,
        );
        self.peers.push(peer);
        ready
    }

    async fn original(&self) -> Proof<StoredCommand> {
        let body = FsWrite {
            work: None,
            environment_id: self.environment,
            path: "source.bin".into(),
            blob: Hash::of(&tokio::fs::read(&self.native.payload).await?),
            mode: Some(0o600),
            expected_hash: Some(ExpectedHash::Absent),
        };
        let id = new_id();
        let key = SerialKey::Environment(self.environment).wire();
        let deadline = Time::new(
            self.clock.now()
                + chrono::Duration::seconds(i64::from(CommandKind::FsWrite.deadline_secs())),
        );
        let body = serde_json::to_value(body)?;
        let mut conn = self.test.pool.acquire().await?;
        assert!(
            rows::insert_command(
                &mut conn,
                &rows::NewCommand {
                    id,
                    machine_id: self.machine,
                    kind: CommandKind::FsWrite.as_str(),
                    body: &body,
                    serial_key: key.as_deref(),
                    deadline_at: deadline.get()
                },
                self.clock.now()
            )
            .await?
        );
        Ok(StoredCommand {
            id: Some(id),
            machine: self.machine,
            kind: CommandKind::FsWrite.as_str().into(),
            key,
            deadline: Some(deadline),
            body,
        })
    }

    async fn probe(&self) -> Proof<Id> {
        Ok(self
            .dispatcher
            .issue(
                self.machine,
                &SerialKey::Environment(self.environment),
                &MachineProbe {},
            )
            .await?
            .id)
    }

    async fn completed_probe(&self) -> Proof<Id> {
        let id = self.probe().await?;
        assert!(matches!(
            within(self.dispatcher.wait(id)).await??,
            CommandOutcome::Succeeded(_)
        ));
        let entry = self
            .server
            .entry(id)
            .ok_or("the successful probe has no actual native journal entry")?;
        assert!(matches!(entry.step,Step::Done {ref result} if result.outcome.is_ok()));
        Ok(id)
    }

    async fn original_retained(&self, original: &StoredCommand) -> Proof {
        let id = original.id.ok_or("the owned original ID is absent")?;
        let saved = rows::command(&mut *self.test.pool.acquire().await?, id)
            .await?
            .ok_or("the canonical command was lost")?;
        assert_eq!(saved.state, "cancelled");
        assert_eq!(saved.body, original.body);
        assert_eq!(saved.serial_key, original.key);
        assert_eq!(
            Time::new(saved.deadline_at),
            original.deadline.ok_or("the original budget is absent")?
        );
        Ok(())
    }

    async fn no_old_effect(&self, original: &StoredCommand) -> Proof {
        let id = original.id.ok_or("the original ID is absent")?;
        assert!(
            self.server.entry(id).is_none(),
            "a withdrawn original never entered the actual native journal"
        );
        assert!(!self.native.destination.exists());
        assert_eq!(self.native.reads.load(Ordering::SeqCst), 0);
        assert_eq!(self.native.writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            tokio::fs::read(&self.native.payload).await?,
            b"original owned input\0\xff"
        );
        assert!(
            !self
                .native
                .wire
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .any(|command| command.id == id)
        );
        self.original_retained(original).await
    }

    async fn close(mut self) -> Proof {
        self.link.shutdown();
        for peer in &self.peers {
            peer.stop.cancel();
        }
        for peer in self.peers.drain(..) {
            peer.join().await?;
        }
        within(self.test.pool.close()).await?;
        Ok(())
    }
}

#[tokio::test]
async fn an_already_connected_dispatcher_adopts_a_retained_turn_start_only_when_issued() -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let thread = new_id();
    let id = new_id();
    let key = SerialKey::Thread(thread).wire();
    let deadline = Time::new(
        fixture.clock.now()
            + chrono::Duration::seconds(i64::from(CommandKind::TurnStart.deadline_secs())),
    );
    let body = serde_json::to_value(TurnStart {
        thread_id: thread,
        turn_id: new_id(),
        text: "the admitted original".into(),
        images: Vec::new(),
        model: "owned-model".into(),
        effort: None,
        params: Default::default(),
        approval_mode: ApprovalMode::Ask,
        sandbox_mode: SandboxMode::ReadOnly,
        native_action: None,
        plan: false,
    })?;
    let original = StoredCommand {
        id: Some(id),
        machine: fixture.machine,
        kind: CommandKind::TurnStart.as_str().into(),
        key,
        deadline: Some(deadline),
        body,
    };
    let mut conn = fixture.test.pool.acquire().await?;
    assert!(
        rows::insert_command(
            &mut conn,
            &rows::NewCommand {
                id,
                machine_id: original.machine,
                kind: &original.kind,
                body: &original.body,
                serial_key: original.key.as_deref(),
                deadline_at: deadline.get(),
            },
            fixture.clock.now(),
        )
        .await?
    );
    drop(conn);

    // The peer is live after the row was admitted, but looking at or waiting
    // for that row does not put the command on an already-established link.
    let queued = fixture
        .dispatcher
        .status(id)
        .await?
        .ok_or("the start was not admitted")?;
    assert_eq!(queued.state, "queued");
    let dispatcher = fixture.dispatcher.clone();
    let waiting = tokio::spawn(async move { dispatcher.wait(id).await });
    let context = SessionContext {
        thread_id: thread,
        segment_id: new_id(),
    };
    // This live observation uses the same serial key as the retained start.
    assert_eq!(
        within(fixture.link.session_context(fixture.machine, &context))
            .await??
            .fill,
        None
    );
    fixture.completed_probe().await?;
    assert!(!waiting.is_finished());
    assert!(fixture.server.entry(id).is_none());
    assert_eq!(fixture.native.starts.load(Ordering::SeqCst), 0);
    assert!(
        !fixture
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|command| command.id == id)
    );

    let issued = fixture.dispatcher.issue_stored(original.clone()).await?;
    assert_eq!(issued.id, id);
    assert_eq!(issued.deadline, deadline);
    assert!(matches!(
        within(waiting).await???,
        CommandOutcome::Succeeded(_)
    ));
    // Re-adopting the retained row remains idempotent after its result.
    fixture.dispatcher.issue_stored(original.clone()).await?;
    assert_eq!(
        within(fixture.link.session_context(fixture.machine, &context))
            .await??
            .fill,
        None
    );
    fixture.completed_probe().await?;
    assert_eq!(fixture.native.starts.load(Ordering::SeqCst), 1);
    let journal = fixture
        .server
        .entry(id)
        .ok_or("the native start was not journaled")?;
    assert!(matches!(journal.step, Step::Done { ref result } if result.outcome.is_ok()));
    let saved = fixture
        .dispatcher
        .status(id)
        .await?
        .ok_or("the original disappeared")?;
    assert_eq!(saved.body, original.body);
    assert_eq!(saved.serial_key, original.key);
    assert_eq!(Time::new(saved.deadline_at), deadline);
    {
        let wire = fixture
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let sent = wire
            .iter()
            .find(|command| command.id == id)
            .ok_or("the start was not sent")?;
        assert_eq!(sent.body, original.body);
        assert_eq!(sent.key, original.key);
        assert_eq!(sent.deadline, deadline);
    }
    fixture.close().await
}

#[tokio::test]
async fn a_cancelled_queued_row_cannot_be_adopted_after_its_actual_native_completion_probe() -> Proof
{
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original ID is absent")?;
    let (read, release) = gate(id, Phase::Adoption);
    let dispatcher = fixture.dispatcher.clone();
    let carried = original.clone();
    let issuer = tokio::spawn(async move { dispatcher.issue_stored(carried).await });
    within(read).await??;
    assert!(fixture.dispatcher.cancel(id).await?);
    fixture.completed_probe().await?;
    fixture.no_old_effect(&original).await?;
    release
        .send(())
        .map_err(|_| "the exact paused issuer was lost")?;
    assert_eq!(within(issuer).await???.id, id);
    fixture.completed_probe().await?;
    fixture.no_old_effect(&original).await?;
    fixture.close().await
}

#[tokio::test]
async fn a_stale_startup_open_row_cannot_resurrect_after_cancellation_before_entry_publication()
-> Proof {
    let mut fixture = Fixture::new(false).await?;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original ID is absent")?;
    let (loaded, release) = gate(fixture.machine, Phase::Entry);
    let ready = fixture.connect(false);
    within(loaded).await??;
    assert!(fixture.link.loaded_entry(fixture.machine).is_none());
    assert!(fixture.dispatcher.cancel(id).await?);
    release
        .send(())
        .map_err(|_| "the exact entry-loading phase was lost")?;
    within(ready).await??;
    fixture.completed_probe().await?;
    fixture.no_old_effect(&original).await?;
    fixture.close().await
}

#[tokio::test]
async fn a_probe_after_cancellation_cannot_pass_the_write_already_owned_by_its_output_iteration()
-> Proof {
    let mut fixture = Fixture::new(true).await?;
    within(fixture.connect(true)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original ID is absent")?;
    let (checked, release) = gate(id, Phase::Wire);
    fixture.dispatcher.issue_stored(original.clone()).await?;
    within(checked).await??;
    assert!(fixture.dispatcher.cancel(id).await?);
    let fence = fixture.probe().await?;
    assert!(fixture.server.entry(id).is_none());
    assert!(fixture.server.entry(fence).is_none());
    release
        .send(())
        .map_err(|_| "the exact checked output phase was lost")?;
    until(|| {
        fixture.native.entered.load(Ordering::SeqCst) && fixture.server.entry(fence).is_some()
    })
    .await?;
    assert!(matches!(
        fixture
            .server
            .entry(id)
            .ok_or("the actual admitted original is absent")?
            .step,
        Step::Started
    ));
    assert!(matches!(
        fixture
            .server
            .entry(fence)
            .ok_or("the actual native queue probe is absent")?
            .step,
        Step::Accepted
    ));
    assert_eq!(
        fixture.native.probes.load(Ordering::SeqCst),
        0,
        "a real accepted probe cannot leave the still-owned native key"
    );
    fixture.native.release.add_permits(1);
    assert!(matches!(
        within(fixture.dispatcher.wait(fence)).await??,
        CommandOutcome::Succeeded(_)
    ));
    assert_eq!(fixture.native.reads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.native.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        tokio::fs::read(&fixture.native.destination).await?,
        b"original owned input\0\xff"
    );
    fixture.original_retained(&original).await?;
    assert!(
        matches!(fixture.server.entry(id).ok_or("the completed native original is absent")?.step,Step::Done {ref result} if result.outcome.is_ok())
    );
    fixture.dispatcher.issue_stored(original.clone()).await?;
    fixture.completed_probe().await?;
    assert_eq!(fixture.native.writes.load(Ordering::SeqCst), 1);
    fixture.close().await
}

#[tokio::test]
async fn a_retired_output_task_cannot_publish_after_the_current_entry_has_completed_its_probe()
-> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original ID is absent")?;
    let (checked, release) = gate(id, Phase::Wire);
    fixture.dispatcher.issue_stored(original.clone()).await?;
    within(checked).await??;
    assert!(fixture.dispatcher.cancel(id).await?);
    fixture.link.rebuild(fixture.machine).await;
    within(fixture.connect(false)).await??;
    fixture.completed_probe().await?;
    release
        .send(())
        .map_err(|_| "the retired output phase was lost")?;
    fixture.completed_probe().await?;
    fixture.no_old_effect(&original).await?;
    fixture.close().await
}

#[tokio::test]
async fn only_an_explicit_live_observation_owner_may_use_its_existing_non_durable_read_route()
-> Proof {
    use orch_link::commands::Progress;
    use orch_link_contract::commands::SessionContext;
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let body = SessionContext {
        thread_id: new_id(),
        segment_id: new_id(),
    };
    let result = within(fixture.link.session_context(fixture.machine, &body)).await??;
    assert_eq!(result.fill, None);
    let commands = fixture
        .native
        .wire
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].kind, CommandKind::SessionContext.as_str());
    assert_eq!(commands[0].body, serde_json::to_value(&body)?);
    assert!(
        rows::command(&mut *fixture.test.pool.acquire().await?, commands[0].id)
            .await?
            .is_none(),
        "an existing live context read does not acquire a durable result/replay row"
    );
    let entry = fixture.link.entry(fixture.machine).await?;
    assert!(
        !entry.owns_observation(commands[0].id),
        "the actual request drops only its observation ownership"
    );
    // A matching kind without that explicit factory/owner is not an exemption.
    let id = new_id();
    let mut unowned = entry.controller.commands().issue_typed(
        id,
        &SerialKey::Thread(body.thread_id),
        Time::new(
            fixture.clock.now()
                + chrono::Duration::seconds(i64::from(CommandKind::SessionContext.deadline_secs())),
        ),
        &body,
    )?;
    let refused = within(unowned.wait()).await?;
    assert!(
        matches!(refused,Progress::Failed(ref error) if error.code==ErrorCode::NotFound.as_str())
    );
    assert!(fixture.server.entry(id).is_none());
    assert_eq!(fixture.native.reads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.native.writes.load(Ordering::SeqCst), 0);
    fixture.close().await
}
