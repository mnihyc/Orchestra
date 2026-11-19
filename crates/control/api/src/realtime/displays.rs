//! Display sockets terminate viewer security and filter every input under current authority.

use super::rfb_filter::{Filter, MAX_INPUT, ProtocolError};
use super::ticket_origin::{CurrentCaller, Origin};
use crate::middleware::authentication::{new_token, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::displays as views;
use crate::state::{App, AppState};
use async_trait::async_trait;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Duration as Span, Utc};
use futures::StreamExt;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::displays::*;
use orch_db::queries::displays as rows;
use orch_engine::displays::{DisplayActions, Displays, SITTING_GAP};
use orch_realtime_contract::feeds::Feed;
use orch_types::{Id, Time, new_id};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{
    Arc, Mutex, PoisonError, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Matches browser gate maintenance without holding a database connection between ticks.
const KEEPER_TICK: Duration = Duration::from_secs(1);
/// A local handshake and its link have the same forty-second bound as display commands.
const READY_WAIT: Duration = Duration::from_secs(40);
/// Small reads preserve end-to-end flow control, without picture queues or a spool.
const READ_BYTES: usize = 16 * 1024;
/// The current display list marks a driver for ten seconds after actual input.
const DRIVING: Span = Span::seconds(10);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
struct Ticket {
    surface: orch_api_contract::surface::HttpSurface,
    display: Id,
    environment: Id,
    machine: Id,
    generation: i32,
    origin: Origin,
    control: bool,
    expires: DateTime<Utc>,
}
struct Sitting {
    from: DateTime<Utc>,
    last: DateTime<Utc>,
}
#[derive(PartialEq, Eq)]
struct ViewerState {
    watching: i64,
    driving: Vec<Id>,
}

/// Only short-lived tickets, viewer identities and safe sitting times are held here.
#[derive(Clone, Default)]
pub struct Sockets {
    tickets: Arc<Mutex<HashMap<Vec<u8>, Ticket>>>,
    viewers: Arc<Mutex<BTreeMap<(Id, Id), Id>>>,
    sittings: Arc<Mutex<BTreeMap<(Id, Id), Sitting>>>,
    published: Arc<Mutex<BTreeMap<Id, ViewerState>>>,
    hub: Arc<Mutex<Option<orch_link::tunnels::terminal::TerminalHub>>>,
    installed: Arc<AtomicBool>,
    keeper_started: Arc<AtomicBool>,
}
impl std::fmt::Debug for Sockets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplaySockets").finish_non_exhaustive()
    }
}

impl Sockets {
    /// Uses a test machine hub; production opens through the existing gateway backplane.
    pub fn through(&self, hub: orch_link::tunnels::terminal::TerminalHub) {
        *lock(&self.hub) = Some(hub);
    }
    /// The number of people viewing, counting several tabs of one person once.
    pub fn watching(&self, display: Id) -> i64 {
        let users: BTreeSet<_> = lock(&self.viewers)
            .iter()
            .filter(|((id, _), _)| *id == display)
            .map(|(_, user)| *user)
            .collect();
        i64::try_from(users.len()).unwrap_or(i64::MAX)
    }
    /// People who have sent actual input recently.
    pub fn driving(&self, display: Id, now: DateTime<Utc>) -> Vec<Id> {
        lock(&self.sittings)
            .iter()
            .filter(|((id, _), s)| *id == display && s.last + DRIVING > now)
            .map(|((_, user), _)| *user)
            .collect()
    }
    /// Issues a single-use ticket bound to one display and the original restricted identity.
    pub fn ticket(
        &self,
        ctx: &Ctx,
        display: Id,
        control: bool,
        environment: Id,
        machine: Id,
        generation: i32,
    ) -> ApiResult<DisplayTicketCreateReply> {
        let token = new_token("dt_");
        let expires = ctx.now + Span::seconds(i64::from(DISPLAY_TICKET_SECONDS));
        let origin = Origin::capture(ctx)?;
        let selection = if control {
            "DisplayTicketCreate.control"
        } else {
            "DisplayTicketCreate.view"
        };
        if origin.operation() != Some(selection) {
            return Err(ApiError::internal(
                "a bound native ticket needs its actual selected operation",
            ));
        }
        let mut tickets = lock(&self.tickets);
        tickets.retain(|_, t| t.expires > ctx.now);
        tickets.insert(
            token_hash(&token),
            Ticket {
                surface: ctx.http_surface()?,
                display,
                environment,
                machine,
                generation,
                origin,
                control,
                expires,
            },
        );
        Ok(DisplayTicketCreateReply {
            ticket: token,
            ticket_expires_at: Time::new(expires),
        })
    }
    fn redeem(
        &self,
        display: Id,
        token: &str,
        now: DateTime<Utc>,
        surface: orch_api_contract::surface::HttpSurface,
    ) -> Option<Ticket> {
        let mut tickets = lock(&self.tickets);
        let hash = token_hash(token);
        if tickets
            .get(&hash)
            .is_none_or(|held| held.surface != surface)
        {
            return None;
        }
        tickets
            .remove(&hash)
            .filter(|held| held.display == display && held.expires > now)
    }

    async fn gates(
        app: &App,
        ticket: &Ticket,
    ) -> ApiResult<(bool, bool, &'static str, Option<CurrentCaller>)> {
        let mut conn = app.db.begin_snapshot().await?;
        let Some((caller, session_deadline)) = ticket
            .origin
            .restore_current(&mut conn, app.clock.as_ref())
            .await?
        else {
            return Ok((false, false, "revoked", None));
        };
        let Some(row) = rows::get(&mut conn, ticket.display).await? else {
            return Ok((false, false, "exited", None));
        };
        if row.state == "closed" {
            return Ok((
                false,
                false,
                match row.close_reason.as_deref() {
                    Some("closed_by_user") => "quit",
                    Some("closed_by_agent") => "closed_by_agent",
                    Some("environment_stopped") => "environment_stopped",
                    Some("switched_off") => "switched_off",
                    Some("link_lost") => "link_lost",
                    _ => "exited",
                },
                None,
            ));
        }
        let Some(site) = rows::site(&mut conn, row.environment_id).await? else {
            return Ok((false, false, "environment_stopped", None));
        };
        if row.environment_id != ticket.environment
            || row.env_generation != site.generation
            || row.env_generation != ticket.generation
            || site.machine_id != Some(ticket.machine)
            || !matches!(site.status.as_str(), "running" | "offline")
        {
            return Ok((false, false, "environment_stopped", None));
        }
        if let Some(off) = Displays::off(&mut conn, &site)
            .await
            .map_err(ApiError::internal)?
        {
            return Ok((
                false,
                false,
                if off == orch_api_contract::browsers::BrowsersOff::EnvironmentOffline {
                    "link_lost"
                } else {
                    "switched_off"
                },
                None,
            ));
        }
        let Some((principal, scope_control)) = ticket
            .origin
            .principal(
                &mut conn,
                caller,
                row.environment_id,
                if ticket.control {
                    "DisplayTicketCreate.control"
                } else {
                    "DisplayTicketCreate.view"
                },
                app.clock.as_ref(),
            )
            .await?
        else {
            return Ok((false, false, "revoked", None));
        };
        let view = views::holds(
            &mut conn,
            &principal,
            row.environment_id,
            Permission::VncView,
        )
        .await?;
        let control = ticket.control
            && scope_control
            && views::holds(
                &mut conn,
                &principal,
                row.environment_id,
                Permission::VncManage,
            )
            .await?;
        conn.commit().await.map_err(orch_db::DbError::from)?;
        let caller = principal.caller().clone();
        if !super::ticket_origin::deadline_allows(&caller, session_deadline, app.clock.as_ref()) {
            return Ok((false, false, "revoked", None));
        }
        Ok((
            view && row.state == "open",
            control,
            "revoked",
            Some((caller, session_deadline)),
        ))
    }

    async fn publish_bytes(
        app: &App,
        ticket: &Ticket,
        socket: &mut WebSocket,
        bytes: Bytes,
    ) -> ApiResult<bool> {
        super::publication::send(socket, app.clock.as_ref(), async {
            let (view, _, _, current) = Self::gates(app, ticket).await?;
            Ok(view
                .then_some(current)
                .flatten()
                .map(|current| (Message::Binary(bytes), current)))
        })
        .await
    }

    async fn input_current(
        app: &App,
        ticket: &Ticket,
        action: bool,
    ) -> ApiResult<Option<CurrentCaller>> {
        let (view, control, _, current) = Self::gates(app, ticket).await?;
        Ok((view && (!action || control)).then_some(current).flatten())
    }

    async fn socket(&self, app: App, ticket: Ticket, mut socket: WebSocket) {
        let reason = self.relay(&app, &ticket, &mut socket).await;
        // Stopping a display can end RFB before the keeper observes its committed close.
        // Use that current close reason instead of reporting a broken link to its viewer.
        let reason = if reason == "link_lost" {
            match Self::gates(&app, &ticket).await {
                Ok((false, _, closed, _)) => closed,
                _ => reason,
            }
        } else {
            reason
        };
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: DISPLAY_CLOSE_CODE,
                reason: reason.into(),
            })))
            .await;
    }

    async fn relay(&self, app: &App, ticket: &Ticket, socket: &mut WebSocket) -> &'static str {
        match Self::gates(app, ticket).await {
            Ok((true, _, _, _)) => {}
            Ok((_, _, reason, _)) => return reason,
            Err(_) => return "link_lost",
        }
        let machine = {
            let Ok(mut conn) = app.db.acquire().await else {
                return "link_lost";
            };
            let Ok(Some(row)) = rows::get(&mut conn, ticket.display).await else {
                return "exited";
            };
            let Ok(Some(site)) = rows::site(&mut conn, row.environment_id).await else {
                return "environment_stopped";
            };
            let Some(machine) = site.machine_id else {
                return "link_lost";
            };
            machine
        };
        let Ok(link_ticket) = app.link.tickets().display(machine, ticket.display) else {
            return "link_lost";
        };
        let hub = lock(&self.hub)
            .clone()
            .unwrap_or_else(|| app.link.terminals().clone());
        let mut tunnel =
            match tokio::time::timeout(READY_WAIT, hub.open(machine, &link_ticket)).await {
                Ok(Ok(tunnel)) => tunnel,
                _ => return "link_lost",
            };
        let mut version = [0; 12];
        if !matches!(
            tokio::time::timeout(READY_WAIT, tunnel.read_exact(&mut version)).await,
            Ok(Ok(_))
        ) {
            return "protocol";
        }
        let mut handshake = match Handshake::new(version) {
            Ok(handshake) => handshake,
            Err(_) => return "protocol",
        };
        // The native handshake awaited IO; restore current Read again before its first bytes.
        match Self::gates(app, ticket).await {
            Ok((true, _, _, _)) => {}
            Ok((_, _, reason, _)) => return reason,
            Err(_) => return "link_lost",
        }
        if !matches!(
            Self::publish_bytes(app, ticket, socket, Bytes::copy_from_slice(&version)).await,
            Ok(true)
        ) {
            return match Self::gates(app, ticket).await {
                Ok((false, _, reason, _)) => reason,
                _ => "link_lost",
            };
        }
        let (mut reader, mut writer) = tokio::io::split(tunnel);
        let viewer = new_id();
        lock(&self.viewers).insert((ticket.display, viewer), ticket.origin.user());
        let _ = app.engine.displays().used(ticket.display).await;
        if let Ok(mut conn) = app.db.acquire().await
            && let Ok(Some(row)) = rows::get(&mut conn, ticket.display).await
        {
            drop(conn);
            app.engine.displays().changed(row.environment_id).await;
        }
        let mut filter = Filter::default();
        let mut bytes = [0; READ_BYTES];
        let mut tick = tokio::time::interval(KEEPER_TICK);
        let mut permission_epoch = app.realtime.inner.epoch.subscribe();
        let mut checked_at = tokio::time::Instant::now();
        let handshake_deadline = tokio::time::sleep(READY_WAIT);
        tokio::pin!(handshake_deadline);
        let reason = loop {
            tokio::select! {
                _=&mut handshake_deadline,if !handshake.ready()=>break "protocol",
                incoming=socket.next()=>{
                    let Some(Ok(message))=incoming else{break "link_lost"};
                    match message {
                        Message::Binary(data)=>{
                            let (view,_,reason,_)=match Self::gates(app,ticket).await{Ok(value)=>value,Err(_)=>break "link_lost"};
                            if !view{break reason}
                            let header=match handshake.receive(&data){Ok(header)=>header,Err(_)=>break "protocol"};
                            let mut failed=false;
                            for reply in header.replies{if !matches!(Self::publish_bytes(app,ticket,socket,reply).await,Ok(true)){failed=true;break;}}
                            if failed{break match Self::gates(app,ticket).await {Ok((false,_,reason,_))=>reason,_=>"link_lost"}}
                            if header.shared {
                                match super::publication::write_message(&mut writer,&[1],app.clock.as_ref(),||Self::input_current(app,ticket,false)).await {
                                    Ok(Some(Ok(())))=>{},
                                    Ok(None)=>{break match Self::gates(app,ticket).await{Ok((false,_,reason,_))=>reason,_=>"link_lost"}},
                                    _=>break "link_lost",
                                }
                            }
                            if filter.push(&header.data).is_err(){break "protocol"}
                            let mut failure=None;
                            loop {
                                let message=match filter.take(){Ok(Some(message))=>message,Ok(None)=>break,Err(_)=>{failure=Some("protocol");break;}};
                                let (view,control,reason,_)=match Self::gates(app,ticket).await{Ok(value)=>value,Err(_)=>{failure=Some("link_lost");break;}};
                                if !view {failure=Some(reason);break;}
                                if message.action&&!control{continue}
                                match super::publication::write_message(&mut writer,&message.bytes,app.clock.as_ref(),||Self::input_current(app,ticket,message.action)).await {
                                    Ok(Some(Ok(())))=>{if message.activity{self.input(app,ticket);}},
                                    Ok(None)=>{
                                        match Self::gates(app,ticket).await{
                                            Ok((true,_,_,_))=>continue,
                                            Ok((false,_,reason,_))=>{failure=Some(reason);break;},
                                            Err(_)=>{failure=Some("link_lost");break;},
                                        }
                                    },
                                    _=>{failure=Some("link_lost");break;},
                                }
                            }
                            if let Some(reason)=failure{break reason}
                        }
                        Message::Ping(bytes)=>{if socket.send(Message::Pong(bytes)).await.is_err(){break "link_lost"}},
                        Message::Pong(_)=>{},
                        Message::Close(_)=>break "quit",
                        Message::Text(_)=>break "protocol",
                    }
                }
                output=reader.read(&mut bytes)=>{
                    let n=match output{Ok(0)|Err(_)=>break "link_lost",Ok(n)=>n};
                    // Raw RFB has no picture boundaries. A current epoch or the normal keeper tick
                    // refreshes viewing; input still refreshes authority for every complete message.
                    if permission_epoch.has_changed().unwrap_or(true)||checked_at.elapsed()>=KEEPER_TICK {
                        let (view,_,reason,_)=match Self::gates(app,ticket).await{Ok(value)=>value,Err(_)=>break "link_lost"};
                        if !view{break reason}
                        permission_epoch.borrow_and_update();checked_at=tokio::time::Instant::now();
                    }
                    if !matches!(Self::publish_bytes(app,ticket,socket,Bytes::copy_from_slice(&bytes[..n])).await,Ok(true)){
                        break match Self::gates(app,ticket).await {Ok((false,_,reason,_))=>reason,_=>"link_lost"}
                    }
                }
                _=tick.tick()=>{
                    let (view,_,reason,_)=match Self::gates(app,ticket).await{Ok(value)=>value,Err(_)=>break "link_lost"};
                    if !view{break reason}
                    permission_epoch.borrow_and_update();checked_at=tokio::time::Instant::now();
                    let _=app.engine.displays().used(ticket.display).await;
                }
                _=app.realtime.inner.stop.cancelled()=>break "link_lost",
            }
        };
        lock(&self.viewers).remove(&(ticket.display, viewer));
        if let Ok(mut conn) = app.db.acquire().await
            && let Ok(Some(row)) = rows::get(&mut conn, ticket.display).await
        {
            drop(conn);
            app.engine.displays().changed(row.environment_id).await;
        }
        reason
    }

    fn input(&self, app: &App, ticket: &Ticket) {
        let now = app.clock.now();
        let mut sittings = lock(&self.sittings);
        sittings
            .entry((ticket.display, ticket.origin.user()))
            .and_modify(|s| s.last = now)
            .or_insert(Sitting {
                from: now,
                last: now,
            });
    }

    fn start_keeper(&self, app: &App) {
        if self.keeper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let sockets = self.clone();
        let app = Arc::downgrade(app);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(KEEPER_TICK);
            loop {
                tick.tick().await;
                let Some(app) = app.upgrade() else { break };
                if app.realtime.inner.stop.is_cancelled() {
                    break;
                }
                let _ = app.engine.displays().keep().await;
                let displays: BTreeSet<_> = lock(&sockets.viewers)
                    .keys()
                    .map(|(display, _)| *display)
                    .chain(lock(&sockets.published).keys().copied())
                    .collect();
                for display in displays {
                    let state = ViewerState {
                        watching: sockets.watching(display),
                        driving: sockets.driving(display, app.clock.now()),
                    };
                    let changed = {
                        let mut published = lock(&sockets.published);
                        let changed = published.get(&display) != Some(&state);
                        if state.watching == 0 && state.driving.is_empty() {
                            published.remove(&display);
                        } else {
                            published.insert(display, state);
                        }
                        changed
                    };
                    if changed
                        && let Ok(mut conn) = app.db.acquire().await
                        && let Ok(Some(row)) = rows::get(&mut conn, display).await
                    {
                        drop(conn);
                        app.engine.displays().changed(row.environment_id).await;
                    }
                }
                let now = app.clock.now();
                let ended: Vec<_> = {
                    let mut sittings = lock(&sockets.sittings);
                    let keys: Vec<_> = sittings
                        .iter()
                        .filter(|(_, s)| s.last + SITTING_GAP <= now)
                        .map(|(key, _)| *key)
                        .collect();
                    keys.into_iter()
                        .filter_map(|key| sittings.remove(&key).map(|s| (key, s)))
                        .collect()
                };
                for ((display, user), sitting) in ended {
                    if app
                        .engine
                        .displays()
                        .sitting(display, user, sitting.from, sitting.last)
                        .await
                        .is_err()
                    {
                        let mut pending = lock(&sockets.sittings);
                        pending
                            .entry((display, user))
                            .and_modify(|current| {
                                current.from = current.from.min(sitting.from);
                                current.last = current.last.max(sitting.last);
                            })
                            .or_insert(sitting);
                    }
                }
            }
        });
    }
}

struct Header {
    replies: Vec<Bytes>,
    shared: bool,
    data: Bytes,
}
struct Handshake {
    pending: BytesMut,
    phase: u8,
    server_minor: u8,
    client_minor: u8,
}
impl Handshake {
    fn new(version: [u8; 12]) -> Result<Self, ProtocolError> {
        let server_minor = minor(&version)?;
        Ok(Self {
            pending: BytesMut::new(),
            phase: 0,
            server_minor,
            client_minor: 0,
        })
    }
    fn ready(&self) -> bool {
        self.phase == 3
    }
    fn receive(&mut self, data: &[u8]) -> Result<Header, ProtocolError> {
        if data.len() > MAX_INPUT || self.pending.len() + data.len() > 2 * MAX_INPUT {
            return Err(ProtocolError);
        }
        self.pending.extend_from_slice(data);
        let mut replies = Vec::new();
        let mut shared = false;
        loop {
            match self.phase {
                0 => {
                    if self.pending.len() < 12 {
                        break;
                    }
                    let version = self.pending.split_to(12);
                    self.client_minor = minor(&version)?;
                    if self.client_minor > self.server_minor {
                        return Err(ProtocolError);
                    }
                    if self.client_minor == 3 {
                        replies.push(Bytes::from_static(&[0, 0, 0, 1]));
                        self.phase = 2;
                    } else {
                        replies.push(Bytes::from_static(&[1, 1]));
                        self.phase = 1;
                    }
                }
                1 => {
                    if self.pending.is_empty() {
                        break;
                    }
                    if self.pending.split_to(1)[0] != 1 {
                        return Err(ProtocolError);
                    }
                    if self.client_minor == 8 {
                        replies.push(Bytes::from_static(&[0, 0, 0, 0]));
                    }
                    self.phase = 2;
                }
                2 => {
                    if self.pending.is_empty() {
                        break;
                    }
                    if !matches!(self.pending.split_to(1)[0], 0 | 1) {
                        return Err(ProtocolError);
                    }
                    shared = true;
                    self.phase = 3;
                }
                _ => break,
            }
        }
        let data = if self.ready() {
            self.pending.split().freeze()
        } else {
            Bytes::new()
        };
        Ok(Header {
            replies,
            shared,
            data,
        })
    }
}
fn minor(version: &[u8]) -> Result<u8, ProtocolError> {
    match version {
        b"RFB 003.003\n" => Ok(3),
        b"RFB 003.007\n" => Ok(7),
        b"RFB 003.008\n" => Ok(8),
        _ => Err(ProtocolError),
    }
}

async fn attach_checked(
    ctx: Ctx,
    query: DisplayAttachQuery,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !ctx
        .headers
        .get(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.split(',').any(|p| p.trim() == DISPLAY_SUBPROTOCOL))
    {
        return Err(ApiError::new(
            ErrorCode::RequestInvalid,
            "the socket needs orch.display.v1",
        ));
    }
    let display = ctx.id("display_id")?;
    let sockets = ctx.app.realtime.displays().clone();
    let ticket = sockets
        .redeem(display, &query.ticket, ctx.now, ctx.http_surface()?)
        .ok_or_else(|| ApiError::new(ErrorCode::Gone, "the display ticket expired or was used"))?;
    let app = Arc::clone(&ctx.app);
    Ok(upgrade
        .protocols([DISPLAY_SUBPROTOCOL])
        .max_message_size(MAX_INPUT)
        .on_upgrade(move |socket| async move {
            sockets.socket(app, ticket, socket).await;
        }))
}
/// Upgrades only the declared protocol and a redeemed single-use ticket.
pub(crate) async fn attach(
    ctx: Ctx,
    query: DisplayAttachQuery,
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
impl DisplayActions for Actions {
    fn seal_password(&self, display: Id, password: &[u8]) -> Result<Vec<u8>, String> {
        crate::routes::resources::sealing::master_key()
            .map_err(|_| "display credential storage is unavailable".to_owned())?
            .seal(password, format!("display/{display}/password").as_bytes())
            .map_err(|_| "the display credential could not be sealed".to_owned())
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
            if !current.displays.is_empty() {
                app.realtime.displays().start_keeper(&app);
            }
            let _ = app
                .realtime
                .publish_list(
                    Feed::EnvDisplays(environment),
                    serde_json::json!({"displays":current.displays}),
                )
                .await;
        }
    }
}

/// Installs display hooks once when the application handle is complete.
pub(crate) fn install(app: &App) {
    let sockets = app.realtime.displays();
    if sockets.installed.swap(true, Ordering::AcqRel) {
        return;
    }
    app.engine.install_display_actions(Arc::new(Actions {
        app: Arc::downgrade(app),
    }));
}
/// Recovers unattended display registrations through the existing screen-discovery path.
pub(crate) fn recover(app: &App) {
    let weak = Arc::downgrade(app);
    tokio::spawn(async move {
        let Some(app) = weak.upgrade() else { return };
        let Ok(mut conn) = app.db.acquire().await else {
            return;
        };
        let Ok(live) = rows::live(&mut conn).await else {
            return;
        };
        drop(conn);
        if live.is_empty() {
            return;
        }
        let environments: BTreeSet<_> = live.iter().map(|r| r.environment_id).collect();
        for environment in environments {
            let _ = app.engine.browsers().look(environment).await;
        }
        app.realtime.displays().start_keeper(&app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::authentication::Via;
    use orch_authz::Caller;
    #[test]
    fn surface_ticket_foreign_redemption_cannot_consume_either_origin() {
        use orch_api_contract::surface::HttpSurface;
        for surface in [HttpSurface::Browser, HttpSurface::Key] {
            let user = orch_types::new_id();
            let target = orch_types::new_id();
            let now = Utc::now();
            let token = "synthetic-transport-ticket";
            let token_id = orch_types::new_id();
            let (caller, via) = match surface {
                HttpSurface::Browser => (
                    Caller::session(user),
                    Via::Session {
                        session_id: orch_types::new_id(),
                    },
                ),
                HttpSurface::Key => {
                    let grant = orch_authz::TokenGrant::from_stored_scope(
                        &["*".to_owned()],
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                    (
                        Caller::token(user, grant.for_token(token_id)),
                        Via::Token {
                            token_id,
                            scopes: vec!["*".to_owned()],
                            node_id: None,
                            operation_scope: None,
                            expires_at: None,
                        },
                    )
                }
            };
            let sockets = Sockets::default();
            lock(&sockets.tickets).insert(
                token_hash(token),
                Ticket {
                    surface,
                    display: target,
                    environment: orch_types::new_id(),
                    machine: orch_types::new_id(),
                    generation: 1,
                    origin: Origin::from_caller(&caller, &via).unwrap(),
                    control: false,
                    expires: now + Span::seconds(30),
                },
            );
            let foreign = if surface == HttpSurface::Browser {
                HttpSurface::Key
            } else {
                HttpSurface::Browser
            };
            assert!(sockets.redeem(target, token, now, foreign).is_none());
            let kept = sockets
                .redeem(target, token, now, surface)
                .expect("wrong plane must leave the original ticket");
            assert_eq!(kept.surface, surface);
            assert_eq!(kept.origin.user(), user);
            assert!(!kept.control);
            assert!(
                sockets.redeem(target, token, now, surface).is_none(),
                "same-plane use remains single-use"
            );
        }
    }

    #[test]
    fn a_viewer_is_asked_for_no_password_even_when_its_messages_are_split() {
        let mut handshake = Handshake::new(*b"RFB 003.008\n").unwrap();
        let mut replies = Vec::new();
        let mut shared = false;
        for byte in b"RFB 003.008\n".iter().chain([1, 0].iter()) {
            let header = handshake.receive(&[*byte]).unwrap();
            replies.extend(header.replies);
            shared |= header.shared;
        }
        assert_eq!(
            replies,
            vec![
                Bytes::from_static(&[1, 1]),
                Bytes::from_static(&[0, 0, 0, 0])
            ]
        );
        assert!(shared && handshake.ready());
    }
    #[test]
    fn a_false_security_choice_ends_the_connection_instead_of_becoming_input() {
        let mut handshake = Handshake::new(*b"RFB 003.008\n").unwrap();
        handshake.receive(b"RFB 003.008\n").unwrap();
        assert!(handshake.receive(&[2, 1, 4, 1, 0, 0, 0, 0, 0, 65]).is_err());
    }
}

/// Test-only observation of the ACTUAL stored HTTP ticket and ordinary redemption predicate.
/// No synthetic caller, metadata constructor or alternate authorization is installed.
#[cfg(test)]
pub(super) async fn ticket_scope_probe(
    app: &App,
    target: Id,
    token: &str,
) -> ApiResult<Option<(bool, bool)>> {
    let sockets = app.realtime.displays();
    let Some(ticket) = sockets.redeem(
        target,
        token,
        app.clock.now(),
        orch_api_contract::surface::HttpSurface::Key,
    ) else {
        return Ok(None);
    };
    let (view, control, _, _) = Sockets::gates(app, &ticket).await?;
    Ok(Some((view, control)))
}

/// Test-only producer uses the ACTUAL HTTP-issued ticket and unchanged native gates.
#[cfg(test)]
pub(super) type InputFixture = Box<
    dyn FnMut() -> futures::future::BoxFuture<'static, ApiResult<Option<CurrentCaller>>>
        + Send
        + 'static,
>;
#[cfg(test)]
pub(super) fn ticket_input_fixture(app: App, id: Id, token: &str) -> Option<InputFixture> {
    let ticket = app.realtime.displays().redeem(
        id,
        token,
        app.clock.now(),
        orch_api_contract::surface::HttpSurface::Key,
    )?;
    Some(Box::new(
        move || -> futures::future::BoxFuture<'static, ApiResult<Option<CurrentCaller>>> {
            let app = app.clone();
            let ticket = ticket.clone();
            Box::pin(async move {
                let (view, control, _, current) = Sockets::gates(&app, &ticket).await?;
                Ok((view && control).then_some(current).flatten())
            })
        },
    ))
}
