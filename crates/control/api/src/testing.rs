//! A typed client for tests: calls a declared endpoint on the application in memory, as a named caller.
//!
//! Tests of handlers go through the same router, binder and middleware
//! as a real request, without a socket. The endpoint's own types are the
//! arguments and the result, so a test reads like the call it checks.

use crate::backplane::Link;
use crate::registry::Registry;
use crate::state::{App, AppState};
use axum::body::Body;
use http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use orch_api_contract::common::{CSRF_HEADER, Problem};
use orch_api_contract::routes::{Endpoint, Shape};
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_db::Db;
use orch_types::{Clock, ManualClock};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tower::ServiceExt;

/// Replaces inside-machine reads for a fault-injection walk; the production link setter stays private.
pub fn machine_request_handler(app: &App, handler: Arc<dyn crate::machine_requests::Handler>) {
    app.link.serve_machine_requests(handler);
}

/// The application in memory.
#[derive(Debug, Clone)]
pub struct TestApp {
    /// Its state.
    pub app: App,
    /// Its clock, which stands still until the test moves it.
    pub clock: ManualClock,
    router: axum::Router,
}

/// What came back from a call.
#[derive(Debug)]
pub struct Reply<T> {
    /// The status.
    pub status: StatusCode,
    /// The headers.
    pub headers: HeaderMap,
    /// The reply, where the call succeeded.
    pub body: Result<T, Problem>,
    /// The body as it was sent.
    pub raw: bytes::Bytes,
}

impl<T: std::fmt::Debug> Reply<T> {
    /// The reply of a call that has to have succeeded.
    ///
    /// # Panics
    /// When the call was refused.
    #[expect(
        clippy::panic,
        reason = "test support: a refused call that had to succeed fails the test"
    )]
    #[track_caller]
    pub fn ok(self) -> T {
        match self.body {
            Ok(body) => body,
            // A detail is its code's catalogue sentence; what this refusal adds is in its members.
            Err(problem) => panic!(
                "expected success, got {} {}: {} {}",
                problem.status,
                problem.code,
                problem.detail,
                serde_json::Value::Object(problem.extra)
            ),
        }
    }

    /// The problem of a call that has to have been refused.
    ///
    /// # Panics
    /// When the call succeeded.
    #[expect(
        clippy::panic,
        reason = "test support: a call that had to be refused and was not fails the test"
    )]
    #[track_caller]
    pub fn problem(self) -> Problem {
        match self.body {
            Err(problem) => problem,
            Ok(_) => panic!("expected a refusal, got {} success", self.status),
        }
    }
}

/// What came back from a call whose reply is not JSON: a file, text, a stream.
#[derive(Debug)]
pub struct RawReply {
    /// The status.
    pub status: StatusCode,
    /// The headers.
    pub headers: HeaderMap,
    /// The body as it was sent.
    pub raw: bytes::Bytes,
}

impl RawReply {
    /// The problem of a call that had to be refused.
    ///
    /// # Panics
    /// When the call succeeded, or its answer is no problem.
    #[expect(
        clippy::expect_used,
        reason = "test support: a call that had to be refused and was not fails the test"
    )]
    #[track_caller]
    pub fn problem(&self) -> Problem {
        assert!(
            !self.status.is_success(),
            "expected a refusal, got {}",
            self.status
        );
        serde_json::from_slice(&self.raw).expect("a problem")
    }
}

/// A call being put together.
#[derive(Debug)]
pub struct Call<'a, E: Endpoint> {
    app: &'a TestApp,
    path: String,
    query: Option<String>,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    peer: Option<std::net::SocketAddr>,
    endpoint: std::marker::PhantomData<E>,
}

impl TestApp {
    /// The application over `db` with the requests of `registry`, a clock
    /// at `clock`, and configured test addresses with the other defaults.
    #[must_use]
    pub fn new(db: Db, clock: ManualClock, registry: Registry) -> Self {
        let mut config = ServerConfig::default();
        // An installed test system has configured addresses; shipped examples deliberately block enrollment.
        config.system.public_url = "https://orch.test".into();
        config.gateway.endpoints[0].url = "wss://gateway.test:9443".into();
        Self::with_config(db, clock, registry, config)
    }

    /// As [`TestApp::new`], with `config`, and no built-in source of a key's facts registered.
    #[must_use]
    pub fn with_config(
        db: Db,
        clock: ManualClock,
        registry: Registry,
        config: ServerConfig,
    ) -> Self {
        Self::with_sources(db, clock, registry, config, Arc::default())
    }

    /// As [`TestApp::with_config`], with the built-in sources of a key's facts that a program registers.
    #[must_use]
    pub fn with_sources(
        db: Db,
        clock: ManualClock,
        registry: Registry,
        config: ServerConfig,
        builtin_sources: Arc<orch_provider_contract::builtins::BuiltinSources>,
    ) -> Self {
        // The same key composition as the server, before constructing runtimes or reading data.
        // Current OnceLock semantics preserve a caller's explicitly installed test key.
        crate::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
        let config = Arc::new(config);
        let clock_handle = Arc::new(clock.clone()) as Arc<dyn Clock>;
        #[expect(
            clippy::expect_used,
            reason = "test support: an authority that cannot be made leaves no test to run"
        )]
        let link = Link::detached(db.clone(), Arc::clone(&clock_handle), Arc::clone(&config))
            .expect("a link without gateways can be made");
        let engine = crate::thread_engine::detached(&db, &clock_handle, &config, &link);
        let blobs = crate::blobs::Blobs::in_memory();
        engine.attach_blobs(blobs.store());
        let cache = crate::cache::Cache::new(&config);
        let realtime = crate::realtime::Realtime::new(&db, &clock_handle, &config);
        let app: App = Arc::new(AppState {
            host_ports: Default::default(),
            db,
            config,
            clock: clock_handle,
            link,
            engine,
            realtime,
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs,
            cache,
            builtin_sources,
        });
        Self::over(app, clock, registry)
    }

    /// The application over a state that was put together by the caller: for
    /// a test that stands something of its own behind the thread engine, such
    /// as a machine that answers the commands it sends.
    #[must_use]
    pub fn over(app: App, clock: ManualClock, registry: Registry) -> Self {
        crate::machine_requests::install(&app);
        crate::port_tools::install(&app);
        let router = registry.into_router().with_state(app.clone());
        Self { app, clock, router }
    }

    /// Applies a connected test machine's delivery through the backplane's transaction and acknowledgement.
    pub async fn deliver_machine_events(
        &self,
        machine: orch_types::Id,
        controller: &orch_link::controller::ControllerLink,
        delivery: orch_link::streams::Delivery,
    ) {
        let discover = delivery.stream == orch_link_contract::streams::Stream::Machine;
        if self
            .app
            .link
            .apply_delivery(machine, controller, delivery)
            .await
            && discover
        {
            // Discovery sends commands whose answers may themselves produce events. Keep receiving
            // those events while the descriptor is being learned, as the backplane's task does.
            let engine = self.app.engine.clone();
            tokio::spawn(async move {
                if let Err(error) = engine.discover_descriptors_after_facts(machine, true).await {
                    tracing::warn!(%machine, %error, "startup harness discovery failed");
                }
            });
        }
    }

    /// Begins a call of `endpoint`. `params` fill the path's parameters in
    /// order.
    #[must_use]
    pub fn call<E: Endpoint>(&self, endpoint: E, params: &[&dyn std::fmt::Display]) -> Call<'_, E> {
        let _ = endpoint;
        let mut values = params.iter();
        let path = E::ROUTE
            .path
            .split('/')
            .map(|part| {
                if part.starts_with('{') {
                    values.next().map(ToString::to_string).unwrap_or_default()
                } else {
                    part.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        Call {
            app: self,
            path,
            query: None,
            headers: HeaderMap::new(),
            body: None,
            peer: None,
            endpoint: std::marker::PhantomData,
        }
    }
}

impl<E: Endpoint> Call<'_, E> {
    /// Supplies the socket peer that a real server obtains from its accepted connection.
    #[must_use]
    pub fn peer(mut self, address: std::net::SocketAddr) -> Self {
        self.peer = Some(address);
        self
    }

    /// Calls with the bearer token `token`.
    #[must_use]
    pub fn token(mut self, token: &str) -> Self {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
            self.headers.insert(header::AUTHORIZATION, value);
        }
        self
    }

    /// Adds a header.
    #[must_use]
    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        if let Ok(value) = HeaderValue::from_str(value) {
            self.headers.append(name, value);
        }
        self
    }

    /// Sends the cookie `name`, with the header a cookie-only request needs.
    #[must_use]
    pub fn cookie(self, name: &str, value: &str) -> Self {
        self.header("cookie", &format!("{name}={value}"))
            .header(CSRF_HEADER, "1")
    }

    /// Sends `query` as the query string.
    #[must_use]
    pub fn query<Q: Serialize + ?Sized>(mut self, query: &Q) -> Self {
        self.query = serde_urlencoded::to_string(query).ok();
        self
    }

    /// Sends a query string as it is written, for what a typed query cannot say.
    #[must_use]
    pub fn raw_query(mut self, query: &str) -> Self {
        self.query = Some(query.to_owned());
        self
    }

    /// Sends `body` as JSON.
    #[must_use]
    pub fn body(mut self, body: &E::Body) -> Self
    where
        E::Body: Serialize,
    {
        self.body = serde_json::to_vec(body).ok();
        self
    }

    /// Sends any JSON as the body, for what a typed body cannot say.
    #[must_use]
    pub fn raw_body(mut self, body: &serde_json::Value) -> Self {
        self.body = serde_json::to_vec(body).ok();
        self
    }

    /// Sends bytes as the body, which are not JSON: a file, a piece of an upload. Their type is
    /// `application/octet-stream` unless the call already states another.
    #[must_use]
    pub fn bytes(mut self, body: &[u8]) -> Self {
        self.body = Some(body.to_vec());
        if !self.headers.contains_key(header::CONTENT_TYPE) {
            self.headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
        }
        self
    }

    /// Makes the call and gives the answer as it came, whatever the reply is.
    ///
    /// # Panics
    /// When the application does not answer at all.
    #[expect(
        clippy::expect_used,
        reason = "test support: an answer outside the contract fails the test"
    )]
    pub async fn send_bytes(self) -> RawReply {
        let response = self.send_response().await;
        let (parts, body) = response.into_parts();
        let raw = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("a body");
        RawReply {
            status: parts.status,
            headers: parts.headers,
            raw,
        }
    }

    /// Makes the call while leaving its streamed body available for page-by-page assertions.
    ///
    /// # Panics
    /// When the application does not answer at all.
    #[expect(
        clippy::expect_used,
        reason = "test support: a missing router answer fails the test"
    )]
    pub async fn send_response(self) -> axum::response::Response {
        let uri = match &self.query {
            Some(query) if !query.is_empty() => format!("{}?{query}", self.path),
            _ => self.path.clone(),
        };
        let mut request = Request::builder().method(E::ROUTE.method.as_str()).uri(uri);
        if let Some(peer) = self.peer {
            request = request.extension(axum::extract::ConnectInfo(peer));
        }
        if let Some(headers) = request.headers_mut() {
            headers.extend(self.headers);
            if self.body.is_some() && !headers.contains_key(header::CONTENT_TYPE) {
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
            }
        }
        let request = request
            .body(Body::from(self.body.unwrap_or_default()))
            .expect("a request");
        self.app
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("the router does not fail")
    }

    /// Makes the call.
    ///
    /// # Panics
    /// When the application answers with something that is neither the
    /// declared reply nor a problem.
    #[expect(
        clippy::expect_used,
        reason = "test support: an answer outside the contract fails the test"
    )]
    pub async fn send(self) -> Reply<E::Reply>
    where
        E::Reply: DeserializeOwned,
    {
        let RawReply {
            status,
            headers,
            raw,
        } = self.send_bytes().await;
        let body = if status.is_success() || status.is_redirection() {
            if matches!(E::ROUTE.reply, Shape::Json(_)) {
                Ok(serde_json::from_slice(&raw).expect("the declared reply"))
            } else {
                Ok(serde_json::from_value(serde_json::Value::Null).expect("an empty reply"))
            }
        } else {
            Err(serde_json::from_slice(&raw).expect("a problem"))
        };
        Reply {
            status,
            headers,
            body,
            raw,
        }
    }
}

// ---------------------------------------------------------------- a machine that is played

/// What a machine that a test plays does with the commands it is sent.
#[async_trait::async_trait]
pub trait Machine: Send + Sync + 'static {
    /// Delivers a stop request without pretending the held operation has already returned.
    fn cancel(&self, _command: orch_types::Id) {}
    /// Answers the command, as it is stored: with its result, or with why it failed.
    async fn run(
        &self,
        command: &orch_link_contract::commands::Cmd,
    ) -> Result<serde_json::Value, orch_link_contract::commands::CommandError>;
}

/// A command that a played machine was sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    /// Its identifier.
    pub id: orch_types::Id,
    /// Its kind.
    pub kind: orch_link_contract::commands::CommandKind,
    /// Its body, as the machine was sent it.
    pub body: serde_json::Value,
}

/// A machine played by a test: it answers the commands that wait for it in the database, one task
/// for each, and tells those who wait for them. It is stopped when it is dropped.
pub struct Played {
    seen: Arc<std::sync::Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for Played {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Played").finish_non_exhaustive()
    }
}

impl Drop for Played {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Played {
    /// Every command it was sent, in the order it took them up.
    #[must_use]
    pub fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The commands of a kind it was sent.
    #[must_use]
    pub fn seen_of(&self, kind: orch_link_contract::commands::CommandKind) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|each| each.kind == kind)
            .collect()
    }
}

impl TestApp {
    /// Plays the machine `machine`: from now on the commands issued for it are taken up and answered
    /// by `behaviour`, which may take as long as it likes, since each runs in a task of its own.
    #[must_use]
    pub fn play(&self, machine: orch_types::Id, behaviour: Arc<dyn Machine>) -> Played {
        use orch_db::queries::link as rows;
        let app = Arc::clone(&self.app);
        let seen: Arc<std::sync::Mutex<Vec<Seen>>> = Arc::default();
        let kept = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            let mut taken: std::collections::HashSet<orch_types::Id> =
                std::collections::HashSet::new();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                let Ok(mut conn) = app.db.acquire().await else {
                    continue;
                };
                let Ok(open) = rows::open_commands(&mut conn, machine).await else {
                    continue;
                };
                let stops: Vec<orch_types::Id> = sqlx::query_scalar("SELECT id FROM commands WHERE machine_id=$1 AND state IN ('failed','cancelled','expired') AND stop_confirmed_at IS NULL")
                    .bind(machine).fetch_all(&mut *conn).await.unwrap_or_default();
                for id in stops {
                    behaviour.cancel(id);
                    if !taken.contains(&id) {
                        let _ = sqlx::query("UPDATE commands SET stop_confirmed_at=$2 WHERE id=$1 AND machine_id=$3 AND state IN ('failed','cancelled','expired') AND stop_confirmed_at IS NULL")
                            .bind(id).bind(app.clock.now()).bind(machine).execute(&mut *conn).await;
                    }
                }
                drop(conn);
                for row in open {
                    // The open list predates stop delivery; never admit a row just confirmed stopped.
                    let still_open = match app.db.acquire().await {
                        Ok(mut conn) => match rows::command(&mut conn, row.id).await.ok().flatten()
                        {
                            Some(current) if current.state == "accepted" => true,
                            Some(current) if !current.is_over() => {
                                // Acceptance precedes native progress and removes the never-sent stop shortcut.
                                rows::mark_accepted(&mut conn, row.id, app.clock.now())
                                    .await
                                    .unwrap_or(false)
                            }
                            _ => false,
                        },
                        Err(_) => false,
                    };
                    if !still_open {
                        continue;
                    }
                    if !taken.insert(row.id) {
                        continue;
                    }
                    let Ok(kind) = row
                        .kind
                        .parse::<orch_link_contract::commands::CommandKind>()
                    else {
                        continue;
                    };
                    let cmd = crate::dispatch::cmd_of(&row);
                    kept.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(Seen {
                            id: row.id,
                            kind,
                            body: cmd.body.clone(),
                        });
                    let (app, behaviour) = (Arc::clone(&app), Arc::clone(&behaviour));
                    tokio::spawn(async move {
                        let outcome = behaviour.run(&cmd).await;
                        let Ok(mut conn) = app.db.acquire().await else {
                            return;
                        };
                        let now = app.clock.now();
                        let written = match &outcome {
                            Ok(result) => {
                                rows::finish_command(
                                    &mut conn,
                                    row.id,
                                    &rows::Ending::Succeeded(result),
                                    now,
                                )
                                .await
                            }
                            Err(error) => {
                                let error = serde_json::to_value(error).unwrap_or_default();
                                rows::finish_command(
                                    &mut conn,
                                    row.id,
                                    &rows::Ending::Failed(&error),
                                    now,
                                )
                                .await
                            }
                        };
                        drop(conn);
                        // A played command uses the same atomic receipt/stop boundary as its real machine.
                        let _ = async {
                            let mut tx = app.db.begin().await?;
                            if let Ok(result) = &outcome {
                                orch_engine::jobs::workspace_upload::retain_native_result(
                                    &mut tx, machine, row.id, result, now,
                                )
                                .await?;
                            }
                            sqlx::query("UPDATE commands SET stop_confirmed_at=$2 WHERE id=$1 AND machine_id=$3 AND stop_confirmed_at IS NULL")
                                .bind(row.id).bind(now).bind(machine).execute(&mut *tx).await?;
                            tx.commit().await.map_err(orch_db::DbError::from)?;
                            Ok::<_, orch_db::DbError>(())
                        }
                        .await;
                        if written.is_ok() {
                            app.link.inner.finished.notify_waiters();
                        }
                    });
                }
            }
        });
        Played { seen, task }
    }
}
