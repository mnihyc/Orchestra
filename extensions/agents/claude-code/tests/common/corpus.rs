//! Native byte-pipe glue only; matching, aliases and the scripted clock belong to the shared replayer.
use async_trait::async_trait;
use orch_adapter_claude_code::process::{
    LaunchSpec, Launcher, ProcessControl, ProcessStatus, ProcessStreams,
};
use orch_testkit::harness_corpus::{Bindings, Frame, Transcript, replay_io};
use orch_types::ManualClock;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader, duplex, split},
    process::Command,
    sync::{Notify, watch},
};

pub const WAIT: Duration = Duration::from_secs(10);

/// Test scheduling between two shared replay slices; no native frame is parsed or matched here.
#[derive(Clone, Debug, Default)]
pub struct ReplayGate {
    reached: Arc<Notify>,
    release: Arc<Notify>,
}
impl ReplayGate {
    pub async fn reached(&self) {
        tokio::time::timeout(WAIT, self.reached.notified())
            .await
            .expect("the real native checkpoint was not reached");
    }
    pub fn release(&self) {
        self.release.notify_one();
    }
}
#[derive(Clone, Debug)]
struct Pause {
    after: usize,
    gate: ReplayGate,
}
struct Control {
    kill: Arc<Notify>,
    ended: watch::Receiver<Option<ProcessStatus>>,
}
#[async_trait]
impl ProcessControl for Control {
    async fn wait(&mut self) -> Option<ProcessStatus> {
        loop {
            let known = *self.ended.borrow_and_update();
            if let Some(status) = known {
                return Some(status);
            }
            if self.ended.changed().await.is_err() {
                return None;
            }
        }
    }
    fn kill(&mut self) {
        self.kill.notify_one();
    }
}

#[derive(Debug)]
pub struct Scripts {
    frames: Mutex<VecDeque<Vec<Frame>>>,
    pub clock: ManualClock,
    bindings: Bindings,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    results: Arc<Mutex<Vec<Result<Transcript, String>>>>,
    replayed: watch::Sender<usize>,
    pub launched: Mutex<Vec<LaunchSpec>>,
    pause: Option<Pause>,
}
impl Scripts {
    pub fn transcripts(&self) -> Vec<Transcript> {
        self.results
            .lock()
            .unwrap()
            .iter()
            .map(|result| result.clone().unwrap())
            .collect()
    }
    pub fn new(frames: Vec<Vec<Frame>>, clock: ManualClock, bindings: Bindings) -> Arc<Self> {
        Self::configured(frames, clock, bindings, None)
    }
    pub fn paused(frames: Vec<Frame>, after: usize, clock: ManualClock) -> (Arc<Self>, ReplayGate) {
        assert!(
            after > 0 && after < frames.len(),
            "the native checkpoint must lie between replay slices"
        );
        let gate = ReplayGate::default();
        let scripts = Self::configured(
            vec![frames],
            clock,
            Bindings::new(),
            Some(Pause {
                after,
                gate: gate.clone(),
            }),
        );
        (scripts, gate)
    }
    fn configured(
        frames: Vec<Vec<Frame>>,
        clock: ManualClock,
        bindings: Bindings,
        pause: Option<Pause>,
    ) -> Arc<Self> {
        let (replayed, _) = watch::channel(0);
        Arc::new(Self {
            frames: Mutex::new(frames.into()),
            clock,
            bindings,
            tasks: Mutex::default(),
            results: Arc::default(),
            launched: Mutex::default(),
            replayed,
            pause,
        })
    }
    /// A native request/reply barrier, not a quiet-time guess about the event stream.
    pub async fn completed(&self) {
        let needed = self.launched.lock().unwrap().len();
        assert_ne!(needed, 0, "no native exchange started");
        let mut replayed = self.replayed.subscribe();
        tokio::time::timeout(WAIT, async {
            loop {
                if *replayed.borrow_and_update() >= needed {
                    break;
                }
                replayed.changed().await.unwrap();
            }
        })
        .await
        .expect("native exchange did not reach its final barrier");
        for result in self.results.lock().unwrap().iter() {
            assert!(result.is_ok(), "native replay failed: {result:?}");
        }
    }
    pub async fn finish(&self) {
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in tasks {
            tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        }
        assert!(
            self.frames.lock().unwrap().is_empty(),
            "an expected native process did not open"
        );
        for result in self.results.lock().unwrap().iter() {
            assert!(result.is_ok(), "native replay failed: {result:?}");
        }
    }
}
impl Launcher for Scripts {
    fn command(&self, _: &LaunchSpec) -> Command {
        panic!("the corpus supplies native byte streams, never an external program");
    }
    fn streams(&self, spec: &LaunchSpec) -> Option<ProcessStreams> {
        self.launched.lock().unwrap().push(spec.clone());
        let frames = self
            .frames
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra native process");
        let (adapter, harness) = duplex(64 * 1024);
        let (stdout, stdin) = split(adapter);
        let (reader, mut writer) = split(harness);
        let mut reader = BufReader::new(reader);
        let (mut stderr, error_reader) = duplex(64 * 1024);
        let (ended, receiver) = watch::channel(None);
        let kill = Arc::new(Notify::new());
        let signal = kill.clone();
        let clock = self.clock.clone();
        let mut bindings = self.bindings.clone();
        let results = self.results.clone();
        let replayed = self.replayed.clone();
        let pause = self.pause.clone();
        let task = tokio::spawn(async move {
            let exchange = async {
                if let Some(pause) = pause {
                    let (before, after) = frames.split_at(pause.after);
                    let mut transcript = replay_io(
                        before,
                        &mut reader,
                        &mut writer,
                        &mut stderr,
                        &clock,
                        &mut bindings,
                        WAIT,
                    )
                    .await?;
                    assert!(
                        transcript.exit_code.is_none(),
                        "a native checkpoint cannot follow exit"
                    );
                    pause.gate.reached.notify_one();
                    pause.gate.release.notified().await;
                    let rest = replay_io(
                        after,
                        &mut reader,
                        &mut writer,
                        &mut stderr,
                        &clock,
                        &mut bindings,
                        WAIT,
                    )
                    .await?;
                    transcript.frames.extend(rest.frames);
                    transcript.exit_code = rest.exit_code;
                    Ok(transcript)
                } else {
                    replay_io(
                        &frames,
                        &mut reader,
                        &mut writer,
                        &mut stderr,
                        &clock,
                        &mut bindings,
                        WAIT,
                    )
                    .await
                }
            };
            let replay = tokio::select! {
                result=exchange=>Some(result),
                ()=signal.notified()=>None,
            };
            let status = match replay {
                Some(Ok(transcript)) => {
                    let code = transcript.exit_code;
                    results.lock().unwrap().push(Ok(transcript));
                    replayed.send_modify(|count| *count += 1);
                    if let Some(code) = code {
                        ProcessStatus { code, signal: None }
                    } else {
                        // A completed script stays alive until the actor closes its input. Unexpected
                        // extra native requests fail instead of being discarded after the barrier.
                        let mut extra = String::new();
                        tokio::select! {
                            ()=signal.notified()=>ProcessStatus{code:None,signal:None},
                            read=reader.read_line(&mut extra)=>{
                                match read {
                                    Ok(0)=>ProcessStatus{code:Some(0),signal:None},
                                    other=>{
                                        results.lock().unwrap().push(Err(format!("unexpected input after native barrier: {other:?} {extra:?}")));
                                        ProcessStatus{code:Some(1),signal:None}
                                    }
                                }
                            }
                        }
                    }
                }
                Some(Err(error)) => {
                    results.lock().unwrap().push(Err(error.to_string()));
                    replayed.send_modify(|count| *count += 1);
                    ProcessStatus {
                        code: Some(1),
                        signal: None,
                    }
                }
                None => {
                    results.lock().unwrap().push(Err(
                        "adapter stopped before its native exchange completed".into(),
                    ));
                    replayed.send_modify(|count| *count += 1);
                    ProcessStatus {
                        code: None,
                        signal: None,
                    }
                }
            };
            ended.send_replace(Some(status));
        });
        self.tasks.lock().unwrap().push(task);
        Some(ProcessStreams {
            stdin: Some(Box::new(stdin)),
            stdout: Some(Box::new(stdout)),
            stderr: Some(Box::new(error_reader)),
            control: Box::new(Control {
                kill,
                ended: receiver,
            }),
        })
    }
}

/// Fixed fixture identities make scope comparisons explicit; none is an account or credential.
pub fn thread() -> orch_types::Id {
    orch_types::Id::from_u128(1)
}
pub fn segment() -> orch_types::Id {
    orch_types::Id::from_u128(2)
}
pub fn turn() -> orch_types::Id {
    orch_types::Id::from_u128(3)
}
pub const SESSION: &str = "00000000-0000-0000-0000-000000000004";

pub fn from_native(frame: orch_types::Json) -> Frame {
    Frame::FromHarness { frame }
}
pub fn to_native(frame: orch_types::Json) -> Frame {
    Frame::ToHarness {
        frame,
        subset: true,
    }
}
pub fn handshake() -> Vec<Frame> {
    vec![
        to_native(
            serde_json::json!({"type":"control_request","request_id":{"$replay_capture":"initialize"},"request":{"subtype":"initialize"}}),
        ),
        from_native(
            serde_json::json!({"type":"control_response","response":{"subtype":"success","request_id":{"$replay_ref":"initialize"},"response":{}}}),
        ),
        from_native(
            serde_json::json!({"type":"system","subtype":"init","session_id":SESSION,"model":"mock-large","uuid":"init","mcp_servers":[]}),
        ),
    ]
}
pub fn input() -> Frame {
    to_native(
        serde_json::json!({"type":"user","uuid":turn(),"message":{"role":"user","content":[{"type":"text","text":"Corpus input"}]}}),
    )
}
pub fn barrier() -> Vec<Frame> {
    vec![
        from_native(
            serde_json::json!({"type":"control_request","request_id":"corpus-barrier","request":{"subtype":"hook_callback","callback_id":"orch-instructions-loaded","input":{}}}),
        ),
        to_native(
            serde_json::json!({"type":"control_response","response":{"subtype":"success","request_id":"corpus-barrier","response":{}}}),
        ),
    ]
}

/// Actual conversation and bounded production stream codec; this does not probe or claim a binary version.
pub struct Actor {
    pub handle: orch_adapter_claude_code::conversation::ConversationHandle,
    pub events: orch_agent_contract::adapter::EventStream,
    pub scripts: Arc<Scripts>,
    pub opened: Result<(), orch_agent_contract::adapter::AdapterError>,
    home: tempfile::TempDir,
    _work: tempfile::TempDir,
}
impl Actor {
    pub async fn start(
        frames: Vec<Frame>,
        origin: orch_adapter_claude_code::sessions::Origin,
        idle: bool,
    ) -> Self {
        Self::start_with_home(frames, origin, idle, false).await
    }
    pub async fn start_with_home(
        frames: Vec<Frame>,
        origin: orch_adapter_claude_code::sessions::Origin,
        idle: bool,
        managed_home: bool,
    ) -> Self {
        let scripts = Scripts::new(vec![frames], orch_testkit::clock::manual(), Bindings::new());
        Self::start_with_scripts(scripts, origin, idle, managed_home).await
    }
    pub async fn start_with_scripts(
        scripts: Arc<Scripts>,
        origin: orch_adapter_claude_code::sessions::Origin,
        idle: bool,
        managed_home: bool,
    ) -> Self {
        Self::start_configured(scripts, origin, idle, managed_home, Default::default()).await
    }
    /// Where the program keeps its files for this conversation.
    pub fn home(&self) -> &std::path::Path {
        self.home.path()
    }
    pub async fn start_configured(
        scripts: Arc<Scripts>,
        origin: orch_adapter_claude_code::sessions::Origin,
        idle: bool,
        managed_home: bool,
        config: orch_adapter_claude_code::config::ProfileConfig,
    ) -> Self {
        Self::start_with_files(
            scripts,
            origin,
            idle,
            managed_home,
            config,
            Arc::new(orch_adapter_claude_code::child_usage::Files),
        )
        .await
    }
    /// As [`Actor::start_configured`], with the transcripts of sub-agents read by `files`.
    pub async fn start_with_files(
        scripts: Arc<Scripts>,
        origin: orch_adapter_claude_code::sessions::Origin,
        idle: bool,
        managed_home: bool,
        config: orch_adapter_claude_code::config::ProfileConfig,
        files: Arc<dyn orch_adapter_claude_code::child_usage::Transcripts>,
    ) -> Self {
        use orch_adapter_claude_code::{
            conversation::{self, Command, Params},
            sessions::{HarnessSetup, TurnSettings},
        };
        use orch_agent_contract::{
            adapter::EventSink,
            descriptor::{ApprovalMode, SandboxMode},
            loopback::Loopback,
            session::{SessionMode, TurnStart},
        };
        use orch_plugin_contract::Secret;
        use serde_json::json;
        let clock = scripts.clock.clone();
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let directory = work.path().to_str().unwrap();
        let spec = super::fixture_spec(
            thread(),
            segment(),
            directory,
            "offline-corpus",
            SessionMode::New,
            None,
        );
        let setup = HarnessSetup {
            program: "corpus-never-executed".into(),
            home: Some(home.path().to_owned()),
            managed_home,
            loopback: Loopback::new(1, Secret::new("offline-corpus")),
            profile_env: Default::default(),
            config,
            inherited: std::collections::BTreeMap::from([(
                "HOME".into(),
                home.path().to_str().unwrap().into(),
            )]),
            as_root: false,
        };
        let (sink, events) = EventSink::channel();
        let handle = conversation::start_with(
            Params {
                thread_id: thread(),
                segment_id: segment(),
                spec,
                setup: Arc::new(setup),
                launcher: scripts.clone(),
                sink,
                clock: Arc::new(clock),
                session_id: SESSION.into(),
                origin,
                token: Arc::new(Mutex::new(None)),
            },
            files,
        );
        let opened = if idle {
            handle
                .ask(|reply| Command::Warm {
                    turn: Box::new(TurnSettings {
                        cache_lifetimes: None,
                        page_fetch: None,
                        model: "mock-large".into(),
                        effort: None,
                        approval_mode: ApprovalMode::Ask,
                        sandbox_mode: SandboxMode::Open,
                        params: json!({}),
                        plan: false,
                    }),
                    reply,
                })
                .await
                .unwrap()
        } else {
            handle
                .ask(|reply| Command::StartTurn {
                    turn: Box::new(TurnStart {
                        native_model: None,
                        thread_id: thread(),
                        turn_id: turn(),
                        text: "Corpus input".into(),
                        images: vec![],
                        native_wake: None,
                        plan: false,
                        model: "mock-large".into(),
                        effort: None,
                        params: json!({}),
                        approval_mode: ApprovalMode::Ask,
                        sandbox_mode: SandboxMode::Open,
                    }),
                    blocks: conversation::message_blocks("Corpus input", vec![]),
                    reply,
                })
                .await
                .unwrap()
        };
        Self {
            handle,
            events,
            scripts,
            opened,
            home,
            _work: work,
        }
    }
    /// The next input of the conversation, for a turn of its own.
    pub async fn next_turn(
        &self,
        turn_id: orch_types::Id,
        text: &str,
    ) -> Result<(), orch_agent_contract::adapter::AdapterError> {
        use orch_adapter_claude_code::conversation::{self, Command};
        use orch_agent_contract::{
            descriptor::{ApprovalMode, SandboxMode},
            session::TurnStart,
        };
        use serde_json::json;
        self.handle
            .ask(|reply| Command::StartTurn {
                turn: Box::new(TurnStart {
                    native_model: None,
                    thread_id: thread(),
                    turn_id,
                    text: text.into(),
                    images: vec![],
                    native_wake: None,
                    plan: false,
                    model: "mock-large".into(),
                    effort: None,
                    params: json!({}),
                    approval_mode: ApprovalMode::Ask,
                    sandbox_mode: SandboxMode::Open,
                }),
                blocks: conversation::message_blocks(text, vec![]),
                reply,
            })
            .await
            .unwrap()
    }
    pub async fn collect(&mut self) -> Vec<orch_testkit::harness_corpus::Observation> {
        use orch_testkit::harness_corpus::Observation;
        let mut rows = Vec::new();
        let scripts = self.scripts.clone();
        let completed = scripts.completed();
        tokio::pin!(completed);
        loop {
            tokio::select! {
                ()=&mut completed=>break,
                event=self.events.recv()=>{
                    rows.push(Observation::from_event(&event.expect("native event stream closed before its barrier")).unwrap());
                }
            }
        }
        while let Ok(event) = self.events.try_recv() {
            rows.push(Observation::from_event(&event).unwrap());
        }
        rows
    }
    pub async fn close(&mut self) {
        use orch_adapter_claude_code::conversation::Command;
        tokio::time::timeout(WAIT, self.handle.ask(|reply| Command::Close { reply }))
            .await
            .unwrap()
            .unwrap();
        self.scripts.finish().await;
    }
}
