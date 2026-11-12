//! Real HTTP/socket readers and native observations across held projection boundaries.

use crate::realtime_support as fixture;
use tracing::instrument::WithSubscriber;

use crate::support::failure_trace::FailureTrace;

use super::ephemeral;

use crate::middleware::authentication::API_TOKEN_PREFIX;

use crate::middleware::authentication::new_token;

use crate::middleware::authentication::token_hash;

use fixture::Client;

use fixture::Heard;

use fixture::PATIENCE;

use fixture::Rig;

use orch_db::queries::access;

use orch_db::queries::identity;

use orch_link_contract::background::BackgroundPlace;

use orch_link_contract::background::BackgroundState;

use orch_link_contract::background::NativeBackground;

use orch_link_contract::background::NativeBackgroundCommand;

use orch_link_contract::commands::CommandKind;

use orch_link_contract::commands::CommandResult;

use orch_realtime_contract::feeds::Feed;

use orch_types::Id;

use orch_types::Time;

use orch_types::new_id;

use serde_json::Value;

use serde_json::json;

use std::collections::HashMap;

use std::collections::HashSet;

use std::sync::atomic::AtomicUsize;

use std::sync::atomic::Ordering;

use std::sync::Arc;

use std::sync::Condvar;

use std::sync::LazyLock;

use std::sync::Mutex;

use std::sync::PoisonError;

use tokio::sync::Notify;
pub type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const PRIVATE_CWD: &str = "/provider/background-private-directory";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cut {
    HttpPrepared,
    HttpProjected,
    SocketPrepared,
    SocketProjected,
}

struct Boundary {
    reached: Notify,
    resume: Notify,
}

type Boundaries = HashMap<(Id, Cut), Arc<Boundary>>;

static BOUNDARIES: LazyLock<Mutex<Boundaries>> = LazyLock::new(|| Mutex::new(HashMap::new()));

static MANUAL_WATCHES: LazyLock<Mutex<HashSet<Id>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub(crate) fn manual_watch(thread: Id) -> bool {
    MANUAL_WATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&thread)
}

struct ManualWatch(Id);

impl ManualWatch {
    fn of(thread: Id) -> Self {
        assert!(
            MANUAL_WATCHES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(thread)
        );
        Self(thread)
    }
}

impl Drop for ManualWatch {
    fn drop(&mut self) {
        MANUAL_WATCHES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

pub async fn at(thread: Id, cut: Cut) {
    let boundary = BOUNDARIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(thread, cut));
    if let Some(boundary) = boundary {
        boundary.reached.notify_one();
        boundary.resume.notified().await;
    }
}

struct Pause {
    key: (Id, Cut),
    boundary: Arc<Boundary>,
}

impl Pause {
    fn arm(thread: Id, cut: Cut) -> Self {
        let boundary = Arc::new(Boundary {
            reached: Notify::new(),
            resume: Notify::new(),
        });
        assert!(
            BOUNDARIES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert((thread, cut), boundary.clone())
                .is_none()
        );
        Self {
            key: (thread, cut),
            boundary,
        }
    }
    async fn reached(&self) -> Proof {
        tokio::time::timeout(PATIENCE, self.boundary.reached.notified()).await?;
        Ok(())
    }
    fn release(&self) {
        self.boundary.resume.notify_one();
    }
}

impl Drop for Pause {
    fn drop(&mut self) {
        self.release();
        BOUNDARIES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

fn observation(count: usize) -> NativeBackground {
    NativeBackground {
        supported: true,
        as_of: Some(Time::new(orch_testkit::clock::start())),
        state: BackgroundState::Live,
        reason: None,
        items: (0..count)
            .map(|index| NativeBackgroundCommand {
                id: format!("current-reader-{index}"),
                native_item_ref: None,
                turn_id: None,
                command: Some(format!("background command {index}")),
                command_truncated: false,
                cwd: Some(PRIVATE_CWD.into()),
                place: BackgroundPlace::Provider,
                since: None,
                cpu_percent: None,
                memory_bytes: None,
            })
            .collect(),
        next_cursor: None,
        total: Some(count as i64),
    }
}

async fn world() -> Proof<(Rig, Id, Arc<AtomicUsize>)> {
    let rig = Rig::with_time_held().await;
    let thread = rig.thread("Actual background reader").await;
    rig.server
        .engine
        .submit_input(
            &orch_authz::Caller::session(rig.ada),
            thread,
            fixture::queued("Open an existing native source"),
        )
        .await?;
    let turn = rig.turn_start(thread, 0).await;
    rig.begin_message(thread, &turn, "background-reader-source")
        .await;
    rig.finish_turn(thread, &turn, "background-reader-source", "Ready")
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    rig.machine
        .handler
        .on(CommandKind::SessionBackground, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult::SessionBackground(observation(2)))
        });
    Ok((rig, thread, calls))
}

fn url(rig: &Rig, thread: Id, token: &str) -> String {
    let surface = orch_api_contract::surface::HttpSurface::for_token(token).unwrap();
    format!(
        "http://{}{}/threads/{thread}/background",
        rig.server.addr,
        surface.base_path()
    )
}

async fn http(
    rig: &Rig,
    thread: Id,
    token: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Proof<(reqwest::StatusCode, Value)> {
    let mut request = reqwest::Client::new()
        .get(url(rig, thread, token))
        .bearer_auth(token);
    if let Some(cursor) = cursor {
        request = request.query(&[("cursor", cursor)]);
    }
    if let Some(limit) = limit {
        request = request.query(&[("limit", limit)]);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.json().await?;
    Ok((status, body))
}

async fn positive(rig: &Rig, thread: Id, token: &str) -> Proof<Value> {
    let (status, body) = http(rig, thread, token, None, None).await?;
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "actual ordinary native roster: {body}"
    );
    assert_eq!(body["items"][0]["cwd"], PRIVATE_CWD);
    assert_eq!(body["total"], 2);
    assert!(body["list_ref"].is_string());
    Ok(body)
}

fn rules(thread: Id, provider: Id, provider_read: bool) -> Value {
    let mut rules = vec![
        json!({"target":{"kind":"thread","id":thread,"reach":"self"},"resources":["thread"],"allow":[{"operation":"ThreadBackgroundGet","actions":["read"]}]}),
    ];
    if provider_read {
        rules.push(json!({"target":{"kind":"provider","id":provider,"reach":"self"},"resources":["provider"],"allow":[{"operation":"ThreadBackgroundGet","actions":["read"]}]}));
    }
    json!({"rules":rules})
}

async fn scoped_key(rig: &Rig, thread: Id, expires: Option<Time>) -> Proof<(Id, String)> {
    let id = new_id();
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *rig.pool.acquire().await?,
        &identity::NewApiToken {
            id,
            user_id: rig.ada,
            name: "Background reader control",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: rig.clock.now(),
            expires_at: expires.map(Time::get),
        },
        Some(&rules(thread, rig.provider, true)),
    )
    .await?;
    Ok((id, token))
}

async fn deadline(kind: &str, expire: bool) -> Proof {
    let (rig, thread, _) = world().await?;
    let token = match kind {
        "session" => rig.token_for(rig.ada, chrono::Duration::seconds(2)).await,
        "legacy" => {
            let token = new_token(API_TOKEN_PREFIX);
            identity::create_api_token(
                &mut *rig.pool.acquire().await?,
                &identity::NewApiToken {
                    id: new_id(),
                    user_id: rig.ada,
                    name: "Legacy background reader deadline",
                    token_hash: &token_hash(&token),
                    scopes: &["*".into()],
                    node_id: None,
                    at: rig.clock.now(),
                    expires_at: Some(rig.clock.now() + chrono::Duration::seconds(2)),
                },
            )
            .await?;
            token
        }
        _ => {
            scoped_key(
                &rig,
                thread,
                expire.then(|| Time::new(rig.clock.now() + chrono::Duration::seconds(2))),
            )
            .await?
            .1
        }
    };
    positive(&rig, thread, &token).await?;
    let pause = Pause::arm(thread, Cut::HttpPrepared);
    let endpoint = url(&rig, thread, &token);
    let credential = token.clone();
    let pending = tokio::spawn(async move {
        reqwest::Client::new()
            .get(endpoint)
            .bearer_auth(credential)
            .send()
            .await
    });
    if let Err(error) = pause.reached().await {
        pause.release();
        pending.abort();
        let _ = pending.await;
        return Err(error);
    }
    rig.let_time_pass(chrono::Duration::seconds(3));
    pause.release();
    let response = tokio::time::timeout(PATIENCE, pending).await???;
    let status = response.status();
    let body: Value = response.json().await?;
    if expire {
        assert_eq!(
            status,
            reqwest::StatusCode::UNAUTHORIZED,
            "actual raw credential must end: {body}"
        );
        assert!(!body.to_string().contains(PRIVATE_CWD));
        assert!(body.get("list_ref").is_none());
    } else {
        assert_eq!(status, reqwest::StatusCode::OK, "lasting key: {body}");
        assert_eq!(body["items"][0]["cwd"], PRIVATE_CWD);
    }
    rig.shutdown().await;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn http_session_expiry_after_native_preparation_withholds_the_roster() -> Proof {
    deadline("session", true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_http_session_expiry_after_native_preparation_withholds_the_roster()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::http_session_expiry_after_native_preparation_withholds_the_roster(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn http_legacy_key_expiry_after_native_preparation_withholds_the_roster() -> Proof {
    deadline("legacy", true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_http_legacy_key_expiry_after_native_preparation_withholds_the_roster()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::http_legacy_key_expiry_after_native_preparation_withholds_the_roster(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn http_scoped_key_expiry_after_native_preparation_withholds_the_roster() -> Proof {
    deadline("scoped", true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_http_scoped_key_expiry_after_native_preparation_withholds_the_roster()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::http_scoped_key_expiry_after_native_preparation_withholds_the_roster(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn http_nonexpiring_key_remains_valid_after_native_preparation() -> Proof {
    deadline("scoped", false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_http_nonexpiring_key_remains_valid_after_native_preparation()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(self::http_nonexpiring_key_remains_valid_after_native_preparation())
}

async fn viewer(rig: &Rig) -> Proof<(Id, String)> {
    let (user, token) = rig.person("background-source-reader").await;
    rig.give(user, "viewer").await;
    access::insert_entry(
        &mut *rig.pool.acquire().await?,
        &access::NewEntry {
            id: new_id(),
            page: &access::PageKey::new("provider", rig.provider, "live"),
            profile: "any",
            permission: "provider.manage",
            holder_kind: "user",
            holder_id: Some(user),
            holder_role: None,
            options: &json!({}),
            expires_at: Some(rig.clock.now() + chrono::Duration::seconds(2)),
            note: "",
            source: "page",
            created_by: Some(rig.ada),
            at: rig.clock.now(),
        },
    )
    .await?;
    rig.bump_epoch().await;
    Ok((user, token))
}

#[cfg(any(test, feature = "test-support"))]
async fn http_second_page_reprojects_an_expired_optional_source_without_native_recapture() -> Proof
{
    let (rig, thread, calls) = world().await?;
    let (_, token) = viewer(&rig).await?;
    positive(&rig, thread, &token).await?;
    let (status, first) = http(&rig, thread, &token, None, Some(1)).await?;
    assert_eq!(status, reqwest::StatusCode::OK);
    let cursor = first["next_cursor"]
        .as_str()
        .ok_or("a real second page is required")?
        .to_owned();
    let pause = Pause::arm(thread, Cut::HttpProjected);
    let endpoint = url(&rig, thread, &token);
    let pending = tokio::spawn(async move {
        reqwest::Client::new()
            .get(endpoint)
            .bearer_auth(token)
            .query(&[("cursor", cursor)])
            .send()
            .await
    });
    if let Err(error) = pause.reached().await {
        pause.release();
        pending.abort();
        let _ = pending.await;
        return Err(error);
    }
    let observed = calls.load(Ordering::SeqCst);
    let mut conn = rig.pool.acquire().await?;
    let epoch = access::authority_epoch(&mut conn).await?;
    drop(conn);
    rig.let_time_pass(chrono::Duration::seconds(3));
    pause.release();
    let response = tokio::time::timeout(PATIENCE, pending).await???;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let page: Value = response.json().await?;
    assert_eq!(page["list_ref"], first["list_ref"]);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["cwd"], Value::Null);
    assert_eq!(page["items"][0]["cwd_redacted"], true);
    assert_eq!(page["total"], 2);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        observed,
        "projection retries must not ask the native session again"
    );
    assert_eq!(
        access::authority_epoch(&mut *rig.pool.acquire().await?).await?,
        epoch,
        "expiry must not be replaced by an epoch mutation"
    );
    rig.shutdown().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_http_second_page_reprojects_an_expired_optional_source_without_native_recapture()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::http_second_page_reprojects_an_expired_optional_source_without_native_recapture(),
    )
}

struct NativeBoundary {
    reached: Notify,
    released: Mutex<bool>,
    wake: Condvar,
}

struct NativeRelease(Arc<NativeBoundary>);

impl Drop for NativeRelease {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl NativeBoundary {
    fn release(&self) {
        *self.released.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.wake.notify_all();
    }
}

impl Drop for NativeBoundary {
    fn drop(&mut self) {
        self.wake.notify_all();
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn http_native_wait_uses_the_actual_session_mode_without_borrowing_another_session() -> Proof
{
    let (rig, thread, _) = world().await?;
    let (user, token) = rig.person("background-mode-reader").await;
    rig.give(user, "viewer").await;
    access::testing::make_system_admin(&mut *rig.pool.acquire().await?, user).await?;
    let session: Id =
        sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
            .bind(token_hash(&token))
            .fetch_one(rig.pool.pool())
            .await?;
    assert!(
        identity::set_session_admin_mode(
            &mut *rig.pool.acquire().await?,
            user,
            session,
            true,
            rig.clock.now()
        )
        .await?
    );
    positive(&rig, thread, &token).await?;
    let other = rig.token_for(user, chrono::Duration::minutes(5)).await;
    let other_session: Id =
        sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
            .bind(token_hash(&other))
            .fetch_one(rig.pool.pool())
            .await?;
    assert!(
        identity::set_session_admin_mode(
            &mut *rig.pool.acquire().await?,
            user,
            other_session,
            true,
            rig.clock.now()
        )
        .await?
    );
    let boundary = Arc::new(NativeBoundary {
        reached: Notify::new(),
        released: Mutex::new(false),
        wake: Condvar::new(),
    });
    let _release = NativeRelease(boundary.clone());
    let held = boundary.clone();
    rig.machine
        .handler
        .on(CommandKind::SessionBackground, move |_| {
            held.reached.notify_one();
            let released = held.released.lock().unwrap_or_else(PoisonError::into_inner);
            let (released, timeout) = held
                .wake
                .wait_timeout_while(released, PATIENCE, |released| !*released)
                .unwrap_or_else(PoisonError::into_inner);
            assert!(
                !timeout.timed_out() && *released,
                "the actual native observation must be released"
            );
            Ok(CommandResult::SessionBackground(observation(2)))
        });
    // Expire only the existing one-second observation cache; no credential or access changes yet.
    rig.let_time_pass(chrono::Duration::seconds(1));
    let endpoint = url(&rig, thread, &token);
    let pending = tokio::spawn(async move {
        reqwest::Client::new()
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
    });
    if let Err(error) = tokio::time::timeout(PATIENCE, boundary.reached.notified()).await {
        boundary.release();
        pending.abort();
        let _ = pending.await;
        return Err(error.into());
    }
    assert!(
        identity::set_session_admin_mode(
            &mut *rig.pool.acquire().await?,
            user,
            session,
            false,
            rig.clock.now()
        )
        .await?
    );
    boundary.release();
    let response = tokio::time::timeout(PATIENCE, pending).await???;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let page: Value = response.json().await?;
    assert_eq!(page["items"][0]["cwd"], Value::Null);
    assert_eq!(page["items"][0]["cwd_redacted"], true);
    assert_eq!(
        identity::session_admin_mode(
            &mut *rig.pool.acquire().await?,
            user,
            other_session,
            rig.clock.now()
        )
        .await?,
        Some(true)
    );
    rig.shutdown().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_http_native_wait_uses_the_actual_session_mode_without_borrowing_another_session()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::http_native_wait_uses_the_actual_session_mode_without_borrowing_another_session(),
    )
}

async fn observed_frame(
    trace: &FailureTrace,
    client: &mut Client,
    connection: &super::server::Connection,
) -> Value {
    match client.hear_within(PATIENCE).await {
        Some(Heard::Frame(frame)) => {
            trace.mark(format!(
                "received op={} total={:?} items={:?}",
                frame["op"].as_str().unwrap_or("unknown"),
                frame["data"]["total"].as_u64(),
                frame["data"]["items"].as_array().map(Vec::len),
            ));
            frame
        }
        other => {
            let subscriptions = connection
                .subscriptions()
                .iter()
                .map(|sub| {
                    format!(
                        "{} live={} decided={}",
                        sub.feed(),
                        sub.is_live(),
                        sub.decided()
                    )
                })
                .collect::<Vec<_>>();
            trace.mark(format!(
                "socket receive={other:?}; subscriptions={subscriptions:?}"
            ));
            panic!("a background frame was expected within {PATIENCE:?}, and {other:?} came");
        }
    }
}

async fn socket_expiry(seed: bool, expire_origin: bool) -> Proof {
    let mut trace = FailureTrace::new("background socket optional-source expiry", PATIENCE);
    trace.mark(format!(
        "prepare scene; seed={seed} expire_origin={expire_origin}"
    ));
    let (rig, thread, calls) = world().await?;
    let (user, token) = if expire_origin {
        (
            rig.ada,
            rig.token_for(rig.ada, chrono::Duration::seconds(2)).await,
        )
    } else {
        viewer(&rig).await?
    };
    positive(&rig, thread, &token).await?;
    trace.mark("HTTP positive completed");
    let capture = rig.server.engine.capture_background(thread).await?;
    let data = serde_json::to_value(capture)?;
    let feed = Feed::ThreadBackground(thread);
    // Drive only this thread's canonical seed/broadcast ourselves. The real socket,
    // native observation, projection and writer remain unchanged; no polling race can
    // inject an already accepted baseline row after the tested cut.
    let _watch = ManualWatch::of(thread);
    let observed = calls.load(Ordering::SeqCst);
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    rig.server
        .app
        .realtime
        .inner
        .ephemeral
        .test_background_seed(feed.clone(), data.clone());
    let mut pause = if seed {
        Some(Pause::arm(thread, Cut::SocketProjected))
    } else {
        None
    };
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    trace.mark("actual socket subscribed");
    let connection = rig
        .server
        .app
        .realtime
        .inner
        .registry()
        .connections
        .values()
        .find(|connection| connection.user == user)
        .cloned()
        .ok_or("the actual socket owner is absent")?;
    let pending = if seed {
        None
    } else {
        tokio::time::timeout(PATIENCE, async {
            loop {
                let frame = observed_frame(&trace, &mut client, &connection).await;
                if frame["op"] == "event" && frame["data"]["items"].is_array() {
                    assert!(frame.to_string().contains(PRIVATE_CWD));
                    break;
                }
            }
        })
        .await?;
        pause = Some(Pause::arm(thread, Cut::SocketProjected));
        let inner = rig.server.app.realtime.inner.clone();
        let feed = feed.clone();
        let data = data.clone();
        let diagnostic = trace.recorder();
        Some(tokio::spawn(async move {
            let result = ephemeral::deliver(&inner, &feed, &data, None)
                .with_subscriber(diagnostic.clone())
                .await;
            diagnostic.mark(format!("broadcast delivery result={result:?}"));
            result
        }))
    };
    let pause = pause.ok_or("the actual projection boundary was not armed")?;
    if let Err(error) = pause.reached().await {
        pause.release();
        if let Some(pending) = pending {
            pending.abort();
            let _ = pending.await;
        }
        return Err(error);
    }
    let epoch = access::authority_epoch(&mut *rig.pool.acquire().await?).await?;
    trace.mark(format!(
        "native projection paused; epoch={epoch} native_calls={observed}"
    ));
    rig.let_time_pass(chrono::Duration::seconds(3));
    trace.mark("advanced the held clock by three seconds and released projection");
    pause.release();
    if let Some(pending) = pending {
        tokio::time::timeout(PATIENCE, pending).await???;
    }
    if expire_origin {
        let mut closed = false;
        while let Some(heard) = client.hear_within(PATIENCE).await {
            match heard {
                Heard::Frame(frame) => assert!(!frame.to_string().contains(PRIVATE_CWD)),
                Heard::Closed(code) => {
                    assert_eq!(code, Some(4401));
                    closed = true;
                    break;
                }
            }
        }
        assert!(
            closed,
            "expired actual origin must close before another roster row"
        );
    } else {
        tokio::time::timeout(PATIENCE, async {
            loop {
                let frame = observed_frame(&trace, &mut client, &connection).await;
                assert!(
                    !frame.to_string().contains(PRIVATE_CWD),
                    "old private directory reached the actual socket"
                );
                if frame["op"] == "event" && frame["data"]["items"].is_array() {
                    assert_eq!(frame["data"]["items"][0]["cwd_redacted"], true);
                    assert_eq!(frame["data"]["total"], 2);
                    break;
                }
            }
        })
        .await?;
        assert!(
            connection
                .subscriptions()
                .iter()
                .any(|sub| sub.feed() == &feed),
            "main ThreadView survives optional source loss"
        );
        assert_eq!(
            access::authority_epoch(&mut *rig.pool.acquire().await?).await?,
            epoch
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        observed,
        "read reprojection must reuse the same native observation"
    );
    rig.shutdown().await;
    trace.finish();
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn socket_initial_seed_reprojects_optional_source_expiry_without_an_epoch_change() -> Proof {
    socket_expiry(true, false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_socket_initial_seed_reprojects_optional_source_expiry_without_an_epoch_change()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::socket_initial_seed_reprojects_optional_source_expiry_without_an_epoch_change(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn socket_broadcast_reprojects_optional_source_expiry_without_an_epoch_change() -> Proof {
    socket_expiry(false, false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_socket_broadcast_reprojects_optional_source_expiry_without_an_epoch_change()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::socket_broadcast_reprojects_optional_source_expiry_without_an_epoch_change(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn socket_raw_session_expiry_withholds_a_projected_roster_and_closes() -> Proof {
    socket_expiry(false, true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_socket_raw_session_expiry_withholds_a_projected_roster_and_closes()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::socket_raw_session_expiry_withholds_a_projected_roster_and_closes(),
    )
}

async fn require_second_step(rig: &Rig, token: &str) -> Proof<(Id, Id)> {
    let other = rig.token_for(rig.ada, chrono::Duration::minutes(5)).await;
    let mut conn = rig.pool.acquire().await?;
    let session: Id =
        sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
            .bind(token_hash(token))
            .fetch_one(&mut *conn)
            .await?;
    let peer: Id = sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
        .bind(token_hash(&other))
        .fetch_one(&mut *conn)
        .await?;
    orch_settings::write::write(
        &mut conn,
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::System,
            target: orch_settings::Target::Node(rig.root),
            key: "auth.second_step",
            value: json!("required"),
            locked: None,
            expected_revision: None,
            may_lock: false,
            origin: &orch_db::audit::Origin::default(),
        },
        rig.clock.now(),
    )
    .await?;
    sqlx::query("UPDATE users SET email_code_at=$2 WHERE id=$1")
        .bind(rig.ada)
        .bind(rig.clock.now())
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE sessions SET second_step_at=$2 WHERE id=ANY($1)")
        .bind(vec![session, peer])
        .bind(rig.clock.now())
        .execute(&mut *conn)
        .await?;
    Ok((session, peer))
}

#[cfg(any(test, feature = "test-support"))]
async fn http_background_uses_its_actual_second_step_hold_after_source_preparation() -> Proof {
    let (rig, thread, _) = world().await?;
    let (session, peer) = require_second_step(&rig, &rig.token).await?;
    positive(&rig, thread, &rig.token).await?;
    let pause = Pause::arm(thread, Cut::HttpPrepared);
    let endpoint = url(&rig, thread, &rig.token);
    let credential = rig.token.clone();
    let pending = tokio::spawn(async move {
        reqwest::Client::new()
            .get(endpoint)
            .bearer_auth(credential)
            .send()
            .await
    });
    if let Err(error) = pause.reached().await {
        pause.release();
        pending.abort();
        let _ = pending.await;
        return Err(error);
    }
    sqlx::query("UPDATE sessions SET second_step_at=NULL WHERE id=$1")
        .bind(session)
        .execute(rig.pool.pool())
        .await?;
    pause.release();
    let response = tokio::time::timeout(PATIENCE, pending).await???;
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "auth.second_step_required");
    assert!(!body.to_string().contains(PRIVATE_CWD));
    assert!(body.get("list_ref").is_none());
    assert!(
        orch_db::queries::second_step::session_given_at(&mut *rig.pool.acquire().await?, peer)
            .await?
            .is_some(),
        "another session's proof remains independent"
    );
    rig.shutdown().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_http_background_uses_its_actual_second_step_hold_after_source_preparation()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::http_background_uses_its_actual_second_step_hold_after_source_preparation(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn socket_background_closes_its_feed_on_actual_second_step_loss_without_borrowing_peer_proof()
-> Proof {
    let (rig, thread, _) = world().await?;
    let (session, peer) = require_second_step(&rig, &rig.token).await?;
    positive(&rig, thread, &rig.token).await?;
    let data = serde_json::to_value(rig.server.engine.capture_background(thread).await?)?;
    let feed = Feed::ThreadBackground(thread);
    let _watch = ManualWatch::of(thread);
    rig.server
        .app
        .realtime
        .inner
        .ephemeral
        .test_background_seed(feed.clone(), data.clone());
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if frame["op"] == "event" && frame["data"]["items"].is_array() {
                assert!(frame.to_string().contains(PRIVATE_CWD));
                break;
            }
        }
    })
    .await?;
    let pause = Pause::arm(thread, Cut::SocketPrepared);
    let inner = rig.server.app.realtime.inner.clone();
    let pending = tokio::spawn(async move { ephemeral::deliver(&inner, &feed, &data, None).await });
    if let Err(error) = pause.reached().await {
        pause.release();
        pending.abort();
        let _ = pending.await;
        return Err(error);
    }
    let epoch = access::authority_epoch(&mut *rig.pool.acquire().await?).await?;
    sqlx::query("UPDATE sessions SET second_step_at=NULL WHERE id=$1")
        .bind(session)
        .execute(rig.pool.pool())
        .await?;
    pause.release();
    tokio::time::timeout(PATIENCE, pending).await???;
    tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = client.frame().await;
            assert!(!frame.to_string().contains(PRIVATE_CWD));
            if frame["op"] == "closed" {
                assert_eq!(frame["reason"], "revoked");
                break;
            }
        }
    })
    .await?;
    assert_eq!(
        access::authority_epoch(&mut *rig.pool.acquire().await?).await?,
        epoch,
        "actual assurance refresh must not depend on a manufactured epoch change"
    );
    assert!(
        orch_db::queries::second_step::session_given_at(&mut *rig.pool.acquire().await?, peer)
            .await?
            .is_some()
    );
    rig.shutdown().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_socket_background_closes_its_feed_on_actual_second_step_loss_without_borrowing_peer_proof()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(self::socket_background_closes_its_feed_on_actual_second_step_loss_without_borrowing_peer_proof())
}

#[cfg(any(test, feature = "test-support"))]
async fn indirect_role_expiry_reprojects_provider_directories_from_the_native_holder_stamp() -> Proof
{
    let (rig, thread, _) = world().await?;
    let (user, token) = rig.person("background-role-source").await;
    let until = rig.clock.now() + chrono::Duration::seconds(2);
    let mut conn = rig.pool.acquire().await?;
    access::testing::give_role(
        &mut conn,
        rig.project,
        "user",
        user,
        "developer",
        rig.ada,
        None,
    )
    .await?;
    access::put_role(
        &mut conn,
        &access::NewRole {
            id: new_id(),
            node: rig.topic,
            subject_kind: "role",
            subject: rig.project,
            subject_role: Some("developer"),
            role: "admin",
            expires_at: Some(until),
            note: "",
            by: rig.ada,
            at: rig.clock.now(),
        },
    )
    .await?;
    // Bind the provider's real Manage row to this indirect role explicitly; the
    // deciding expiry must not depend on a fixture's resource-default preset.
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id: new_id(),
            page: &access::PageKey::new("provider", rig.provider, "live"),
            profile: "any",
            permission: "provider.manage",
            holder_kind: "role",
            holder_id: Some(rig.topic),
            holder_role: Some("admin"),
            options: &json!({}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(rig.ada),
            at: rig.clock.now(),
        },
    )
    .await?;
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(user),
        rig.clock.time(),
    )
    .await?;
    assert_eq!(
        orch_db::queries::access::kept::read_deadline(
            &mut conn,
            user,
            principal.groups(),
            principal.member_of(),
            rig.clock.now()
        )
        .await?,
        None,
        "the original direct-subject query does not include the indirect role source"
    );
    assert_eq!(
        principal.read_deadline(&mut conn).await?,
        Some(Time::new(until)),
        "the ordinary holder stamp carries the real dependency deadline"
    );
    drop(conn);
    rig.bump_epoch().await;
    positive(&rig, thread, &token).await?;
    let pause = Pause::arm(thread, Cut::HttpProjected);
    let endpoint = url(&rig, thread, &token);
    let pending = tokio::spawn(async move {
        reqwest::Client::new()
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
    });
    if let Err(error) = pause.reached().await {
        pause.release();
        pending.abort();
        let _ = pending.await;
        return Err(error);
    }
    let epoch = access::authority_epoch(&mut *rig.pool.acquire().await?).await?;
    rig.let_time_pass(chrono::Duration::seconds(3));
    pause.release();
    let response = tokio::time::timeout(PATIENCE, pending).await???;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let page: Value = response.json().await?;
    assert_eq!(page["items"][0]["cwd"], Value::Null);
    assert_eq!(page["items"][0]["cwd_redacted"], true);
    assert_eq!(page["total"], 2);
    assert_eq!(
        access::authority_epoch(&mut *rig.pool.acquire().await?).await?,
        epoch
    );
    rig.shutdown().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_indirect_role_expiry_reprojects_provider_directories_from_the_native_holder_stamp()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::indirect_role_expiry_reprojects_provider_directories_from_the_native_holder_stamp(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn native_background_wrapper_preserves_scoped_admission_and_current_key_revocation() -> Proof
{
    let (rig, thread, _) = world().await?;
    let (id, token) = scoped_key(&rig, thread, None).await?;
    let connection = super::server::epoch_test_connection(
        &rig.server.app,
        &rig.server.app.realtime.inner,
        &token,
    )
    .await
    .map_err(|(code, _)| format!("actual key authenticator refused the native control: {code}"))?;
    let caller = connection.credential().current;
    assert!(
        caller.admitted_operation().is_none(),
        "the native caller needs the real producer's admission"
    );
    let origin = orch_engine::background::Identity {
        credential_id: id,
        token: true,
    };
    let page = rig
        .server
        .engine
        .background(&caller, origin, thread, None, None)
        .await?;
    assert_eq!(page.items[0].cwd.as_deref(), Some(PRIVATE_CWD));
    assert_eq!(page.total, Some(2));
    assert!(
        identity::revoke_api_token(
            &mut *rig.pool.acquire().await?,
            id,
            rig.ada,
            rig.clock.now()
        )
        .await?
    );
    assert!(matches!(
        rig.server
            .engine
            .background(&caller, origin, thread, None, None)
            .await,
        Err(orch_engine::background::BackgroundError::Scope)
    ));
    drop(connection);
    rig.shutdown().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_native_background_wrapper_preserves_scoped_admission_and_current_key_revocation()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::background_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::native_background_wrapper_preserves_scoped_admission_and_current_key_revocation(),
    )
}
