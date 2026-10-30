//! Browser sockets share one live tunnel and let each viewer draw only its newest picture.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{
    Arc, Mutex, PoisonError, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Duration as Span, Utc};
use futures::SinkExt;
use orch_access_contract::permissions::Permission;
use orch_api_contract::browsers::{
    BROWSER_TICKET_SECONDS, BrowserAttachQuery, BrowserTicketCreateReply,
};
use orch_db::queries::browsers as rows;
use orch_engine::browsers::{BrowserActions, SITTING_GAP};
use orch_link::tunnels::channel::Tunnel;
use orch_link_contract::tunnels::{ExecMessage, ExecOpcode};
use orch_realtime_contract::browser::*;
use orch_realtime_contract::feeds::Feed;
use orch_types::{Id, Json, Time};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use crate::middleware::authentication::{new_token, token_hash};
use crate::middleware::authorization::RetainedIdentity;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::browsers as views;
use crate::state::{App, AppState};

/// A bounded maximum viewport JPEG, refused before tunnel allocation.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// Control JSON is much smaller than a picture and must be bounded separately.
const MAX_INPUT: usize = 256 * 1024;
/// Metadata bursts fit without keeping a page's history.
const META_QUEUE: usize = 32;
/// Ordered pointer/key bursts wait here without growing forever.
const INPUT_QUEUE: usize = 64;
/// The protocol marks a person as driving for ten seconds after their last input.
const DRIVING: Span = Span::seconds(10);
/// Gates, idleness and sitting expiry are inspected once a second.
const KEEPER_TICK: Duration = Duration::from_secs(1);
/// A live tunnel remains briefly after the last viewer, matching other unretained watches.
const TUNNEL_GRACE: Span = Span::seconds(10);
/// A local browser's initial tab list gets room for slow gateway setup.
const READY_WAIT: Duration = Duration::from_secs(40);
/// Small reads carry input promptly without coupling viewer acknowledgements to the environment's link.
const READ_BYTES: usize = 16 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
struct Ticket {
    browser: Id,
    user: Id,
    identity: RetainedIdentity,
    control: bool,
    expires: DateTime<Utc>,
}

#[derive(Clone)]
struct Watch {
    tab: String,
    width: u16,
    height: u16,
    dpr: f64,
}

struct Viewer {
    user: Id,
    watch: Option<Watch>,
    control: bool,
    metadata: mpsc::Sender<BrowserServerFrame>,
    picture: watch::Sender<Option<Bytes>>,
}

#[derive(Clone)]
struct Ready {
    tabs: Vec<Tab>,
    viewport: Viewport,
}

struct GroupState {
    ready: Option<Ready>,
    viewers: BTreeMap<u64, Viewer>,
    driver_at: BTreeMap<Id, DateTime<Utc>>,
    published_driving: BTreeSet<Id>,
    empty_at: DateTime<Utc>,
}

struct Group {
    id: Id,
    environment: Id,
    outgoing: mpsc::Sender<Json>,
    ready: watch::Sender<Option<Ready>>,
    state: Mutex<GroupState>,
    alive: AtomicBool,
    stop: tokio_util::sync::CancellationToken,
}

struct Sitting {
    from: DateTime<Utc>,
    last: DateTime<Utc>,
    hosts: BTreeSet<String>,
    tabs: BTreeSet<String>,
}

/// Per-process tickets, live viewers and unretained sitting metadata.
#[derive(Clone, Default)]
pub struct Sockets {
    hub: Arc<Mutex<Option<orch_link::tunnels::terminal::TerminalHub>>>,
    tickets: Arc<Mutex<HashMap<Vec<u8>, Ticket>>>,
    groups: Arc<tokio::sync::Mutex<BTreeMap<Id, Arc<Group>>>>,
    sittings: Arc<Mutex<BTreeMap<(Id, Id), Sitting>>>,
    numbers: Arc<AtomicU64>,
    installed: Arc<AtomicBool>,
    recovery_started: Arc<AtomicBool>,
    keeper_started: Arc<AtomicBool>,
}

impl std::fmt::Debug for Sockets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserSockets").finish_non_exhaustive()
    }
}

impl Sockets {
    fn start_keeper(&self, app: &App) {
        if self.keeper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let sockets = self.clone();
        let weak = Arc::downgrade(app);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(KEEPER_TICK);
            loop {
                tick.tick().await;
                let Some(app) = weak.upgrade() else { break };
                if app.realtime.inner.stop.is_cancelled() {
                    break;
                }
                sockets.keep(&app).await;
            }
        });
    }

    async fn ending(app: &App, browser: Id) -> BrowserClosedReason {
        let Ok(mut conn) = app.db.acquire().await else {
            return BrowserClosedReason::LinkLost;
        };
        let Ok(Some(row)) = rows::get(&mut conn, browser).await else {
            return BrowserClosedReason::EnvironmentStopped;
        };
        if row.state == "closed" {
            return match row.close_reason.as_deref() {
                Some("quit") => BrowserClosedReason::Quit,
                Some("closed_by_agent") => BrowserClosedReason::ClosedByAgent,
                Some("idle") => BrowserClosedReason::Idle,
                Some("environment_stopped") => BrowserClosedReason::EnvironmentStopped,
                Some("switched_off") => BrowserClosedReason::SwitchedOff,
                Some("exited") => BrowserClosedReason::Exited,
                _ => BrowserClosedReason::LinkLost,
            };
        }
        let Ok(Some(site)) = rows::site(&mut conn, row.environment_id).await else {
            return BrowserClosedReason::EnvironmentStopped;
        };
        if row.env_generation != site.generation
            || !matches!(site.status.as_str(), "running" | "offline")
        {
            return BrowserClosedReason::EnvironmentStopped;
        }
        if let Ok(Some(off)) = orch_engine::browsers::Browsers::off(&mut conn, &site).await {
            return if off == orch_api_contract::browsers::BrowsersOff::EnvironmentOffline {
                BrowserClosedReason::LinkLost
            } else {
                BrowserClosedReason::SwitchedOff
            };
        }
        BrowserClosedReason::Revoked
    }
    /// Gives tests a machine hub while production uses the application's gateway backplane.
    pub fn through(&self, hub: orch_link::tunnels::terminal::TerminalHub) {
        *lock(&self.hub) = Some(hub);
    }
    /// Gives one person a short, single-use socket ticket with their original token restrictions.
    pub fn ticket(
        &self,
        ctx: &Ctx,
        browser: Id,
        control: bool,
    ) -> ApiResult<BrowserTicketCreateReply> {
        let token = new_token("bt_");
        let expires = ctx.now + Span::seconds(i64::from(BROWSER_TICKET_SECONDS));
        let authed = ctx.user()?;
        let mut tickets = lock(&self.tickets);
        tickets.retain(|_, t| t.expires > ctx.now);
        tickets.insert(
            token_hash(&token),
            Ticket {
                browser,
                user: authed.id(),
                identity: RetainedIdentity::of(authed),
                control,
                expires,
            },
        );
        Ok(BrowserTicketCreateReply {
            ticket: token,
            ticket_expires_at: Time::new(expires),
        })
    }

    fn redeem(&self, browser: Id, token: &str, now: DateTime<Utc>) -> Option<Ticket> {
        lock(&self.tickets)
            .remove(&token_hash(token))
            .filter(|t| t.browser == browser && t.expires > now)
    }

    /// Those who sent input recently, counted once even with several sockets.
    pub fn driving(&self, browser: Id, now: DateTime<Utc>) -> Vec<Id> {
        lock(&self.sittings)
            .iter()
            .filter(|((id, _), s)| *id == browser && now - s.last < DRIVING)
            .map(|((_, user), _)| *user)
            .collect()
    }

    async fn ensure(&self, app: &App, browser: Id) -> Result<Arc<Group>, String> {
        self.start_keeper(app);
        let mut groups = self.groups.lock().await;
        if let Some(group) = groups
            .get(&browser)
            .filter(|g| g.alive.load(Ordering::Acquire))
        {
            return Ok(Arc::clone(group));
        }
        let mut conn = app.db.acquire().await.map_err(|e| e.to_string())?;
        let row = rows::get(&mut conn, browser)
            .await
            .map_err(|e| e.to_string())?
            .filter(|r| r.state == "open")
            .ok_or_else(|| "the browser is not open".to_owned())?;
        let site = rows::site(&mut conn, row.environment_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "the environment is gone".to_owned())?;
        let machine = site
            .machine_id
            .ok_or_else(|| "the environment has no machine".to_owned())?;
        drop(conn);
        let ticket = app
            .link
            .tickets()
            .browser(machine, browser)
            .map_err(|e| e.to_string())?;
        let hub = lock(&self.hub)
            .clone()
            .unwrap_or_else(|| app.link.terminals().clone());
        let tunnel = hub
            .open(machine, &ticket)
            .await
            .map_err(|e| e.to_string())?;
        let (outgoing, rx) = mpsc::channel(INPUT_QUEUE);
        let (ready, _) = watch::channel(None);
        let group = Arc::new(Group {
            id: browser,
            environment: row.environment_id,
            outgoing,
            ready,
            state: Mutex::new(GroupState {
                ready: None,
                viewers: BTreeMap::new(),
                driver_at: BTreeMap::new(),
                published_driving: BTreeSet::new(),
                empty_at: app.clock.now(),
            }),
            alive: AtomicBool::new(true),
            stop: tokio_util::sync::CancellationToken::new(),
        });
        groups.insert(browser, Arc::clone(&group));
        let group_for_task = Arc::clone(&group);
        let sockets = self.clone();
        let app = Arc::downgrade(app);
        tokio::spawn(async move {
            sockets.pump(app, group_for_task, tunnel, rx).await;
        });
        Ok(group)
    }

    async fn pump(
        &self,
        app: Weak<AppState>,
        group: Arc<Group>,
        tunnel: Tunnel,
        mut outgoing: mpsc::Receiver<Json>,
    ) {
        let (mut reader, mut writer) = tokio::io::split(tunnel);
        let download = async {
            let mut inbox = BytesMut::new();
            let mut bytes = [0; READ_BYTES];
            while let Ok(n) = reader.read(&mut bytes).await {
                if n == 0 {
                    break;
                }
                inbox.extend_from_slice(&bytes[..n]);
                loop {
                    let message = match ExecMessage::take(&mut inbox, MAX_MESSAGE) {
                        Ok(Some(m)) => m,
                        Ok(None) => break,
                        Err(_) => return,
                    };
                    self.receive(&app, &group, message).await;
                }
            }
        };
        let upload = async {
            while let Some(frame) = outgoing.recv().await {
                let message = ExecMessage {
                    opcode: ExecOpcode::Text,
                    payload: Bytes::from(frame.to_string()),
                };
                let mut wire = BytesMut::new();
                if message.write(&mut wire).is_err() || writer.write_all(&wire).await.is_err() {
                    break;
                }
            }
        };
        tokio::select! {_=download=>{},_=upload=>{},_=group.stop.cancelled()=>{}}
        group.alive.store(false, Ordering::Release);
        let reason = if let Some(app) = app.upgrade() {
            let ending = Self::ending(&app, group.id).await;
            if ending == BrowserClosedReason::Revoked {
                BrowserClosedReason::LinkLost
            } else {
                ending
            }
        } else {
            BrowserClosedReason::LinkLost
        };
        let state = lock(&group.state);
        for viewer in state.viewers.values() {
            let _ = viewer
                .metadata
                .try_send(BrowserServerFrame::Closed { reason });
        }
    }

    async fn receive(&self, app: &Weak<AppState>, group: &Arc<Group>, message: ExecMessage) {
        match message.opcode {
            ExecOpcode::Binary => {
                let bytes = &message.payload;
                let Some(prefix) = bytes.get(..2) else {
                    return;
                };
                let size = usize::from(u16::from_be_bytes([prefix[0], prefix[1]]));
                let Some(tab) = bytes
                    .get(2..2 + size)
                    .and_then(|b| std::str::from_utf8(b).ok())
                else {
                    return;
                };
                let picture = bytes.slice(2 + size..);
                if PictureHeader::decode(&picture).is_none() {
                    return;
                }
                for viewer in lock(&group.state)
                    .viewers
                    .values()
                    .filter(|v| v.watch.as_ref().is_some_and(|w| w.tab == tab))
                {
                    viewer.picture.send_replace(Some(picture.clone()));
                }
            }
            ExecOpcode::Text => {
                let Ok(value) = serde_json::from_slice::<Json>(&message.payload) else {
                    return;
                };
                let tab = value["tab"].as_str();
                let Ok(mut frame) =
                    serde_json::from_value::<BrowserServerFrame>(value["frame"].clone())
                else {
                    return;
                };
                if matches!(frame, BrowserServerFrame::Closed { .. })
                    && let Some(app) = app.upgrade()
                {
                    let ending = Self::ending(&app, group.id).await;
                    if ending != BrowserClosedReason::Revoked {
                        frame = BrowserServerFrame::Closed { reason: ending };
                    }
                }
                match &frame {
                    BrowserServerFrame::Ready { tabs, viewport, .. } => {
                        let ready = Ready {
                            tabs: tabs.clone(),
                            viewport: *viewport,
                        };
                        lock(&group.state).ready = Some(ready.clone());
                        group.ready.send_replace(Some(ready));
                    }
                    BrowserServerFrame::Tabs { tabs } => {
                        if let Some(ready) = lock(&group.state).ready.as_mut() {
                            ready.tabs = tabs.clone();
                        }
                        if let Some(app) = app.upgrade() {
                            let now = app.clock.now();
                            let mut sittings = lock(&self.sittings);
                            for ((browser, _), sitting) in
                                sittings.iter_mut().filter(|((browser, _), s)| {
                                    *browser == group.id && now - s.last < SITTING_GAP
                                })
                            {
                                let _ = browser;
                                for target in tabs.iter().filter(|t| sitting.tabs.contains(&t.id)) {
                                    if let Some(host) = host(&target.url) {
                                        sitting.hosts.insert(host);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if let BrowserServerFrame::Ready { people, .. } = &mut frame
                    && let Some(app) = app.upgrade()
                {
                    *people = self.people(&app, group).await;
                }
                let state = lock(&group.state);
                for viewer in state.viewers.values() {
                    if tab.is_some_and(|tab| !viewer.watch.as_ref().is_some_and(|w| w.tab == tab)) {
                        continue;
                    }
                    if matches!(
                        frame,
                        BrowserServerFrame::Dialog { .. }
                            | BrowserServerFrame::Chooser { .. }
                            | BrowserServerFrame::Copied { .. }
                    ) && !viewer.control
                    {
                        continue;
                    }
                    let mut frame = frame.clone();
                    if let BrowserServerFrame::Ready { control, .. } = &mut frame {
                        *control = viewer.control;
                    }
                    let _ = viewer.metadata.try_send(frame);
                }
            }
        }
    }

    async fn aggregate(group: &Group, tab: &str) {
        let value = {
            let state = lock(&group.state);
            let watchers: Vec<_> = state
                .viewers
                .values()
                .filter(|v| v.watch.as_ref().is_some_and(|w| w.tab == tab))
                .collect();
            let width = watchers
                .iter()
                .filter_map(|v| v.watch.as_ref())
                .map(|w| (f64::from(w.width) * w.dpr).min(f64::from(u16::MAX)) as u16)
                .max()
                .unwrap_or_default();
            let height = watchers
                .iter()
                .filter_map(|v| v.watch.as_ref())
                .map(|w| (f64::from(w.height) * w.dpr).min(f64::from(u16::MAX)) as u16)
                .max()
                .unwrap_or_default();
            json!({"op":"watch","tab":tab,"width":width,"height":height,"dpr":1,"control":watchers.iter().any(|v|v.control)})
        };
        let _ = group.outgoing.send(value).await;
    }

    async fn gates(app: &App, ticket: &Ticket) -> ApiResult<(bool, bool, bool)> {
        let mut conn = app.db.acquire().await?;
        let now = app.clock.now();
        let Some(principal) = ticket.identity.principal(&mut conn, Time::new(now)).await? else {
            return Ok((false, false, false));
        };
        let Some(row) = rows::get(&mut conn, ticket.browser).await? else {
            return Ok((false, false, false));
        };
        let Some(site) = rows::site(&mut conn, row.environment_id).await? else {
            return Ok((false, false, false));
        };
        if row.state != "open"
            || row.env_generation != site.generation
            || orch_engine::browsers::Browsers::off(&mut conn, &site)
                .await
                .map_err(ApiError::internal)?
                .is_some()
        {
            return Ok((false, false, false));
        }
        let view = views::holds(
            &mut conn,
            &principal,
            row.environment_id,
            Permission::BrowserView,
        )
        .await?;
        let control = ticket.control
            && views::holds(
                &mut conn,
                &principal,
                row.environment_id,
                Permission::BrowserManage,
            )
            .await?;
        let files = control
            && views::holds(
                &mut conn,
                &principal,
                row.environment_id,
                Permission::FilesView,
            )
            .await?;
        Ok((view, control, files))
    }

    async fn socket(&self, app: App, ticket: Ticket, mut socket: WebSocket) {
        match Self::gates(&app, &ticket).await {
            Ok((true, _, _)) => {}
            Ok(_) => {
                let _ = send(
                    &mut socket,
                    &BrowserServerFrame::Closed {
                        reason: BrowserClosedReason::Revoked,
                    },
                )
                .await;
                return;
            }
            Err(_) => return,
        }
        let group = match self.ensure(&app, ticket.browser).await {
            Ok(g) => g,
            Err(_) => {
                let _ = send(
                    &mut socket,
                    &BrowserServerFrame::Closed {
                        reason: BrowserClosedReason::LinkLost,
                    },
                )
                .await;
                return;
            }
        };
        let mut ready_rx = group.ready.subscribe();
        if ready_rx.borrow().is_none()
            && tokio::time::timeout(READY_WAIT, ready_rx.changed())
                .await
                .is_err()
        {
            return;
        }
        let Some(ready) = ready_rx.borrow().clone() else {
            return;
        };
        let (view, mut control, _) = match Self::gates(&app, &ticket).await {
            Ok(g) => g,
            Err(_) => return,
        };
        if !view {
            let _ = send(
                &mut socket,
                &BrowserServerFrame::Closed {
                    reason: Self::ending(&app, ticket.browser).await,
                },
            )
            .await;
            return;
        }
        let id = self.numbers.fetch_add(1, Ordering::Relaxed);
        let (metadata, mut meta_rx) = mpsc::channel(META_QUEUE);
        let (picture, mut pic_rx) = watch::channel(None);
        lock(&group.state).viewers.insert(
            id,
            Viewer {
                user: ticket.user,
                watch: None,
                control,
                metadata,
                picture,
            },
        );
        self.count(&app, &group).await;
        let people = self.people(&app, &group).await;
        if send(
            &mut socket,
            &BrowserServerFrame::Ready {
                control,
                tabs: ready.tabs,
                viewport: ready.viewport,
                people,
            },
        )
        .await
        .is_err()
        {
            self.depart(&app, &group, id).await;
            return;
        }
        let mut in_flight = None;
        let mut keeper = tokio::time::interval(KEEPER_TICK);
        loop {
            tokio::select! {
                message=socket.recv()=>{
                    let Some(Ok(message))=message else {break;};
                    match message {
                        Message::Text(text)=>{
                            if text.len()>MAX_INPUT {break;}
                            let frame=match serde_json::from_str::<BrowserClientFrame>(&text) {Ok(frame)=>frame,Err(_)=>{let _=send(&mut socket,&BrowserServerFrame::Error{code:BrowserSocketError::Invalid,detail:"This is not a browser frame.".to_owned()}).await;continue;}};
                            if let BrowserClientFrame::Ping{n}=frame {let _=send(&mut socket,&BrowserServerFrame::Pong{n}).await;continue;}
                            if let BrowserClientFrame::Ack{n}=frame {if in_flight==Some(n){in_flight=None;}continue;}
                            let (view,now_control,now_files)=match Self::gates(&app,&ticket).await {Ok(g)=>g,Err(_)=>break};
                            if !view {let _=send(&mut socket,&BrowserServerFrame::Closed{reason:Self::ending(&app,ticket.browser).await}).await;break;}
                            if control!=now_control {
                                let tab={let mut state=lock(&group.state);state.viewers.get_mut(&id).and_then(|v|{v.control=now_control;v.watch.as_ref().map(|w|w.tab.clone())})};
                                if let Some(tab)=tab {Self::aggregate(&group,&tab).await;}
                            }
                            control=now_control;let files=now_files;
                            if frame.needs_control()&&!control {let _=send(&mut socket,&BrowserServerFrame::Error{code:BrowserSocketError::ViewOnly,detail:"This ticket can only watch.".to_owned()}).await;continue;}
                            if needs_files(&frame)&&!files {let _=send(&mut socket,&BrowserServerFrame::Error{code:BrowserSocketError::NotAllowed,detail:"This input also needs files.view on the environment.".to_owned()}).await;continue;}
                            if let BrowserClientFrame::Watch{tab,width,height,dpr}=&frame {
                                if *width==0||*height==0||!dpr.is_finite()||*dpr<=0.0 {let _=send(&mut socket,&BrowserServerFrame::Error{code:BrowserSocketError::Invalid,detail:"A watch needs a positive panel size and density.".to_owned()}).await;continue;}
                                let exists={let state=lock(&group.state);state.ready.as_ref().is_some_and(|r|r.tabs.iter().any(|t|t.id==*tab))};
                                if !exists {let _=send(&mut socket,&BrowserServerFrame::Error{code:BrowserSocketError::NoSuchTab,detail:"That tab no longer exists.".to_owned()}).await;continue;}
                                let old={let mut state=lock(&group.state);let Some(viewer)=state.viewers.get_mut(&id) else {break;};viewer.control=control;let old=viewer.watch.replace(Watch{tab:tab.clone(),width:*width,height:*height,dpr:*dpr});viewer.picture.send_replace(None);old};
                                in_flight=None;pic_rx.borrow_and_update();
                                if let Some(old)=old {Self::aggregate(&group,&old.tab).await;}Self::aggregate(&group,tab).await;
                            } else {
                                self.input(&app,&group,ticket.user,&frame);
                                if group.outgoing.send(serde_json::to_value(&frame).unwrap_or(Json::Null)).await.is_err() {break;}
                                let _=app.engine.browsers().used(ticket.browser).await;
                            }
                        }
                        Message::Binary(_)=>{let _=send(&mut socket,&BrowserServerFrame::Error{code:if control{BrowserSocketError::Invalid}else{BrowserSocketError::ViewOnly},detail:"Input is carried as checked text frames.".to_owned()}).await;}
                        Message::Ping(data)=>{if socket.send(Message::Pong(data)).await.is_err(){break;}}
                        Message::Close(_)=>break,
                        Message::Pong(_)=>{},
                    }
                }
                frame=meta_rx.recv()=>{let Some(frame)=frame else {break;};let Ok((view,control,_))=Self::gates(&app,&ticket).await else {break;};if !view {let _=send(&mut socket,&BrowserServerFrame::Closed{reason:Self::ending(&app,ticket.browser).await}).await;break;}
                    if !control&&matches!(frame,BrowserServerFrame::Dialog{..}|BrowserServerFrame::Chooser{..}|BrowserServerFrame::Copied{..}){continue;}let closed=matches!(frame,BrowserServerFrame::Closed{..});if send(&mut socket,&frame).await.is_err()||closed{break;}}
                changed=pic_rx.changed(),if in_flight.is_none()=>{
                    if changed.is_err(){break;}let Ok((view,_,_))=Self::gates(&app,&ticket).await else {break;};if !view {let _=send(&mut socket,&BrowserServerFrame::Closed{reason:Self::ending(&app,ticket.browser).await}).await;break;}
                    let picture=pic_rx.borrow_and_update().clone();if let Some(picture)=picture {in_flight=PictureHeader::decode(&picture).map(|h|h.n);if socket.send(Message::Binary(picture)).await.is_err(){break;}}
                }
                _=keeper.tick()=>{
                    let (view,now_control,now_files)=match Self::gates(&app,&ticket).await {Ok(g)=>g,Err(_)=>break};
                    if !view {let _=send(&mut socket,&BrowserServerFrame::Closed{reason:Self::ending(&app,ticket.browser).await}).await;break;}
                    if control&&!now_control {
                        let tab={let mut state=lock(&group.state);state.viewers.get_mut(&id).and_then(|v|{v.control=false;v.watch.as_ref().map(|w|w.tab.clone())})};
                        if let Some(tab)=tab {Self::aggregate(&group,&tab).await;}
                        let ready=lock(&group.state).ready.clone();if let Some(ready)=ready {let people=self.people(&app,&group).await;let _=send(&mut socket,&BrowserServerFrame::Ready{control:false,tabs:ready.tabs,viewport:ready.viewport,people}).await;}
                    }
                    control=now_control;let _=now_files;
                    let people=self.people(&app,&group).await;if send(&mut socket,&BrowserServerFrame::People{watching:people.watching,driving:people.driving}).await.is_err(){break;}
                    let _=app.engine.browsers().used(ticket.browser).await;
                }
            }
        }
        self.depart(&app, &group, id).await;
        let _ = socket.close().await;
    }

    async fn count(&self, app: &App, group: &Group) {
        let count = lock(&group.state)
            .viewers
            .values()
            .map(|v| v.user)
            .collect::<BTreeSet<_>>()
            .len();
        app.engine
            .browsers()
            .memory()
            .set_watching(group.id, i64::try_from(count).unwrap_or(i64::MAX));
        app.engine.browsers().changed(group.environment).await;
    }

    async fn people(&self, app: &App, group: &Group) -> BrowserPeople {
        let watching: Vec<_> = lock(&group.state)
            .viewers
            .values()
            .map(|v| v.user)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let driving = self.driving(group.id, app.clock.now());
        let mut conn = match app.db.acquire().await {
            Ok(c) => c,
            Err(_) => {
                return BrowserPeople {
                    watching: Vec::new(),
                    driving: Vec::new(),
                };
            }
        };
        let ids: Vec<_> = watching.iter().chain(&driving).copied().collect();
        let refs = crate::middleware::authorization::refusals::user_refs(&mut conn, &ids)
            .await
            .unwrap_or_default();
        BrowserPeople {
            watching: watching
                .iter()
                .filter_map(|p| refs.get(p).cloned())
                .collect(),
            driving: driving
                .iter()
                .filter_map(|p| refs.get(p).cloned())
                .collect(),
        }
    }

    async fn depart(&self, app: &App, group: &Group, id: u64) {
        let removed = {
            let mut state = lock(&group.state);
            let removed = state.viewers.remove(&id);
            if state.viewers.is_empty() {
                state.empty_at = app.clock.now();
            }
            removed
        };
        if let Some(viewer) = removed
            && let Some(watch) = viewer.watch
        {
            Self::aggregate(group, &watch.tab).await;
        }
        self.count(app, group).await;
    }

    fn input(&self, app: &App, group: &Group, user: Id, frame: &BrowserClientFrame) {
        let now = app.clock.now();
        let tab = frame_tab(frame);
        let url = match frame {
            BrowserClientFrame::Navigate { url, .. }
            | BrowserClientFrame::Tab { url: Some(url), .. } => Some(url.as_str()),
            _ => None,
        };
        let mut sittings = lock(&self.sittings);
        let sitting = sittings.entry((group.id, user)).or_insert_with(|| Sitting {
            from: now,
            last: now,
            hosts: BTreeSet::new(),
            tabs: BTreeSet::new(),
        });
        sitting.last = now;
        if let Some(tab) = tab {
            sitting.tabs.insert(tab.to_owned());
            if let Some(ready) = lock(&group.state).ready.as_ref()
                && let Some(page) = ready.tabs.iter().find(|t| t.id == tab)
                && let Some(host) = host(&page.url)
            {
                sitting.hosts.insert(host);
            }
        }
        if let Some(host) = url.and_then(host) {
            sitting.hosts.insert(host);
        }
        lock(&group.state).driver_at.insert(user, now);
    }

    async fn keep(&self, app: &App) {
        let now = app.clock.now();
        let expired = {
            let mut sittings = lock(&self.sittings);
            let keys: Vec<_> = sittings
                .iter()
                .filter(|(_, s)| now - s.last >= SITTING_GAP)
                .map(|(key, _)| *key)
                .collect();
            keys.into_iter()
                .filter_map(|key| sittings.remove(&key).map(|s| (key, s)))
                .collect::<Vec<_>>()
        };
        for ((browser, user), sitting) in expired {
            let _ = app
                .engine
                .browsers()
                .sitting(
                    browser,
                    user,
                    sitting.from,
                    sitting.last + SITTING_GAP,
                    sitting.hosts,
                )
                .await;
        }
        let active_sittings: BTreeSet<_> = lock(&self.sittings)
            .keys()
            .map(|(browser, _)| *browser)
            .collect();
        let mut groups = self.groups.lock().await;
        let mut changed = BTreeSet::new();
        groups.retain(|id, group| {
            let mut state = lock(&group.state);
            let driving: BTreeSet<_> = state
                .driver_at
                .iter()
                .filter(|(_, at)| now - **at < DRIVING)
                .map(|(user, _)| *user)
                .collect();
            if !driving.is_empty() || driving != state.published_driving {
                state.published_driving = driving;
                changed.insert(group.environment);
            }
            let keep = group.alive.load(Ordering::Acquire)
                && (!state.viewers.is_empty()
                    || active_sittings.contains(id)
                    || now - state.empty_at < TUNNEL_GRACE);
            if !keep {
                group.stop.cancel();
            }
            keep
        });
        drop(groups);
        for environment in changed {
            app.engine.browsers().changed(environment).await;
        }
        let _ = app.engine.browsers().sweep().await;
    }
}

async fn send(socket: &mut WebSocket, frame: &BrowserServerFrame) -> Result<(), ()> {
    socket
        .send(Message::Text(
            serde_json::to_string(frame).map_err(|_| ())?.into(),
        ))
        .await
        .map_err(|_| ())
}

fn host(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_owned)
}

fn frame_tab(frame: &BrowserClientFrame) -> Option<&str> {
    match frame {
        BrowserClientFrame::Pointer { tab, .. }
        | BrowserClientFrame::Key { tab, .. }
        | BrowserClientFrame::Text { tab, .. }
        | BrowserClientFrame::Navigate { tab, .. }
        | BrowserClientFrame::History { tab, .. }
        | BrowserClientFrame::Dialog { tab, .. }
        | BrowserClientFrame::Files { tab, .. }
        | BrowserClientFrame::Copy { tab, .. } => Some(tab),
        BrowserClientFrame::Tab { tab: Some(tab), .. } => Some(tab),
        _ => None,
    }
}

fn needs_files(frame: &BrowserClientFrame) -> bool {
    match frame {
        BrowserClientFrame::Files {
            paths: Some(paths), ..
        } => !paths.is_empty(),
        BrowserClientFrame::Navigate { url, .. }
        | BrowserClientFrame::Tab { url: Some(url), .. } => {
            url::Url::parse(url).is_ok_and(|u| u.scheme() == "file")
        }
        _ => false,
    }
}

/// Upgrades a socket only after spending its single-use ticket.
async fn attach_checked(
    ctx: Ctx,
    query: BrowserAttachQuery,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    let protocol = ctx
        .headers
        .get(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|protocol| protocol.trim() == BROWSER_SUBPROTOCOL)
        });
    if !protocol {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::RequestInvalid,
            "the socket needs orch.browser.v1",
        ));
    }
    let browser = ctx.id("browser_id")?;
    let sockets = ctx.app.realtime.browsers().clone();
    let ticket = sockets
        .redeem(browser, &query.ticket, ctx.now)
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::Gone,
                "the browser ticket expired or was used",
            )
        })?;
    let app = Arc::clone(&ctx.app);
    Ok(upgrade
        .protocols([BROWSER_SUBPROTOCOL])
        .max_message_size(MAX_INPUT)
        .on_upgrade(move |socket| async move {
            sockets.socket(app, ticket, socket).await;
        }))
}

/// Upgrades with the declared protocol, or returns the ticket refusal as HTTP.
pub(crate) async fn attach(
    ctx: Ctx,
    query: BrowserAttachQuery,
    upgrade: WebSocketUpgrade,
) -> Response {
    let request_id = ctx.request_id.clone();
    match attach_checked(ctx, query, upgrade).await {
        Ok(response) => response,
        Err(error) => error.into_response_for(&request_id),
    }
}

struct Actions {
    app: Weak<AppState>,
}

#[async_trait]
impl BrowserActions for Actions {
    async fn navigate(&self, browser: Id, url: &str) -> Result<(), String> {
        let app = self
            .app
            .upgrade()
            .ok_or_else(|| "the API stopped".to_owned())?;
        let group = app.realtime.browsers().ensure(&app, browser).await?;
        let mut ready = group.ready.subscribe();
        if ready.borrow().is_none() {
            tokio::time::timeout(READY_WAIT, ready.changed())
                .await
                .map_err(|_| "the browser did not send its tabs".to_owned())?
                .map_err(|_| "the browser ended".to_owned())?;
        }
        let tab = ready
            .borrow()
            .as_ref()
            .and_then(|r| r.tabs.first())
            .map(|t| t.id.clone())
            .ok_or_else(|| "the browser has no page".to_owned())?;
        group
            .outgoing
            .send(json!({"op":"navigate","tab":tab,"url":url}))
            .await
            .map_err(|_| "the browser ended".to_owned())
    }

    async fn changed(&self, environment: Id) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let Ok(mut conn) = app.db.acquire().await else {
            return;
        };
        if let Ok(current) = views::current(&app, &mut conn, None, environment).await {
            drop(conn);
            if !current.browsers.is_empty() {
                app.realtime.browsers().start_keeper(&app);
            }
            let _ = app
                .realtime
                .publish_list(
                    Feed::EnvBrowsers(environment),
                    json!({"browsers":current.browsers,"profiles":current.profiles}),
                )
                .await;
        }
    }
}

/// Installs browser transport once an application's complete handle is available.
pub(crate) fn install(app: &App) {
    let sockets = app.realtime.browsers().clone();
    if sockets.installed.swap(true, Ordering::AcqRel) {
        return;
    }
    app.engine.install_browser_actions(Arc::new(Actions {
        app: Arc::downgrade(app),
    }));
}

/// Recovers unattended browser rows when the HTTP application starts serving.
pub(crate) fn recover(app: &App) {
    let sockets = app.realtime.browsers().clone();
    if sockets.recovery_started.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(app);
    tokio::spawn(async move {
        let Some(app) = weak.upgrade() else { return };
        let Ok(mut conn) = app.db.acquire().await else {
            return;
        };
        // Recover unattended browsers once; an unused browser feature needs no recurring SQL.
        let needed = rows::needs_maintenance(&mut conn).await.unwrap_or(false);
        drop(conn);
        if needed {
            sockets.start_keeper(&app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_navigation_and_workspace_chooser_paths_need_the_files_door() {
        let navigate = BrowserClientFrame::Navigate {
            tab: "t".to_owned(),
            url: "file:///workspace/a".to_owned(),
        };
        assert!(needs_files(&navigate));
        let text = BrowserClientFrame::Text {
            tab: "t".to_owned(),
            text: "file:///workspace/a".to_owned(),
        };
        assert!(!needs_files(&text));
    }
    #[test]
    fn a_sitting_keeps_only_the_host_of_a_visited_address() {
        assert_eq!(
            host("https://example.test/private?q=secret"),
            Some("example.test".to_owned())
        );
        assert_eq!(host("about:blank"), None);
    }
}
