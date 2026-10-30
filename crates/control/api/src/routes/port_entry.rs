//! Entering a port without the browser route: a link's use, its landing, the raw socket and the public assertion keys.
//!
//! A person's helper gets a use of a port with their own sign-in (`POST …/ports/{port}/use`); a
//! link's holder gets one with the link (`POST /port-links/use`), by a way in the link has. Either
//! use is then carried over one WebSocket (`GET /ws/ports/{use_id}`): TCP bytes in binary messages,
//! the end of one direction as the text message `eof`, or one UDP datagram per binary message. A
//! link's landing is a generic page of the web application, which reads the proof from the
//! address's fragment and asks `POST /port-links/inspect` for what may be shown. The public keys
//! that verify a caller's assertion are served to anybody: they are public.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::ports_runtime::BrowserEntry;
use crate::registry::{Ctx, Registry};
use crate::routes::ports::{
    address, entries, entry_proof, environment, listening, may_use, may_use_link, observed, parse,
    parse_link, proof_matches,
};
use axum::body::Body;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http::{HeaderValue, StatusCode, header};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ports::{self as api, *};
use orch_db::queries::ports as rows;
use orch_engine::ports as core;
use orch_link::tunnels::channel::Tunnel;
use orch_pki::port_assertions::{AUDIENCE_TEMPLATE, PortAssertionSigner};
use orch_types::{Time, new_id};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The WebSocket subprotocol of a port's raw socket.
pub const PORT_SUBPROTOCOL: &str = "orch.port.v1";
/// The text message that ends one TCP direction while the other goes on.
pub const END_OF_DIRECTION: &str = "eof";
/// The most bytes of one message of the socket: a port's TCP buffer, so that one message never
/// holds more than a direction may have in flight.
const SOCKET_MESSAGE_BYTES: usize = 262_144;
/// The most bytes read from the inside connection into one message: a few slow-link frames, so a
/// message is sent on before it waits long for more.
const SOCKET_CHUNK_BYTES: usize = 65_536;
/// How often a socket notices that its use ended (the use's admission is rechecked at the
/// runtime's tick; this is the same quarter second).
const ENDED_CHECK: Duration = Duration::from_millis(250);

static SIGNER: OnceLock<PortAssertionSigner> = OnceLock::new();

/// Gives the installation's port assertion key, for composition and for tests. The first key given
/// stays.
pub fn install_port_signer(signer: PortAssertionSigner) -> bool {
    SIGNER.set(signer).is_ok()
}

/// The port assertion key: the one given, or else one made now for this process alone, which can
/// no longer vouch for what it signed once the process ends (a deployment gives its own).
pub(crate) fn port_signer() -> &'static PortAssertionSigner {
    SIGNER.get_or_init(|| {
        tracing::warn!(
            "the installation gave no port assertion key; assertions made now cannot be verified after a restart"
        );
        PortAssertionSigner::generate()
    })
}

/// Binds the entries that do not go through the browser route.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::PortAssertionKeysGet, keys)
        .on(api::PortLinkUseCreate, link_use)
        .on(api::PortLinkInspect, inspect)
        .on(api::EnvironmentPortEnter, enter)
        .on_raw(api::PortLinkLanding, landing)
        .on_socket(api::PortUseSocket, socket);
}

/// `GET /ports/assertion-keys`: the public verification keys and their audience template.
async fn keys(ctx: Ctx, (): (), (): ()) -> ApiResult<PortAssertionKeys> {
    Ok(assertion_keys(&ctx.app))
}

/// Public verification context shared by HTTP and the authenticated inside read path.
pub(crate) fn assertion_keys(app: &crate::state::App) -> PortAssertionKeys {
    let signer = port_signer();
    PortAssertionKeys {
        issuer: app.config.system.public_url.clone(),
        audience_template: AUDIENCE_TEMPLATE.to_owned(),
        keys: vec![PortAssertionKey {
            kty: PortAssertionKeyType::Okp,
            crv: PortAssertionCurve::Ed25519,
            alg: PortAssertionAlgorithm::Ed25519,
            r#use: PortAssertionKeyUse::Sig,
            kid: signer.kid().to_owned(),
            x: signer.public_key(),
        }],
    }
}

/// The link's opening, when `link` is its whole and current proof; nothing tells an unknown
/// opening from a wrong proof.
async fn opening_of(conn: &mut orch_db::Conn, link: &str) -> ApiResult<rows::Opening> {
    let (opening, proof) = parse_link(link)?;
    rows::opening(conn, opening)
        .await?
        .filter(|row| row.kind == "link" && proof_matches(row, &proof))
        .ok_or_else(ApiError::not_found)
}

/// `POST /port-links/use`: a link's holder takes one use of its port, by a way in and a transport
/// the link has. Its first use of a day is the day's one use mark.
async fn link_use(ctx: Ctx, (): (), body: PortLinkUseCreateBody) -> ApiResult<PortUse> {
    let now = Time::new(ctx.now);
    let mut tx = ctx.app.db.begin().await?;
    let row = opening_of(&mut tx, &body.link).await?;
    let method = match body.entry_method {
        PortLinkMethod::Browser => "browser",
        PortLinkMethod::Helper => "helper",
    };
    if !row.entry_methods.iter().any(|word| word == method) {
        return Err(ApiError::new(
            ErrorCode::PortsNotAllowed,
            "this link does not admit that way in",
        ));
    }
    let env = environment(&mut tx, row.environment_id).await?;
    let port = u16::try_from(row.port).map_err(|_| ApiError::not_found())?;
    may_use_link(&mut tx, &env, &row, body.transport, now).await?;
    listening(&ctx, &env, port, body.transport)?;
    core::link_used(&mut tx, row.id, now)
        .await
        .map_err(ApiError::internal)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let expires =
        Time::new(now.get() + chrono::Duration::seconds(crate::ports_runtime::USE_SECONDS));
    ctx.app.cache.ports().admit_link(
        &ctx,
        &env,
        port,
        body.transport,
        row.id,
        entry_proof(),
        expires,
        new_id(),
    )
}

/// `POST /port-links/inspect`: what a link's landing may show, read with its proof, without a use,
/// a ticket, a connection, a use mark or a line of the audit. A lapsed or closed link shows nothing.
async fn inspect(ctx: Ctx, (): (), body: PortLinkInspectBody) -> ApiResult<PortLinkInfo> {
    let mut conn = ctx.app.db.acquire().await?;
    let row = opening_of(&mut conn, &body.link).await?;
    let until = row
        .until_at
        .filter(|end| *end > ctx.now)
        .ok_or_else(ApiError::not_found)?;
    if matches!(row.state.as_str(), "closed" | "refused" | "waiting") {
        return Err(ApiError::not_found());
    }
    let env = environment(&mut conn, row.environment_id).await?;
    let port = u16::try_from(row.port).map_err(|_| ApiError::not_found())?;
    let methods = row
        .entry_methods
        .iter()
        .map(|word| parse::<PortEntryMethod>(word))
        .collect::<ApiResult<Vec<_>>>()?;
    let transports = row
        .transports
        .iter()
        .map(|word| parse::<PortTransport>(word))
        .collect::<ApiResult<Vec<_>>>()?;
    let (program, _) = rows::program(&mut conn, env.id, port).await?;
    let snapshot = observed(&ctx, &env);
    let serving = snapshot.discovery == PortDiscovery::Native
        && snapshot
            .listeners
            .iter()
            .any(|l| l.port == port && !l.internal && l.unavailable_reason.is_none());
    let state = if env.status != "running" {
        PortOpeningState::Stopped
    } else if serving {
        PortOpeningState::Open
    } else {
        parse::<PortOpeningState>(&row.state)?
    };
    Ok(PortLinkInfo {
        port,
        label: row.label.clone(),
        until: Time::new(until),
        state,
        entries: entries(
            &ctx,
            env.id,
            port,
            &methods,
            &transports,
            PortEntryNeeds::Link,
            &program,
        ),
        entry_methods: methods,
    })
}

/// `POST /environments/{id}/ports/{port}/enter`: a signed-in person's single-use way into this port's
/// stable name, at a browser, as a holder of its door or a viewer of the named place it was opened
/// to. The code lasts as long as a use and admits no other port.
async fn enter(
    ctx: Ctx,
    (): (),
    body: EnvironmentPortEnterBody,
) -> ApiResult<EnvironmentPortEnterReply> {
    let id = ctx.id("id")?;
    let port: u16 = ctx
        .param("port")?
        .parse()
        .map_err(|_| ApiError::not_found())?;
    let address = address(&ctx, id, port).ok_or_else(|| {
        ApiError::new(
            ErrorCode::PortsUnavailable,
            "this installation has no ports domain: use the helper, or Here",
        )
    })?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    if !guard.user().is_session() {
        return Err(ApiError::new(
            ErrorCode::AuthSessionRequired,
            "a port's name is entered from a browser session, not with a token",
        ));
    }
    let env = environment(&mut tx, id).await?;
    may_use(
        &mut tx,
        guard.principal(),
        &env,
        port,
        PortTransport::Tcp,
        body.opening_id,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let now = guard.now();
    let expires_at =
        Time::new(now.get() + chrono::Duration::seconds(crate::ports_runtime::USE_SECONDS));
    let code = ctx.app.cache.ports().enter_code(
        BrowserEntry {
            person: Some(guard.principal().user()),
            identity: Some(crate::middleware::authorization::RetainedIdentity::of(
                guard.user(),
            )),
            environment: env.id,
            generation: env.generation,
            port,
            opening: body.opening_id,
            expires: expires_at,
        },
        now,
    );
    Ok(EnvironmentPortEnterReply {
        url: format!("{address}{}?code={code}", crate::ports_browser::ENTER_PATH),
        expires_at,
    })
}

/// `GET /ports/links/{opening_id}`: the landing of a link, which is the same generic page for every
/// address. The proof is in the fragment, which a browser keeps across this redirect and never
/// sends; the page reads it and asks `POST /port-links/inspect`.
async fn landing(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let opening = ctx.id("opening_id")?;
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    let location = format!("/ports/link/{opening}");
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&location).map_err(ApiError::internal)?,
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// `GET /ws/ports/{use_id}`: the raw socket of one admitted use. The use is taken, and its channel
/// opened, before the upgrade, so that a refused or spent use is an ordinary answer.
async fn socket(ctx: Ctx, query: PortUseSocketQuery, upgrade: WebSocketUpgrade) -> Response {
    let id = match ctx.id("use_id") {
        Ok(id) => id,
        Err(error) => return error.into_response_for(&ctx.request_id),
    };
    let Some(transport) = ctx.app.cache.ports().pending_transport(id) else {
        return ApiError::new(ErrorCode::PortsNotAllowed, "the use is unknown or spent")
            .into_response_for(&ctx.request_id);
    };
    // Behind the edge the caller's own port is not forwarded; the address is.
    let outside = SocketAddr::new(ctx.remote.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), 0);
    match ctx
        .app
        .cache
        .ports()
        .open_channel(id, &query.ticket, outside)
        .await
    {
        Ok(tunnel) => upgrade
            .protocols([PORT_SUBPROTOCOL])
            .max_message_size(SOCKET_MESSAGE_BYTES)
            .max_frame_size(SOCKET_MESSAGE_BYTES)
            .on_upgrade(move |socket| async move {
                match transport {
                    PortTransport::Tcp => carry_tcp(socket, tunnel).await,
                    PortTransport::Udp => carry_udp(socket, tunnel).await,
                }
            }),
        Err(error) => error.into_response_for(&ctx.request_id),
    }
}

/// Carries one TCP connection: each direction ends by itself (the caller's `eof` half-closes the
/// inside connection; the inside end's close is told with `eof`), and the use's end closes both.
async fn carry_tcp(socket: WebSocket, tunnel: Tunnel) {
    let closer = tunnel.closer();
    let (mut sink, mut stream) = socket.split();
    let (mut reading, mut writing) = tokio::io::split(tunnel);
    let inbound = async {
        let mut tick = tokio::time::interval(ENDED_CHECK);
        loop {
            tokio::select! {
                message = stream.next() => match message {
                    Some(Ok(Message::Binary(bytes))) => {
                        if writing.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) if text.as_str() == END_OF_DIRECTION => {
                        let _ = writing.shutdown().await;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
                _ = tick.tick() => if closer.is_closed() {
                    break;
                },
            }
        }
        let _ = writing.shutdown().await;
    };
    let outbound = async {
        let mut buffer = vec![0u8; SOCKET_CHUNK_BYTES];
        loop {
            match reading.read(&mut buffer).await {
                Ok(0) | Err(_) => {
                    let _ = sink
                        .send(Message::Text(Utf8Bytes::from_static(END_OF_DIRECTION)))
                        .await;
                    break;
                }
                Ok(read) => {
                    let bytes = Bytes::copy_from_slice(&buffer[..read]);
                    if sink.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
            }
        }
        sink
    };
    let ((), mut sink) = tokio::join!(inbound, outbound);
    let _ = sink.close().await;
    closer.close();
}

/// Carries one UDP sender association: one datagram a binary message, either way, an empty one
/// included; the association ends with the socket or the use.
async fn carry_udp(socket: WebSocket, tunnel: Tunnel) {
    let (mut sink, mut stream) = socket.split();
    let inbound = async {
        while let Some(Ok(message)) = stream.next().await {
            match message {
                Message::Binary(bytes) => {
                    if tunnel.send_datagram(bytes).is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let outbound = async {
        while let Ok(datagram) = tunnel.recv_datagram().await {
            if sink.send(Message::Binary(datagram)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    };
    tokio::select! {
        () = inbound => {}
        () = outbound => {}
    }
    tunnel.closer().close();
}
