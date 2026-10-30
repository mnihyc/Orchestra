//! A watched tab's pictures and input, with link adaptation and a sharp still image.

use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::{BufMut, Bytes, BytesMut};
use orch_link_contract::tunnels::{ExecMessage, ExecOpcode};
use orch_realtime_contract::browser::*;
use orch_types::Json;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::browser_cdp::CdpError;
use crate::browsers::{RunningBrowser, viewport};
use crate::files::Files;

/// Two browser frames in flight bound how much a slow link lets the browser draw ahead.
const IN_FLIGHT: u32 = 2;
/// Metadata bursts fit beside two pictures, while input remains a separate queue.
const OUTPUT_MESSAGES: usize = 16;
/// A burst of ordered keyboard and pointer input fits without unbounded buffering.
const INPUT_MESSAGES: usize = 64;
/// A still page is redrawn sharp after half a second.
const STILL_WAIT: Duration = Duration::from_millis(500);
/// Timers notice a still page or an expired dialog within a tenth of a second.
const TIMER_TICK: Duration = Duration::from_millis(100);
/// Link pressure is sampled once a second.
const QUALITY_TICK: Duration = Duration::from_secs(1);
/// A question with no answer is dismissed after a minute.
const DIALOG_WAIT: Duration = Duration::from_secs(60);
/// Ten empty samples avoid quality oscillation on a link near its limit.
const CLEAR_SAMPLES: u32 = 10;
/// The largest dimension allowed on a picture is that of the page itself.
const FULL_SCALE: u16 = 100;
/// The initial level balances clarity with a busy link.
const INITIAL_LEVEL: usize = 1;
/// Only five levels are used, in the order of increasing link pressure.
const LEVELS: [(u16, u16, u16); 5] = [
    (80, 100, 1),
    (65, 100, 1),
    (50, 75, 1),
    (40, 50, 1),
    (30, 50, 2),
];

/// The link's waiting bytes and the bytes handed on during the latest sample.
#[derive(Default)]
pub struct LinkLoad {
    waiting: AtomicU64,
    sent: AtomicU64,
}

impl LinkLoad {
    /// Notes bytes handed to the transport after its back-pressure allowed them through.
    pub fn handed(&self, bytes: u64) {
        let _ = self
            .waiting
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(bytes))
            });
        self.sent.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// A deterministic controller driven by the link's real queue, never by a viewer's drawing speed.
#[derive(Debug)]
pub struct Quality {
    level: usize,
    clear: u32,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            level: INITIAL_LEVEL,
            clear: 0,
        }
    }
}

impl Quality {
    /// The current level.
    #[must_use]
    pub fn level(&self) -> u16 {
        u16::try_from(self.level).unwrap_or_default()
    }

    /// Samples one second of transport progress and returns a changed level.
    pub fn sample(&mut self, waiting: u64, sent: u64) -> Option<u16> {
        let before = self.level;
        if waiting > sent {
            self.level = (self.level + 1).min(LEVELS.len() - 1);
            self.clear = 0;
        } else if waiting == 0 {
            self.clear += 1;
            if self.clear >= CLEAR_SAMPLES {
                self.level = self.level.saturating_sub(1);
                self.clear = 0;
            }
        } else {
            self.clear = 0;
        }
        (before != self.level).then(|| self.level())
    }
}

/// An aggregated watch's authority is set by the API, never by a socket's caller.
pub struct BrowserInput {
    /// The public frame after the server's permission checks.
    pub frame: BrowserClientFrame,
    /// Whether a holder of a control ticket watches that tab.
    pub control_watch: bool,
}

/// One tunnel message and, for a screencast, the acknowledgement due after transport progress.
pub struct BrowserOutput {
    /// The tab to which it belongs, where it belongs to one.
    pub tab: Option<String>,
    /// A live JSON control frame or a JPEG's public binary frame.
    pub message: ExecMessage,
    ack: Option<(String, i64)>,
}

impl BrowserOutput {
    /// Encodes a tab-tagged message inside the tunnel's existing message framing.
    pub fn wire(&self) -> Result<Bytes, CdpError> {
        let payload = match self.message.opcode {
            ExecOpcode::Text => {
                let frame: Json = serde_json::from_slice(&self.message.payload)
                    .map_err(|_| CdpError::Protocol("frame".to_owned()))?;
                Bytes::from(json!({"tab":self.tab,"frame":frame}).to_string())
            }
            ExecOpcode::Binary => {
                let tab = self.tab.as_deref().unwrap_or_default().as_bytes();
                let len = u16::try_from(tab.len())
                    .map_err(|_| CdpError::Protocol("tab identifier".to_owned()))?;
                let mut payload = BytesMut::new();
                payload.put_u16(len);
                payload.extend_from_slice(tab);
                payload.extend_from_slice(&self.message.payload);
                payload.freeze()
            }
        };
        let mut wire = BytesMut::new();
        ExecMessage {
            opcode: self.message.opcode,
            payload,
        }
        .write(&mut wire)
        .map_err(|_| CdpError::Protocol("frame".to_owned()))?;
        Ok(wire.freeze())
    }
}

/// A shared browser tunnel's input, output and real transport measurements.
pub struct BrowserStream {
    /// Ordered checked input.
    pub input: mpsc::Sender<BrowserInput>,
    /// Live browser output.
    pub output: mpsc::Receiver<BrowserOutput>,
    /// The transport's queue measurements.
    pub load: Arc<LinkLoad>,
    browser: Arc<RunningBrowser>,
}

impl BrowserStream {
    /// Begins a live view; opening this does not make a picture before a tab is watched.
    pub fn start(browser: Arc<RunningBrowser>, files: Files) -> Self {
        let (input, rx) = mpsc::channel(INPUT_MESSAGES);
        let (output, output_rx) = mpsc::channel(OUTPUT_MESSAGES);
        let load = Arc::new(LinkLoad::default());
        let screen = Screen {
            browser: Arc::clone(&browser),
            files,
            output,
            load: Arc::clone(&load),
            sessions: BTreeMap::new(),
            watches: BTreeMap::new(),
            choosers: BTreeMap::new(),
            dialogs: BTreeMap::new(),
            loading: BTreeMap::new(),
            active: None,
            quality: Quality::default(),
            number: 0,
            size: browser.viewport,
        };
        tokio::spawn(screen.run(rx));
        Self {
            input,
            output: output_rx,
            load,
            browser,
        }
    }

    /// Acknowledges a browser frame only after it was handed to the tunnel.
    pub async fn handed(&self, output: &BrowserOutput, bytes: u64) {
        self.load.handed(bytes);
        if let Some((session, id)) = &output.ack {
            let _ = self
                .browser
                .cdp
                .call(
                    "Page.screencastFrameAck",
                    json!({"sessionId":id}),
                    Some(session),
                )
                .await;
        }
    }
}

struct Watch {
    width: u16,
    height: u16,
    control: bool,
    last_frame: Instant,
    last_level: u16,
    clear_sent: bool,
    last_hash: Option<orch_types::Hash>,
    last_css: (u16, u16),
}

struct Screen {
    browser: Arc<RunningBrowser>,
    files: Files,
    output: mpsc::Sender<BrowserOutput>,
    load: Arc<LinkLoad>,
    sessions: BTreeMap<String, String>,
    watches: BTreeMap<String, Watch>,
    choosers: BTreeMap<String, i64>,
    dialogs: BTreeMap<String, Instant>,
    loading: BTreeMap<String, std::collections::BTreeSet<String>>,
    active: Option<String>,
    quality: Quality,
    number: u32,
    size: (u16, u16),
}

impl Screen {
    async fn send(&self, tab: Option<&str>, frame: BrowserServerFrame) {
        let Ok(value) = serde_json::to_vec(&frame) else {
            return;
        };
        let message = BrowserOutput {
            tab: tab.map(str::to_owned),
            message: ExecMessage {
                opcode: ExecOpcode::Text,
                payload: Bytes::from(value),
            },
            ack: None,
        };
        if let Ok(wire) = message.wire() {
            self.load
                .waiting
                .fetch_add(wire.len() as u64, Ordering::Relaxed);
        }
        let _ = self.output.send(message).await;
    }

    async fn tabs(&self) -> Result<Vec<Tab>, CdpError> {
        let found = self
            .browser
            .cdp
            .call("Target.getTargets", json!({}), None)
            .await?;
        Ok(found["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .filter_map(|target| {
                Some(Tab {
                    id: target["targetId"].as_str()?.to_owned(),
                    title: target["title"].as_str().unwrap_or_default().to_owned(),
                    url: target["url"].as_str().unwrap_or_default().to_owned(),
                    loading: target["targetId"]
                        .as_str()
                        .and_then(|id| self.loading.get(id))
                        .is_some_and(|frames| !frames.is_empty()),
                    active: self.active.as_deref() == target["targetId"].as_str(),
                    opener: target["openerId"].as_str().map(str::to_owned),
                })
            })
            .collect())
    }

    async fn attach(&mut self, tab: &str) -> Result<String, CdpError> {
        if let Some(session) = self.sessions.get(tab) {
            return Ok(session.clone());
        }
        if !self.tabs().await?.iter().any(|t| t.id == tab) {
            return Err(CdpError::Protocol("no_such_tab".to_owned()));
        }
        let found = self
            .browser
            .cdp
            .call(
                "Target.attachToTarget",
                json!({"targetId":tab,"flatten":true}),
                None,
            )
            .await?;
        let session = found["sessionId"]
            .as_str()
            .ok_or_else(|| CdpError::Protocol("attach".to_owned()))?
            .to_owned();
        self.browser
            .cdp
            .call("Page.enable", json!({}), Some(&session))
            .await?;
        self.sessions.insert(tab.to_owned(), session.clone());
        Ok(session)
    }

    async fn casting(&self, tab: &str, send_last: bool) -> Result<(), CdpError> {
        let Some(watch) = self.watches.get(tab) else {
            return Ok(());
        };
        let Some(session) = self.sessions.get(tab) else {
            return Ok(());
        };
        let level = usize::from(self.quality.level());
        let (quality, scale, every) = LEVELS[level];
        let width = (u32::from(watch.width) * u32::from(scale) / u32::from(FULL_SCALE)).max(1);
        let height = (u32::from(watch.height) * u32::from(scale) / u32::from(FULL_SCALE)).max(1);
        let _ = self
            .browser
            .cdp
            .call("Page.stopScreencast", json!({}), Some(session))
            .await;
        self.browser.cdp.call("Page.startScreencast",json!({"format":"jpeg","quality":quality,"maxWidth":width,"maxHeight":height,"everyNthFrame":every,"maxFramesInFlight":IN_FLIGHT,"sendLastFrame":send_last}),Some(session)).await?;
        Ok(())
    }

    async fn run(mut self, mut input: mpsc::Receiver<BrowserInput>) {
        let mut events = self.browser.cdp.events();
        if let Ok(mut tabs) = self.tabs().await {
            self.active = tabs.first().map(|t| t.id.clone());
            if let Some(first) = tabs.first_mut() {
                first.active = true;
                if let Ok(session) = self.attach(&first.id).await
                    && let Ok(metrics) = self
                        .browser
                        .cdp
                        .call("Page.getLayoutMetrics", json!({}), Some(&session))
                        .await
                    && let Some(width) = metrics["cssLayoutViewport"]["clientWidth"]
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok())
                        .filter(|n| *n > 0)
                    && let Some(height) = metrics["cssLayoutViewport"]["clientHeight"]
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok())
                        .filter(|n| *n > 0)
                {
                    self.size = (width, height);
                }
            }
            self.send(
                None,
                BrowserServerFrame::Ready {
                    control: false,
                    tabs,
                    viewport: Viewport {
                        width: self.size.0,
                        height: self.size.1,
                    },
                    people: BrowserPeople {
                        watching: Vec::new(),
                        driving: Vec::new(),
                    },
                },
            )
            .await;
        }
        let mut timer = tokio::time::interval(TIMER_TICK);
        let mut quality_timer =
            tokio::time::interval_at(Instant::now() + QUALITY_TICK, QUALITY_TICK);
        loop {
            tokio::select! {
                incoming=input.recv()=>{let Some(incoming)=incoming else {break;};if let Err(error)=self.input(incoming).await {self.send(None,BrowserServerFrame::Error{code:if error.to_string().contains("no_such_tab") {BrowserSocketError::NoSuchTab}else{BrowserSocketError::Invalid},detail:error.to_string()}).await;}}
                event=events.recv()=>match event {Ok(event)=>{if event["method"]=="Orch.connectionClosed" {self.send(None,BrowserServerFrame::Closed{reason:BrowserClosedReason::Exited}).await;break;}self.event(event).await;},Err(broadcast::error::RecvError::Lagged(_))=>{},Err(_)=>break},
                _=quality_timer.tick()=>{
                    let waiting=self.load.waiting.load(Ordering::Relaxed);let sent=self.load.sent.swap(0,Ordering::Relaxed);
                    if let Some(level)=self.quality.sample(waiting,sent) {
                        self.send(None,BrowserServerFrame::Quality{level,why:BrowserQualityWhy::Link}).await;
                        let tabs:Vec<_>=self.watches.keys().filter(|tab|!self.dialogs.contains_key(*tab)).cloned().collect();for tab in tabs {let _=self.casting(&tab,false).await;}
                    }
                }
                _=timer.tick()=>{self.timers().await;}
            }
        }
        for session in self.sessions.values() {
            let _ = self
                .browser
                .cdp
                .call("Page.stopScreencast", json!({}), Some(session))
                .await;
            let _ = self
                .browser
                .cdp
                .call(
                    "Page.setInterceptFileChooserDialog",
                    json!({"enabled":false}),
                    Some(session),
                )
                .await;
        }
    }

    async fn input(&mut self, input: BrowserInput) -> Result<(), CdpError> {
        match input.frame {
            BrowserClientFrame::Watch {
                tab,
                width,
                height,
                dpr,
            } => {
                let session = self.attach(&tab).await?;
                if width == 0 || height == 0 {
                    self.watches.remove(&tab);
                    self.browser
                        .cdp
                        .call("Page.stopScreencast", json!({}), Some(&session))
                        .await?;
                    self.browser
                        .cdp
                        .call(
                            "Page.setInterceptFileChooserDialog",
                            json!({"enabled":false}),
                            Some(&session),
                        )
                        .await?;
                    return Ok(());
                }
                if !dpr.is_finite() || dpr <= 0.0 {
                    return Err(CdpError::Protocol("watch size".to_owned()));
                }
                let width = ((f64::from(width) * dpr).min(f64::from(self.size.0))).round() as u16;
                let height = ((f64::from(height) * dpr).min(f64::from(self.size.1))).round() as u16;
                self.watches.insert(
                    tab.clone(),
                    Watch {
                        width,
                        height,
                        control: input.control_watch,
                        last_frame: Instant::now(),
                        last_level: self.quality.level(),
                        clear_sent: false,
                        last_hash: None,
                        last_css: self.size,
                    },
                );
                self.browser
                    .cdp
                    .call(
                        "Page.setInterceptFileChooserDialog",
                        json!({"enabled":input.control_watch}),
                        Some(&session),
                    )
                    .await?;
                self.casting(&tab, true).await?;
            }
            BrowserClientFrame::Ack { .. } | BrowserClientFrame::Ping { .. } => {}
            BrowserClientFrame::Pointer {
                tab,
                kind,
                x,
                y,
                button,
                buttons,
                clicks,
                dx,
                dy,
                modifiers,
            } => {
                if ![x, y, dx.unwrap_or_default(), dy.unwrap_or_default()]
                    .iter()
                    .all(|n| n.is_finite())
                {
                    return Err(CdpError::Protocol("pointer".to_owned()));
                }
                let session = self.attach(&tab).await?;
                let kind = match kind {
                    PointerKind::Move => "mouseMoved",
                    PointerKind::Down => "mousePressed",
                    PointerKind::Up => "mouseReleased",
                    PointerKind::Wheel => "mouseWheel",
                };
                let button = match button {
                    Some(PointerButton::Left) => "left",
                    Some(PointerButton::Middle) => "middle",
                    Some(PointerButton::Right) => "right",
                    None => "none",
                };
                self.browser.cdp.call("Input.dispatchMouseEvent",json!({"type":kind,"x":x,"y":y,"button":button,"buttons":buttons.unwrap_or_default(),"clickCount":clicks.unwrap_or(1),"deltaX":dx.unwrap_or_default(),"deltaY":dy.unwrap_or_default(),"modifiers":modifiers.unwrap_or_default()}),Some(&session)).await?;
            }
            BrowserClientFrame::Key {
                tab,
                kind,
                key,
                code,
                text,
                modifiers,
                repeat,
                location,
            } => {
                let modifier_mask = modifiers.unwrap_or_default();
                let inserted = text.unwrap_or_else(|| {
                    if kind == KeyKind::Down && key.chars().count() == 1 && modifier_mask & 7 == 0 {
                        key.clone()
                    } else {
                        String::new()
                    }
                });
                let virtual_key = virtual_key(&key);
                let session = self.attach(&tab).await?;
                self.browser.cdp.call("Input.dispatchKeyEvent",json!({"type":match kind {KeyKind::Down=>"keyDown",KeyKind::Up=>"keyUp"},"key":key,"code":code,"text":inserted,"windowsVirtualKeyCode":virtual_key,"modifiers":modifier_mask,"autoRepeat":repeat.unwrap_or_default(),"location":location.unwrap_or_default()}),Some(&session)).await?;
            }
            BrowserClientFrame::Text { tab, text } => {
                let session = self.attach(&tab).await?;
                self.browser
                    .cdp
                    .call("Input.insertText", json!({"text":text}), Some(&session))
                    .await?;
            }
            BrowserClientFrame::Navigate { tab, url } => {
                self.navigation(&url)?;
                let session = self.attach(&tab).await?;
                self.browser
                    .cdp
                    .call("Page.navigate", json!({"url":url}), Some(&session))
                    .await?;
            }
            BrowserClientFrame::History { tab, to } => {
                let session = self.attach(&tab).await?;
                match to {
                    BrowserHistoryTo::Reload => {
                        self.browser
                            .cdp
                            .call("Page.reload", json!({}), Some(&session))
                            .await?;
                    }
                    BrowserHistoryTo::Stop => {
                        self.browser
                            .cdp
                            .call("Page.stopLoading", json!({}), Some(&session))
                            .await?;
                    }
                    BrowserHistoryTo::Back | BrowserHistoryTo::Forward => {
                        let history = self
                            .browser
                            .cdp
                            .call("Page.getNavigationHistory", json!({}), Some(&session))
                            .await?;
                        let index = history["currentIndex"].as_i64().unwrap_or_default()
                            + if to == BrowserHistoryTo::Back { -1 } else { 1 };
                        if let Some(entry) = usize::try_from(index)
                            .ok()
                            .and_then(|i| history["entries"].as_array()?.get(i))
                        {
                            self.browser
                                .cdp
                                .call(
                                    "Page.navigateToHistoryEntry",
                                    json!({"entryId":entry["id"]}),
                                    Some(&session),
                                )
                                .await?;
                        }
                    }
                }
            }
            BrowserClientFrame::Tab { action, tab, url } => match action {
                BrowserTabAction::New => {
                    let url = url.unwrap_or_else(|| "about:blank".to_owned());
                    self.navigation(&url)?;
                    let target = self
                        .browser
                        .cdp
                        .call(
                            "Target.createTarget",
                            // Tab targets inherit their window; explicit bounds are valid only for a new window.
                            json!({"url":url}),
                            None,
                        )
                        .await?;
                    self.active = target["targetId"].as_str().map(str::to_owned);
                }
                BrowserTabAction::Close => {
                    let tab = tab.ok_or_else(|| CdpError::Protocol("tab".to_owned()))?;
                    self.browser
                        .cdp
                        .call("Target.closeTarget", json!({"targetId":tab}), None)
                        .await?;
                }
                BrowserTabAction::Activate => {
                    let tab = tab.ok_or_else(|| CdpError::Protocol("tab".to_owned()))?;
                    self.browser
                        .cdp
                        .call("Target.activateTarget", json!({"targetId":tab}), None)
                        .await?;
                    self.active = Some(tab);
                }
            },
            BrowserClientFrame::Dialog { tab, accept, text } => {
                let session = self.attach(&tab).await?;
                self.browser
                    .cdp
                    .call(
                        "Page.handleJavaScriptDialog",
                        json!({"accept":accept,"promptText":text.unwrap_or_default()}),
                        Some(&session),
                    )
                    .await?;
                self.dialogs.remove(&tab);
            }
            BrowserClientFrame::Files { tab, paths } => {
                let paths = self.file_paths(&paths.unwrap_or_default())?;
                let session = self.attach(&tab).await?;
                let backend = self
                    .choosers
                    .remove(&tab)
                    .ok_or_else(|| CdpError::Protocol("file chooser".to_owned()))?;
                self.browser
                    .cdp
                    .call(
                        "DOM.setFileInputFiles",
                        json!({"files":paths,"backendNodeId":backend}),
                        Some(&session),
                    )
                    .await?;
            }
            BrowserClientFrame::Viewport { width, height } => {
                viewport(&format!("{width}x{height}"))
                    .map_err(|_| CdpError::Protocol("viewport".to_owned()))?;
                self.size = (width, height);
                for tab in self.tabs().await? {
                    self.attach(&tab.id).await?;
                }
                for session in self.sessions.values() {
                    self.browser.cdp.call("Emulation.setDeviceMetricsOverride",json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),Some(session)).await?;
                }
                for tab in self.sessions.keys() {
                    if let Ok(window) = self
                        .browser
                        .cdp
                        .call("Browser.getWindowForTarget", json!({"targetId":tab}), None)
                        .await
                    {
                        let _=self.browser.cdp.call("Browser.setWindowBounds",json!({"windowId":window["windowId"],"bounds":{"width":width,"height":height}}),None).await;
                    }
                }
                self.send(
                    None,
                    BrowserServerFrame::Ready {
                        control: false,
                        tabs: self.tabs().await?,
                        viewport: Viewport { width, height },
                        people: BrowserPeople {
                            watching: Vec::new(),
                            driving: Vec::new(),
                        },
                    },
                )
                .await;
            }
            BrowserClientFrame::Copy { tab } => {
                let session = self.attach(&tab).await?;
                let result=self.browser.cdp.call("Runtime.evaluate",json!({"expression":"window.getSelection()?.toString() || ''","returnByValue":true}),Some(&session)).await?;
                self.send(
                    Some(&tab),
                    BrowserServerFrame::Copied {
                        text: result["result"]["value"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                    },
                )
                .await;
            }
        }
        Ok(())
    }

    fn file_paths(&self, paths: &[String]) -> Result<Vec<String>, CdpError> {
        paths
            .iter()
            .map(|path| {
                let relative = self
                    .files
                    .parse(path)
                    .map_err(|_| CdpError::Protocol("workspace path".to_owned()))?;
                self.files
                    .open_regular(&relative)
                    .map_err(|_| CdpError::Protocol("workspace file".to_owned()))?;
                let located = self
                    .files
                    .root()
                    .locate(&relative, crate::paths::Locate::THROUGH)
                    .map_err(|_| CdpError::Protocol("workspace path".to_owned()))?;
                Ok(located
                    .real
                    .under(self.files.root().root_path())
                    .to_string_lossy()
                    .into_owned())
            })
            .collect()
    }

    fn navigation(&self, url: &str) -> Result<(), CdpError> {
        if let Ok(parsed) = reqwest::Url::parse(url)
            && parsed.scheme() == "file"
        {
            let path = parsed
                .to_file_path()
                .map_err(|_| CdpError::Protocol("file path".to_owned()))?;
            let path = path
                .strip_prefix(self.files.root().root_path())
                .map_err(|_| CdpError::Protocol("workspace path".to_owned()))?;
            let relative = self
                .files
                .parse(&path.to_string_lossy())
                .map_err(|_| CdpError::Protocol("workspace path".to_owned()))?;
            self.files
                .root()
                .locate(&relative, crate::paths::Locate::THROUGH)
                .map_err(|_| CdpError::Protocol("workspace path".to_owned()))?;
        }
        Ok(())
    }

    async fn event(&mut self, event: Json) {
        let method = event["method"].as_str().unwrap_or_default();
        let tab = event["sessionId"].as_str().and_then(|s| {
            self.sessions
                .iter()
                .find_map(|(tab, session)| (session == s).then(|| tab.clone()))
        });
        let params = &event["params"];
        match method {
            "Page.screencastFrame" => {
                let Some(tab) = tab else {
                    return;
                };
                let Some(session) = self.sessions.get(&tab).cloned() else {
                    return;
                };
                let Some(watch) = self.watches.get_mut(&tab) else {
                    let _ = self
                        .browser
                        .cdp
                        .call(
                            "Page.screencastFrameAck",
                            json!({"sessionId":params["sessionId"]}),
                            Some(&session),
                        )
                        .await;
                    return;
                };
                let Ok(jpeg) = STANDARD.decode(params["data"].as_str().unwrap_or_default()) else {
                    return;
                };
                let hash = orch_types::Hash::of(&jpeg);
                if watch.last_hash == Some(hash) {
                    let _ = self
                        .browser
                        .cdp
                        .call(
                            "Page.screencastFrameAck",
                            json!({"sessionId":params["sessionId"]}),
                            Some(&session),
                        )
                        .await;
                    return;
                }
                watch.last_hash = Some(hash);
                watch.last_frame = Instant::now();
                watch.last_level = self.quality.level();
                watch.clear_sent = false;
                let css = (
                    params["metadata"]["deviceWidth"]
                        .as_f64()
                        .unwrap_or(f64::from(self.size.0)) as u16,
                    params["metadata"]["deviceHeight"]
                        .as_f64()
                        .unwrap_or(f64::from(self.size.1)) as u16,
                );
                watch.last_css = css;
                self.picture(
                    &tab,
                    jpeg,
                    css,
                    Some((session, params["sessionId"].as_i64().unwrap_or_default())),
                )
                .await;
            }
            "Page.javascriptDialogOpening" => {
                if let Some(tab) = tab
                    && self.watches.get(&tab).is_some_and(|w| w.control)
                {
                    self.dialogs
                        .insert(tab.clone(), Instant::now() + DIALOG_WAIT);
                    let kind = serde_json::from_value(params["type"].clone())
                        .unwrap_or(BrowserDialogKind::Alert);
                    self.send(
                        Some(&tab),
                        BrowserServerFrame::Dialog {
                            tab: tab.clone(),
                            kind,
                            message: params["message"].as_str().unwrap_or_default().to_owned(),
                            default: params["defaultPrompt"].as_str().map(str::to_owned),
                        },
                    )
                    .await;
                }
            }
            "Page.javascriptDialogClosed" => {
                if let Some(tab) = tab {
                    self.dialogs.remove(&tab);
                    let _ = self.casting(&tab, false).await;
                    self.send(
                        Some(&tab),
                        BrowserServerFrame::DialogClosed { tab: tab.clone() },
                    )
                    .await;
                }
            }
            "Page.fileChooserOpened" => {
                if let Some(tab) = tab
                    && self.watches.get(&tab).is_some_and(|w| w.control)
                {
                    if let Some(node) = params["backendNodeId"].as_i64() {
                        self.choosers.insert(tab.clone(), node);
                    }
                    self.send(
                        Some(&tab),
                        BrowserServerFrame::Chooser {
                            tab: tab.clone(),
                            multiple: params["mode"] == "selectMultiple",
                        },
                    )
                    .await;
                }
            }
            "Target.targetDestroyed" => {
                if let Some(tab) = params["targetId"].as_str() {
                    self.sessions.remove(tab);
                    self.watches.remove(tab);
                    self.loading.remove(tab);
                }
                if let Ok(tabs) = self.tabs().await {
                    self.send(None, BrowserServerFrame::Tabs { tabs }).await;
                }
            }
            "Target.targetCreated"
            | "Target.targetInfoChanged"
            | "Page.frameStartedLoading"
            | "Page.frameStoppedLoading"
            | "Page.frameDetached" => {
                if let Some(tab) = tab
                    && let Some(frame) = params["frameId"].as_str()
                {
                    if method == "Page.frameStartedLoading" {
                        self.loading
                            .entry(tab)
                            .or_default()
                            .insert(frame.to_owned());
                    } else if matches!(method, "Page.frameStoppedLoading" | "Page.frameDetached")
                        && let Some(frames) = self.loading.get_mut(&tab)
                    {
                        frames.remove(frame);
                    }
                }
                if let Ok(tabs) = self.tabs().await {
                    self.send(None, BrowserServerFrame::Tabs { tabs }).await;
                }
            }
            _ => {}
        }
    }

    async fn picture(
        &mut self,
        tab: &str,
        jpeg: Vec<u8>,
        css: (u16, u16),
        ack: Option<(String, i64)>,
    ) {
        let Some(image) = jpeg_size(&jpeg) else {
            return;
        };
        self.number = self.number.wrapping_add(1);
        let header = PictureHeader {
            n: self.number,
            viewport: Viewport {
                width: css.0,
                height: css.1,
            },
            image,
        };
        let mut bytes = BytesMut::new();
        bytes.extend_from_slice(&header.encode());
        bytes.extend_from_slice(&jpeg);
        let output = BrowserOutput {
            tab: Some(tab.to_owned()),
            message: ExecMessage {
                opcode: ExecOpcode::Binary,
                payload: bytes.freeze(),
            },
            ack,
        };
        if let Ok(wire) = output.wire() {
            self.load
                .waiting
                .fetch_add(wire.len() as u64, Ordering::Relaxed);
        }
        let _ = self.output.send(output).await;
    }

    async fn timers(&mut self) {
        let now = Instant::now();
        let expired: Vec<_> = self
            .dialogs
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(tab, _)| tab.clone())
            .collect();
        for tab in expired {
            self.dialogs.remove(&tab);
            if let Some(session) = self.sessions.get(&tab) {
                let _ = self
                    .browser
                    .cdp
                    .call(
                        "Page.handleJavaScriptDialog",
                        json!({"accept":false}),
                        Some(session),
                    )
                    .await;
            }
        }
        let still: Vec<_> = self
            .watches
            .iter()
            .filter(|(_, w)| {
                !w.clear_sent && w.last_level != 0 && now.duration_since(w.last_frame) >= STILL_WAIT
            })
            .filter(|(tab, _)| !self.dialogs.contains_key(*tab))
            .filter(|_| self.load.waiting.load(Ordering::Relaxed) == 0)
            .map(|(tab, _)| tab.clone())
            .collect();
        for tab in still {
            if let Some(watch) = self.watches.get_mut(&tab) {
                watch.clear_sent = true;
            }
            let Some(session) = self.sessions.get(&tab) else {
                continue;
            };
            let Some(watch) = self.watches.get(&tab) else {
                continue;
            };
            let css = watch.last_css;
            let scale = (f64::from(watch.width) / f64::from(css.0))
                .min(f64::from(watch.height) / f64::from(css.1))
                .min(1.0);
            if let Ok(frame) = self
                .browser
                .cdp
                .call(
                    "Page.captureScreenshot",
                    json!({"format":"jpeg","quality":LEVELS[0].0,"captureBeyondViewport":false,"clip":{"x":0,"y":0,"width":css.0,"height":css.1,"scale":scale}}),
                    Some(session),
                )
                .await
                && let Ok(jpeg) = STANDARD.decode(frame["data"].as_str().unwrap_or_default())
            {
                self.send(
                    Some(&tab),
                    BrowserServerFrame::Quality {
                        level: 0,
                        why: BrowserQualityWhy::Clear,
                    },
                )
                .await;
                self.picture(&tab, jpeg, css, None).await;
            }
        }
    }
}

/// Reads JPEG start-of-frame dimensions without decoding or altering the browser's picture.
#[must_use]
pub fn jpeg_size(jpeg: &[u8]) -> Option<Viewport> {
    if jpeg.get(..2) != Some(&[0xff, 0xd8]) {
        return None;
    }
    let mut at = 2;
    while at + 4 <= jpeg.len() {
        if jpeg[at] != 0xff {
            return None;
        }
        let marker = jpeg[at + 1];
        at += 2;
        if marker == 0xff {
            at -= 1;
            continue;
        }
        if matches!(marker, 0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7) {
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*jpeg.get(at)?, *jpeg.get(at + 1)?]));
        if len < 2 || at + len > jpeg.len() {
            return None;
        }
        if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) {
            return Some(Viewport {
                height: u16::from_be_bytes([*jpeg.get(at + 3)?, *jpeg.get(at + 4)?]),
                width: u16::from_be_bytes([*jpeg.get(at + 5)?, *jpeg.get(at + 6)?]),
            });
        }
        at += len;
    }
    None
}

// Standard Windows virtual key codes are the key identifiers the browser's protocol expects.
fn virtual_key(key: &str) -> u32 {
    const SPECIAL: &[(&str, u32)] = &[
        ("Backspace", 8),
        ("Tab", 9),
        ("Enter", 13),
        ("Escape", 27),
        (" ", 32),
        ("PageUp", 33),
        ("PageDown", 34),
        ("End", 35),
        ("Home", 36),
        ("ArrowLeft", 37),
        ("ArrowUp", 38),
        ("ArrowRight", 39),
        ("ArrowDown", 40),
        ("Delete", 46),
    ];
    SPECIAL
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, code)| *code)
        .unwrap_or_else(|| {
            if key.len() == 1 && key.is_ascii() {
                u32::from(key.as_bytes()[0].to_ascii_uppercase())
            } else {
                0
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_busy_link_falls_a_level_per_second_and_takes_ten_clear_seconds_to_rise() {
        let mut quality = Quality::default();
        assert_eq!(quality.sample(101, 100), Some(2));
        assert_eq!(quality.sample(101, 100), Some(3));
        assert_eq!(quality.sample(1, 0), Some(4));
        for _ in 0..9 {
            assert_eq!(quality.sample(0, 100), None);
        }
        assert_eq!(quality.sample(0, 100), Some(3));
        assert_eq!(quality.sample(100, 100), None);
        for _ in 0..9 {
            assert_eq!(quality.sample(0, 100), None);
        }
        assert_eq!(quality.sample(0, 100), Some(2));
    }
}
