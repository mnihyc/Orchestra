//! Real native tunnel and browser/RFB socket current-scope controls, without vendor UI claims.

use super::fixture as tickets;
#[path = "../../tests/browsers_support/mod.rs"]
mod browsers_support;
#[path = "../../tests/displays_support/mod.rs"]
mod displays_support;
use crate::rig::Scene;
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use orch_api_core::{registry::Registry, state::App};
use orch_link::tunnels::channel::{IncomingTunnels, Tunnel};
use orch_link_contract::tunnels::{
    ExecMessage, ExecOpcode, MIN_DATA_FRAME_BYTES, OpenErrorCode, Service,
};
use orch_realtime_contract::browser::{BrowserPeople, BrowserServerFrame, Tab, Viewport};
use orch_types::{Id, Json};
use serde_json::json;
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError},
};
use tickets::{Proof, Surface};
use tokio::{
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}
fn tag(surface: Surface) -> u8 {
    match surface {
        Surface::Browser => 1,
        Surface::Display => 2,
    }
}
fn tabs(title: &str) -> Vec<Tab> {
    vec![Tab {
        id: "owned".into(),
        title: title.into(),
        url: "https://example.test/owned".into(),
        loading: false,
        active: true,
        opener: None,
    }]
}
fn exec(frame: BrowserServerFrame) -> Vec<u8> {
    let message = ExecMessage {
        opcode: ExecOpcode::Text,
        payload: Bytes::from(json!({"frame":frame}).to_string()),
    };
    let mut bytes = BytesMut::new();
    message.write(&mut bytes).unwrap();
    bytes.to_vec()
}

#[derive(Default)]
struct Native {
    registered: Mutex<BTreeSet<(u8, Id)>>,
    opened: Mutex<Vec<(u8, Id)>>,
    publishers: Mutex<HashMap<Id, mpsc::UnboundedSender<Vec<u8>>>>,
    inputs: Mutex<HashMap<Id, Vec<Json>>>,
    rfb_keys: Mutex<HashMap<Id, usize>>,
    version_holds: Mutex<HashMap<Id, (oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
impl Native {
    fn send(&self, id: Id, bytes: Vec<u8>) {
        // A correct proactive authority close may already have retired its receiver.
        // The caller still requires the actual revoked close and forbids native payload.
        let _ = lock(&self.publishers)
            .get(&id)
            .expect("actual native channel was established by the positive")
            .send(bytes);
    }
    fn input_count(&self, id: Id) -> usize {
        lock(&self.inputs).get(&id).map_or(0, |frames| {
            frames.iter().filter(|frame| frame["op"] == "text").count()
        })
    }
    fn key_count(&self, id: Id) -> usize {
        lock(&self.rfb_keys).get(&id).copied().unwrap_or(0)
    }
    fn hold_version(&self, id: Id) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, waiting) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        assert!(
            lock(&self.version_holds)
                .insert(id, (reached, resume))
                .is_none()
        );
        (waiting, release)
    }
    async fn browser(
        &self,
        id: Id,
        tunnel: Tunnel,
        mut outgoing: mpsc::UnboundedReceiver<Vec<u8>>,
        stop: CancellationToken,
    ) -> Proof {
        let (mut reader, mut writer) = tokio::io::split(tunnel);
        writer
            .write_all(&exec(BrowserServerFrame::Ready {
                control: true,
                tabs: tabs("native-positive"),
                viewport: Viewport {
                    width: 320,
                    height: 200,
                },
                people: BrowserPeople {
                    watching: vec![],
                    driving: vec![],
                },
            }))
            .await?;
        let mut pending = BytesMut::new();
        let mut bytes = vec![0; MIN_DATA_FRAME_BYTES];
        loop {
            tokio::select! {
                ()=stop.cancelled()=>break,
                data=outgoing.recv()=>{let Some(data)=data else{break};if writer.write_all(&data).await.is_err(){break}},
                result=reader.read(&mut bytes)=>{
                    let Ok(n)=result else{break};if n==0{break}pending.extend_from_slice(&bytes[..n]);
                    while let Some(message)=ExecMessage::take(&mut pending,usize::try_from(orch_link_contract::version::Limits::default().max_frame_bytes)?)? {
                        if message.opcode!=ExecOpcode::Text{return Err("unexpected native browser opcode".into())}
                        let frame:Json=serde_json::from_slice(&message.payload)?;
                        lock(&self.inputs).entry(id).or_default().push(frame.clone());
                        if frame["op"]=="watch" {
                            let title=format!("fence-{}",frame["width"]);
                            if writer.write_all(&exec(BrowserServerFrame::Tabs{tabs:tabs(&title)})).await.is_err(){return Ok(())}
                        }
                    }
                }
            }
        }
        Ok(())
    }
    async fn rfb(
        &self,
        id: Id,
        mut tunnel: Tunnel,
        mut outgoing: mpsc::UnboundedReceiver<Vec<u8>>,
        stop: CancellationToken,
    ) -> Proof {
        let hold = lock(&self.version_holds).remove(&id);
        if let Some((reached, resume)) = hold {
            let _ = reached.send(());
            tokio::select! {_=resume=>{},()=stop.cancelled()=>return Ok(())}
        }
        tunnel.write_all(b"RFB 003.008\n").await?;
        let mut shared = [0; 1];
        tunnel.read_exact(&mut shared).await?;
        assert_eq!(shared, [1], "real API must force shared native RFB");
        let name = b"owned-native-display";
        let mut init = vec![
            0, 1, 0, 1, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0,
        ];
        init.extend(u32::try_from(name.len())?.to_be_bytes());
        init.extend(name);
        tunnel.write_all(&init).await?;
        let (mut reader, mut writer) = tokio::io::split(tunnel);
        loop {
            let mut kind = [0; 1];
            tokio::select! {
                ()=stop.cancelled()=>break,
                data=outgoing.recv()=>{let Some(data)=data else{break};if writer.write_all(&data).await.is_err(){break}},
                result=reader.read_exact(&mut kind)=>{
                    if result.is_err(){break}
                    match kind[0]{
                        2=>{let mut head=[0;3];reader.read_exact(&mut head).await?;let n=usize::from(u16::from_be_bytes([head[1],head[2]]));let mut encodings=vec![0;4*n];reader.read_exact(&mut encodings).await?;},
                        3=>{let mut request=[0;9];reader.read_exact(&mut request).await?;writer.write_all(&[0,0,0,0]).await?;},
                        4=>{let mut key=[0;7];reader.read_exact(&mut key).await?;*lock(&self.rfb_keys).entry(id).or_default()+=1;},
                        _=>return Err("undeclared RFB fixture client message".into()),
                    }
                }
            }
        }
        Ok(())
    }
}

struct Wire {
    scene: Scene,
    surface: Surface,
    app: App,
    address: SocketAddr,
    native: Arc<Native>,
    stop: CancellationToken,
    native_task: Option<JoinHandle<()>>,
    server: Option<JoinHandle<()>>,
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = &self.native_task {
            task.abort()
        }
        if let Some(task) = &self.server {
            task.abort()
        }
    }
}
impl Wire {
    async fn new(surface: Surface) -> Proof<Self> {
        let scene = tickets::scene(surface).await?;
        let app = scene.rig.world.app.app.clone();
        let stop = CancellationToken::new();
        scene
            .rig
            .hub
            .set_ticket_verifier(app.link.tickets().verifier());
        match surface {
            Surface::Browser => app.realtime.browsers().through(scene.rig.hub.terminals()),
            Surface::Display => app.realtime.displays().through(scene.rig.hub.terminals()),
        }
        let native = Arc::new(Native::default());
        let state = native.clone();
        let ended = stop.clone();
        let mut incoming: IncomingTunnels = scene.rig.machine.link.incoming_tunnels();
        let native_task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    ()=ended.cancelled()=>break,
                    tunnel=incoming.next()=>{let Some(tunnel)=tunnel else{break};let parsed=match tunnel.service.parse::<Service>(){
                        Ok(Service::Browser{browser_id})=>(1,browser_id),Ok(Service::Rfb{display_id})=>(2,display_id),_=>{tunnel.refuse(OpenErrorCode::NoSuchService,"not a declared fixture door");continue}
                    };if !lock(&state.registered).contains(&parsed){tunnel.refuse(OpenErrorCode::NoSuchService,"not an actual registered session");continue}
                    lock(&state.opened).push(parsed);let state=state.clone();let stop=ended.clone();let(send,receive)=mpsc::unbounded_channel();lock(&state.publishers).insert(parsed.1,send);
                    if let Ok(tunnel)=tunnel.accept(){tasks.spawn(async move{let _=if parsed.0==1{state.browser(parsed.1,tunnel,receive,stop).await}else{state.rfb(parsed.1,tunnel,receive,stop).await};});}
                    }
                }
            }
            tasks.shutdown().await;
        });
        app.realtime.start(scene.rig.engine.clone());
        app.realtime.listening().await;
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let router = orch_api_core::server::router(app.clone(), registry);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let ending = stop.clone();
        let server = tokio::spawn(async move {
            let _ = orch_api_core::server::serve_plain(listener, router, async move {
                ending.cancelled().await;
            })
            .await;
        });
        Ok(Self {
            scene,
            surface,
            app,
            address,
            native,
            stop,
            native_task: Some(native_task),
            server: Some(server),
        })
    }
    async fn make(&self) -> Proof<Id> {
        let id = self.surface.make(&self.scene).await?;
        lock(&self.native.registered).insert((tag(self.surface), id));
        Ok(id)
    }
    async fn mint(&self, id: Id, key: &tickets::Legacy, control: bool) -> Proof<String> {
        tickets::ticket(
            &tickets::request(&self.scene, self.surface, id, &key.secret, control).await,
            "actual legacy native-wire ticket HTTP premise",
        )
    }
    fn path(&self, id: Id, token: &str) -> String {
        let family = match self.surface {
            Surface::Browser => "browsers",
            Surface::Display => "displays",
        };
        format!("/openapi/v1/ws/{family}/{id}?ticket={token}")
    }
    async fn finish(mut self) {
        self.stop.cancel();
        self.app.realtime.stop().await;
        if let Some(task) = self.native_task.take() {
            let _ = task.await;
        }
        if let Some(task) = self.server.take() {
            let _ = task.await;
        }
    }
}

async fn browser_fence(screen: &mut browsers_support::Screen, width: u16) {
    screen
        .send(json!({"op":"watch","tab":"owned","width":width,"height":200,"dpr":1.0}))
        .await;
    let wanted = format!("fence-{width}");
    tokio::time::timeout(browsers_support::WAIT, async {
        loop {
            match screen.next_wire().await {
                Some(Message::Text(text)) => {
                    let value: Json = serde_json::from_str(&text).unwrap();
                    if value["op"] == "error" {
                        assert_eq!(
                            value["code"], "view_only",
                            "unexpected browser refusal: {value}"
                        );
                    }
                    if value["op"] == "tabs" && value["tabs"][0]["title"] == wanted {
                        break;
                    }
                    assert_ne!(
                        value["op"], "closed",
                        "original .control Read must continue through actual native fence"
                    );
                }
                Some(Message::Ping(_) | Message::Pong(_)) => {}
                other => panic!("native browser fence ended unexpectedly: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_browser_control_keeps_own_read_but_stops_native_input_and_driving_after_manage_loss()
-> Proof {
    let wire = Wire::new(Surface::Browser).await?;
    let id = wire.make().await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let token = wire.mint(id, &key, true).await?;
    let mut screen =
        browsers_support::Screen::connect_at(wire.address, &wire.path(id, &token)).await;
    assert_eq!(screen.text("ready").await["control"], true);
    screen
        .send(json!({"op":"text","tab":"owned","text":"native-positive"}))
        .await;
    browser_fence(&mut screen, 321).await;
    assert_eq!(
        wire.native.input_count(id),
        1,
        "actual control input must reach native before narrowing"
    );
    wire.scene.rig.world.advance(chrono::Duration::seconds(11));
    tickets::current_scope(
        &wire.scene.rig.world,
        key.id,
        tickets::rules(
            "BrowserTicketCreate.control",
            wire.scene.on.environment,
            false,
        ),
    )
    .await?;
    screen
        .send(json!({"op":"text","tab":"owned","text":"must-not-reach-native"}))
        .await;
    browser_fence(&mut screen, 322).await;
    let inputs = wire.native.input_count(id);
    let driving = wire
        .app
        .realtime
        .browsers()
        .driving(id, wire.app.clock.now());
    screen.close().await;
    wire.finish().await;
    assert_eq!(
        inputs, 1,
        "after real watch fence, narrowed control action cannot reach native"
    );
    assert!(driving.is_empty(), "a denied action cannot renew driving");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_display_control_keeps_own_read_but_stops_native_key_and_driving_after_manage_loss()
-> Proof {
    let wire = Wire::new(Surface::Display).await?;
    let id = wire.make().await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let token = wire.mint(id, &key, true).await?;
    let mut screen =
        displays_support::Screen::connect_at(wire.address, &wire.path(id, &token)).await;
    screen.send(&displays_support::key(65, true)).await;
    screen.picture_without_clipboard().await;
    assert_eq!(
        wire.native.key_count(id),
        1,
        "actual native key positive before narrowing"
    );
    wire.scene.rig.world.advance(chrono::Duration::seconds(11));
    tickets::current_scope(
        &wire.scene.rig.world,
        key.id,
        tickets::rules(
            "DisplayTicketCreate.control",
            wire.scene.on.environment,
            false,
        ),
    )
    .await?;
    screen.send(&displays_support::key(66, true)).await;
    screen.picture_without_clipboard().await;
    let keys = wire.native.key_count(id);
    let driving = wire
        .app
        .realtime
        .displays()
        .driving(id, wire.app.clock.now());
    screen.close().await;
    wire.finish().await;
    assert_eq!(
        keys, 1,
        "actual RFB read fence passed without forwarding narrowed native key"
    );
    assert!(
        driving.is_empty(),
        "denied native action cannot renew driving"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_browser_current_read_loss_closes_before_new_native_tab_content() -> Proof {
    let wire = Wire::new(Surface::Browser).await?;
    let id = wire.make().await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let token = wire.mint(id, &key, false).await?;
    let mut screen =
        browsers_support::Screen::connect_at(wire.address, &wire.path(id, &token)).await;
    assert_eq!(screen.text("ready").await["control"], false);
    browser_fence(&mut screen, 321).await;
    tickets::current_scope(
        &wire.scene.rig.world,
        key.id,
        tickets::rules(
            "BrowserTicketCreate.control",
            wire.scene.on.environment,
            true,
        ),
    )
    .await?;
    wire.native.send(
        id,
        exec(BrowserServerFrame::Tabs {
            tabs: tabs("PRIVATE AFTER OWN VIEW READ LOSS"),
        }),
    );
    let actual = tokio::time::timeout(browsers_support::WAIT, async {
        loop {
            match screen.next_wire().await {
                Some(Message::Text(text)) => {
                    let value: Json = serde_json::from_str(&text)?;
                    if value["op"] != "people" {
                        return Ok::<Json, Box<dyn std::error::Error + Send + Sync>>(value);
                    }
                }
                Some(Message::Ping(_) | Message::Pong(_)) => {}
                other => return Err(format!("unexpected actual browser frame: {other:?}").into()),
            }
        }
    })
    .await??;
    screen.close().await;
    wire.finish().await;
    assert_eq!(
        actual["op"], "closed",
        "current own .view Read loss must stop real native content: {actual}"
    );
    assert_eq!(actual["reason"], "revoked");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_display_current_read_loss_closes_before_new_native_pixel_bytes() -> Proof {
    let wire = Wire::new(Surface::Display).await?;
    let id = wire.make().await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let token = wire.mint(id, &key, false).await?;
    let mut screen =
        displays_support::Screen::connect_at(wire.address, &wire.path(id, &token)).await;
    screen.picture_without_clipboard().await;
    tickets::current_scope(
        &wire.scene.rig.world,
        key.id,
        tickets::rules(
            "DisplayTicketCreate.control",
            wire.scene.on.environment,
            true,
        ),
    )
    .await?;
    let mut pixel = vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0];
    pixel.extend([17, 34, 51, 68]);
    wire.native.send(id, pixel);
    let actual = tokio::time::timeout(browsers_support::WAIT, async {
        loop {
            match screen.next_wire().await {
                Some(Message::Ping(_) | Message::Pong(_)) => {}
                other => break other,
            }
        }
    })
    .await?;
    screen.close().await;
    wire.finish().await;
    assert!(
        matches!(actual, Some(Message::Close(Some(ref frame))) if frame.reason == "revoked"),
        "actual native pixels escaped or wrong close followed own Read loss: {actual:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_revoked_browser_ticket_never_runs_native_ensure_before_its_current_gate() -> Proof {
    let wire = Wire::new(Surface::Browser).await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let positive = wire.make().await?;
    let token = wire.mint(positive, &key, false).await?;
    let mut screen =
        browsers_support::Screen::connect_at(wire.address, &wire.path(positive, &token)).await;
    screen.text("ready").await;
    browser_fence(&mut screen, 321).await;
    screen.close().await;
    let id = wire.make().await?;
    let token = wire.mint(id, &key, false).await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key.id)
        .bind(wire.scene.rig.world.now())
        .execute(&wire.scene.rig.world.db.pool)
        .await?;
    let mut screen =
        browsers_support::Screen::connect_at(wire.address, &wire.path(id, &token)).await;
    let actual = screen.text("closed").await;
    let opened = lock(&wire.native.opened)
        .iter()
        .any(|(_, opened)| *opened == id);
    screen.close().await;
    wire.finish().await;
    assert_eq!(actual["op"], "closed");
    assert!(
        !opened,
        "revoked ticket opened real native browser door before checking current authority"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_display_rechecks_revoked_ticket_after_native_version_wait_before_first_bytes()
-> Proof {
    let wire = Wire::new(Surface::Display).await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let positive = wire.make().await?;
    let token = wire.mint(positive, &key, false).await?;
    let mut screen =
        displays_support::Screen::connect_at(wire.address, &wire.path(positive, &token)).await;
    screen.picture_without_clipboard().await;
    screen.close().await;
    let id = wire.make().await?;
    let token = wire.mint(id, &key, false).await?;
    let (reached, release) = wire.native.hold_version(id);
    let mut socket =
        displays_support::Screen::socket_at(wire.address, &wire.path(id, &token)).await;
    tokio::time::timeout(browsers_support::WAIT, reached).await??;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key.id)
        .bind(wire.scene.rig.world.now())
        .execute(&wire.scene.rig.world.db.pool)
        .await?;
    release
        .send(())
        .map_err(|_| "actual native version gate ended")?;
    let actual = tokio::time::timeout(browsers_support::WAIT, socket.next())
        .await?
        .transpose()?;
    let _ = socket.close(None).await;
    wire.finish().await;
    assert!(
        matches!(actual, Some(Message::Close(Some(ref frame))) if frame.reason == "revoked"),
        "native version bytes escaped or wrong close followed actual key loss during IO: {actual:?}"
    );
    Ok(())
}

// These two byte-framing regressions reuse the actual HTTP ticket/native gate fixture
// above. The bounded Tokio byte stream makes the accepted prefix observable without
// a new protocol or claims of desktop rendering; original wire6 covers the real tunnel.
struct ObservedWriter {
    inner: tokio::io::DuplexStream,
    pending: Option<oneshot::Sender<()>>,
}
impl tokio::io::AsyncWrite for ObservedWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let result = std::pin::Pin::new(&mut self.inner).poll_write(cx, bytes);
        if result.is_pending() {
            if let Some(pending) = self.pending.take() {
                let _ = pending.send(());
            }
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn actual_input_factory(
    wire: &Wire,
    id: Id,
    key: &tickets::Legacy,
) -> Proof<crate::realtime::displays::InputFixture> {
    let token = wire.mint(id, key, true).await?;
    crate::realtime::displays::ticket_input_fixture(wire.app.clone(), id, &token)
        .ok_or_else(|| "actual HTTP input ticket could not be consumed once".into())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_actual_started_rfb_message_completes_only_its_own_tail_after_manage_loss() -> Proof {
    let wire = Wire::new(Surface::Display).await?;
    let id = wire.make().await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let token = wire.mint(id, &key, true).await?;
    let mut positive =
        displays_support::Screen::connect_at(wire.address, &wire.path(id, &token)).await;
    positive.send(&displays_support::key(65, true)).await;
    positive.picture_without_clipboard().await;
    assert_eq!(
        wire.native.key_count(id),
        1,
        "actual same-door native input positive"
    );
    positive.close().await;
    let mut current = actual_input_factory(&wire, id, &key).await?;
    let message = displays_support::key(66, true);
    // One byte is exactly the physical start boundary, not a production buffer/timer.
    let (mut writing, mut reading) = tokio::io::duplex(1);
    let send = crate::realtime::publication::write_message(
        &mut writing,
        &message,
        wire.app.clock.as_ref(),
        &mut current,
    );
    let receive = async {
        let mut first = [0; 1];
        reading.read_exact(&mut first).await?;
        assert_eq!(
            first[0], message[0],
            "actual first byte was accepted before withdrawal"
        );
        tickets::current_scope(
            &wire.scene.rig.world,
            key.id,
            tickets::rules(
                "DisplayTicketCreate.control",
                wire.scene.on.environment,
                false,
            ),
        )
        .await?;
        let mut rest = vec![0; message.len() - 1];
        reading.read_exact(&mut rest).await?;
        let mut actual = first.to_vec();
        actual.extend(rest);
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(actual)
    };
    let (sent, actual) = tokio::time::timeout(browsers_support::WAIT, async {
        tokio::join!(send, receive)
    })
    .await?;
    assert!(matches!(sent?, Some(Ok(()))));
    assert_eq!(
        actual?, message,
        "only the exact already-started parser message may finish"
    );
    let next = displays_support::key(67, true);
    let denied = crate::realtime::publication::write_message(
        &mut writing,
        &next,
        wire.app.clock.as_ref(),
        &mut current,
    )
    .await?;
    assert!(
        denied.is_none(),
        "next message cannot borrow earlier first-byte acceptance"
    );
    writing.shutdown().await?;
    let mut extra = Vec::new();
    reading.read_to_end(&mut extra).await?;
    wire.finish().await;
    assert!(
        extra.is_empty(),
        "new action bytes followed the accepted frame without current Manage"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_actual_zero_byte_queued_rfb_message_rechecks_manage_after_real_pending() -> Proof {
    let wire = Wire::new(Surface::Display).await?;
    let id = wire.make().await?;
    let key = tickets::legacy(&wire.scene.rig.world, wire.scene.cai.id()).await?;
    let token = wire.mint(id, &key, true).await?;
    let mut positive =
        displays_support::Screen::connect_at(wire.address, &wire.path(id, &token)).await;
    positive.send(&displays_support::key(65, true)).await;
    positive.picture_without_clipboard().await;
    assert_eq!(
        wire.native.key_count(id),
        1,
        "actual native gate/input positive"
    );
    positive.close().await;
    let mut current = actual_input_factory(&wire, id, &key).await?;
    let prefix = displays_support::key(66, true);
    let message = displays_support::key(67, true);
    // Exactly one existing parser message fills the test byte-stream capacity.
    let (writing, mut reading) = tokio::io::duplex(prefix.len());
    let (pending, reached) = oneshot::channel();
    let mut writing = ObservedWriter {
        inner: writing,
        pending: None,
    };
    assert!(matches!(
        crate::realtime::publication::write_message(
            &mut writing,
            &prefix,
            wire.app.clock.as_ref(),
            &mut current
        )
        .await?,
        Some(Ok(()))
    ));
    writing.pending = Some(pending);
    let send = crate::realtime::publication::write_message(
        &mut writing,
        &message,
        wire.app.clock.as_ref(),
        &mut current,
    );
    let release = async {
        tokio::time::timeout(browsers_support::WAIT, reached).await??;
        tickets::current_scope(
            &wire.scene.rig.world,
            key.id,
            tickets::rules(
                "DisplayTicketCreate.control",
                wire.scene.on.environment,
                false,
            ),
        )
        .await?;
        let mut accepted = vec![0; prefix.len()];
        reading.read_exact(&mut accepted).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(accepted)
    };
    let (sent, accepted) = tokio::time::timeout(browsers_support::WAIT, async {
        tokio::join!(send, release)
    })
    .await?;
    assert!(
        sent?.is_none(),
        "Pending with zero accepted bytes must refresh original current Manage"
    );
    assert_eq!(accepted?, prefix);
    writing.shutdown().await?;
    let mut extra = Vec::new();
    reading.read_to_end(&mut extra).await?;
    wire.finish().await;
    assert!(
        extra.is_empty(),
        "zero-byte queued message was forwarded after Manage withdrawal"
    );
    Ok(())
}
