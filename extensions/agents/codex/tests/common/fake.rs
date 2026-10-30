//! A harness played in the test: an app-server that answers from a function and says what the test tells it
//! to, over pipes in the process. It stands where the real Codex cannot be made to do something (to ask for
//! permissions, to answer a sign-in that needs the vendor's servers, to be an old version).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test binary uses some of the rig")]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use orch_adapter_codex::process::{
    HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit,
};
use orch_types::Json;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};
use tokio::sync::{Notify, mpsc};

/// What the fake answers a request with: a result, or an error code and message.
pub type Answer = Result<Json, (i64, String)>;

/// Decides the answer to a request, by its method and parameters.
pub type Handler = Arc<dyn Fn(&str, &Json) -> Answer + Send + Sync>;

/// What the test can see and do of the fake.
pub struct FakeState {
    pub received: Mutex<Vec<Json>>,
    pub launches: AtomicU32,
    pub kills: AtomicU32,
    pub requests: Mutex<Vec<LaunchRequest>>,
    inject: Mutex<Option<mpsc::UnboundedSender<Json>>>,
    raw: Mutex<Option<mpsc::UnboundedSender<String>>>,
    stop: Mutex<Option<Arc<Notify>>>,
    version: Mutex<String>,
    stderr: Mutex<String>,
    exit_on_initialize: Mutex<bool>,
    initialize_delay: Mutex<Duration>,
    process_exit: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
}

impl FakeState {
    /// Reports an actual process exit while a descendant still holds the fake streams open.
    pub fn exit_with_open_pipes(&self) {
        self.process_exit
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send_replace(true);
    }

    /// Supplies the final stderr diagnostic of the next process.
    pub fn set_stderr(&self, text: &str) {
        *self.stderr.lock().unwrap() = text.to_owned();
    }
    /// Says something to the adapter as the harness.
    pub fn say(&self, frame: Json) {
        let sender = self
            .inject
            .lock()
            .unwrap()
            .clone()
            .expect("a harness is running");
        sender.send(frame).unwrap();
    }

    /// Writes `text` to the adapter as it is, with no line break after it: half a line, when it is part of one.
    pub fn say_raw(&self, text: &str) {
        let sender = self
            .raw
            .lock()
            .unwrap()
            .clone()
            .expect("a harness is running");
        sender.send(text.to_owned()).unwrap();
    }

    /// Ends the process of the harness, as if it were killed: it writes nothing more and its output ends.
    pub fn die(&self) {
        if let Some(stop) = self.stop.lock().unwrap().as_ref() {
            stop.notify_one();
        }
    }

    /// A notification of the harness.
    pub fn notify(&self, method: &str, params: Json) {
        self.say(json!({ "method": method, "params": params }));
    }

    /// A request of the harness.
    pub fn ask(&self, id: i64, method: &str, params: Json) {
        self.say(json!({ "id": id, "method": method, "params": params }));
    }

    /// The frames the adapter sent, in order.
    pub fn frames(&self) -> Vec<Json> {
        self.received.lock().unwrap().clone()
    }

    /// The frames of the adapter that are calls of `method`.
    pub fn calls(&self, method: &str) -> Vec<Json> {
        self.frames()
            .into_iter()
            .filter(|frame| frame["method"] == method)
            .collect()
    }

    /// The answers the adapter gave to a request of the harness.
    pub fn answer_to(&self, id: i64) -> Option<Json> {
        self.frames()
            .into_iter()
            .find(|frame| frame["id"] == json!(id) && frame.get("method").is_none())
    }

    /// Sets the version the harness reports of itself.
    pub fn set_version(&self, version: &str) {
        *self.version.lock().unwrap() = version.to_owned();
    }

    /// Makes the harness write `text` on its error output and end as soon as it is asked to initialize.
    pub fn die_on_initialize(&self, text: &str) {
        *self.stderr.lock().unwrap() = text.to_owned();
        *self.exit_on_initialize.lock().unwrap() = true;
    }

    /// Delays native initialization without blocking the adapter or its process cancellation.
    pub fn delay_initialize(&self, delay: Duration) {
        *self.initialize_delay.lock().unwrap() = delay;
    }
}

struct FakeControl {
    done: tokio::sync::watch::Receiver<bool>,
    stop: Arc<Notify>,
    state: Arc<FakeState>,
}

#[async_trait]
impl ProcessControl for FakeControl {
    async fn wait(&mut self) -> ProcessExit {
        let _ = self.done.wait_for(|done| *done).await;
        ProcessExit {
            code: Some(0),
            detail: String::new(),
        }
    }

    fn kill(&mut self) {
        self.state.kills.fetch_add(1, Ordering::SeqCst);
        self.stop.notify_one();
    }
}

/// Starts the fake harness.
pub struct FakeLauncher {
    pub state: Arc<FakeState>,
    handler: Handler,
}

impl FakeLauncher {
    pub fn new(handler: impl Fn(&str, &Json) -> Answer + Send + Sync + 'static) -> Self {
        Self {
            state: Arc::new(FakeState {
                received: Mutex::new(Vec::new()),
                launches: AtomicU32::new(0),
                kills: AtomicU32::new(0),
                requests: Mutex::new(Vec::new()),
                inject: Mutex::new(None),
                raw: Mutex::new(None),
                stop: Mutex::new(None),
                version: Mutex::new("0.165.0".to_owned()),
                stderr: Mutex::new(String::new()),
                exit_on_initialize: Mutex::new(false),
                initialize_delay: Mutex::new(Duration::ZERO),
                process_exit: Mutex::new(None),
            }),
            handler: Arc::new(handler),
        }
    }

    /// A fake that answers `{}` to everything it is not asked about by the standard replies.
    pub fn plain() -> Self {
        Self::new(|_, _| Ok(json!({})))
    }
}

#[async_trait]
impl Launcher for FakeLauncher {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        self.state.launches.fetch_add(1, Ordering::SeqCst);
        self.state.requests.lock().unwrap().push(request.clone());
        let (adapter_writes, harness_reads) = duplex(1 << 20);
        let (harness_writes, adapter_reads) = duplex(1 << 20);
        let (inject, mut injected) = mpsc::unbounded_channel::<Json>();
        *self.state.inject.lock().unwrap() = Some(inject);
        let (raw_inject, mut raw_injected) = mpsc::unbounded_channel::<String>();
        *self.state.raw.lock().unwrap() = Some(raw_inject);
        let (done_sender, done) = tokio::sync::watch::channel(false);
        *self.state.process_exit.lock().unwrap() = Some(done_sender.clone());
        let stop = Arc::new(Notify::new());
        *self.state.stop.lock().unwrap() = Some(Arc::clone(&stop));
        let state = Arc::clone(&self.state);
        let handler = Arc::clone(&self.handler);
        let task_stop = Arc::clone(&stop);
        let home = request.env.get("CODEX_HOME").cloned().unwrap_or_default();
        tokio::spawn(async move {
            let mut lines = BufReader::new(harness_reads).lines();
            let mut out = harness_writes;
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let frame: Json = serde_json::from_str(&line).unwrap();
                        state.received.lock().unwrap().push(frame.clone());
                        let (Some(id), Some(method)) = (frame.get("id"), frame.get("method").and_then(Json::as_str)) else {
                            continue;
                        };
                        if method == "initialize" && *state.exit_on_initialize.lock().unwrap() {
                            break;
                        }
                        if method == "initialize" {
                            let delay = *state.initialize_delay.lock().unwrap();
                            if !delay.is_zero() {
                                tokio::select! {
                                    () = tokio::time::sleep(delay) => {}
                                    () = task_stop.notified() => break,
                                }
                            }
                        }
                        let answer = match method {
                            "initialize" => Ok(json!({
                                "userAgent": format!("orch/{} (Fake OS 1.0; x86_64) unknown (orch; 0.1.0)", state.version.lock().unwrap()),
                                "codexHome": home, "platformFamily": "unix", "platformOs": "linux"
                            })),
                            "getAuthStatus" => handler(method, frame.get("params").unwrap_or(&Json::Null))
                                .or_else(|_| Ok(json!({ "authMethod": null, "authToken": null, "requiresOpenaiAuth": true }))),
                            "config/read" => handler(method, frame.get("params").unwrap_or(&Json::Null))
                                .ok()
                                .filter(|read| read.get("config").is_some())
                                .map_or_else(|| Ok(json!({ "config": { "model_provider": null } })), Ok),
                            other => handler(other, frame.get("params").unwrap_or(&Json::Null)),
                        };
                        let reply = match answer {
                            Ok(result) => json!({ "id": id, "result": result }),
                            Err((code, message)) => json!({ "id": id, "error": { "code": code, "message": message } }),
                        };
                        if out.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    frame = injected.recv() => {
                        let Some(frame) = frame else { break };
                        if out.write_all(format!("{frame}\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    text = raw_injected.recv() => {
                        let Some(text) = text else { break };
                        if out.write_all(text.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    () = task_stop.notified() => break,
                }
            }
            drop(out);
            done_sender.send_replace(true);
        });
        let said = self.state.stderr.lock().unwrap().clone();
        Ok(HarnessProcess {
            stdin: Box::new(adapter_writes),
            stdout: Box::new(adapter_reads),
            stderr: Some(Box::new(std::io::Cursor::new(said.into_bytes()))),
            control: Box::new(FakeControl {
                done,
                stop,
                state: Arc::clone(&self.state),
            }),
        })
    }
}
