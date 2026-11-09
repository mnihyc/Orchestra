//! Actual socket-origin authentication after its real credential-row wait.
//! These controls call the existing socket login constructor; they do not claim writer-byte proof.

use crate::realtime_support as fixture;

use super::server;

use orch_realtime_contract::version::CloseCode;

use orch_types::Clock;

use axum::extract::ws::Message;

use futures::Sink;

use futures::task::AtomicWaker;

use std::collections::HashMap;

use std::pin::Pin;

use std::sync::atomic::AtomicBool;

use std::sync::atomic::Ordering;

use std::sync::Arc;

use std::sync::LazyLock;

use std::sync::Mutex;

use std::sync::PoisonError;

use std::task::Context;

use std::task::Poll;

use tokio::sync::Notify;
pub type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn raw_deadline_after_identity_wait(key: bool) -> Proof {
    let rig = fixture::Rig::with_time_held().await;
    let (user, browser) = rig.person("socket-origin-reader").await;
    let token = if key {
        rig.api_token(user).await
    } else {
        browser
    };
    let baseline =
        server::epoch_test_connection(&rig.server.app, &rig.server.app.realtime.inner, &token)
            .await
            .map_err(|error| format!("ordinary exact socket login premise: {error:?}"))?;
    assert_eq!(baseline.user, user);
    let id = baseline.credential().id();
    let until = Clock::now(rig.server.app.clock.as_ref()) + chrono::Duration::seconds(2);
    let update = if key {
        "UPDATE api_tokens SET expires_at=$2 WHERE id=$1"
    } else {
        "UPDATE sessions SET expires_at=$2 WHERE id=$1"
    };
    sqlx::query(update)
        .bind(id)
        .bind(until)
        .execute(rig.pool.pool())
        .await?;
    let mut blocker = rig.pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let lock = if key {
        "SELECT id FROM api_tokens WHERE id=$1 FOR NO KEY UPDATE"
    } else {
        "SELECT id FROM sessions WHERE id=$1 FOR NO KEY UPDATE"
    };
    let _: orch_types::Id = sqlx::query_scalar(lock)
        .bind(id)
        .fetch_one(&mut *blocker)
        .await?;
    let app = rig.server.app.clone();
    let pending = tokio::spawn(async move {
        server::epoch_test_connection(&app, &app.realtime.inner, &token).await
    });
    let reached = tokio::time::timeout(fixture::PATIENCE, async {
        let surface = if key {
            orch_api_contract::surface::HttpSurface::Key
        } else {
            orch_api_contract::surface::HttpSurface::Browser
        };
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock'
                  AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)",
            )
            .bind(pid)
            .bind(crate::testing::current_identity_query_pattern(surface))
            .fetch_one(rig.pool.pool())
            .await?;
            if blocked {
                return Ok::<(), sqlx::Error>(());
            }
            if pending.is_finished() {
                return Err(sqlx::Error::RowNotFound);
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if !matches!(reached, Ok(Ok(()))) {
        blocker.rollback().await?;
        pending.abort();
        let _ = pending.await;
        return Err(
            format!("actual socket identity SHARE-row wait was not reached: {reached:?}").into(),
        );
    }
    // No revocation/authority SQL substitutes for the natural deadline crossing.
    rig.let_time_pass(chrono::Duration::seconds(3));
    blocker.rollback().await?;
    let result = tokio::time::timeout(fixture::PATIENCE, pending).await??;
    assert!(
        matches!(result, Err((code,_)) if code == CloseCode::Unauthenticated.code()),
        "the actual socket credential must be rechecked after its identity wait"
    );
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn browser_login_checks_its_actual_session_deadline_after_identity_wait() -> Proof {
    raw_deadline_after_identity_wait(false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_browser_login_checks_its_actual_session_deadline_after_identity_wait()
-> ::futures::future::LocalBoxFuture<'static, ::orch_api_base::realtime::origin_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::browser_login_checks_its_actual_session_deadline_after_identity_wait(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn key_login_checks_its_actual_key_deadline_after_identity_wait() -> Proof {
    raw_deadline_after_identity_wait(true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_key_login_checks_its_actual_key_deadline_after_identity_wait()
-> ::futures::future::LocalBoxFuture<'static, ::orch_api_base::realtime::origin_read_tests::Proof> {
    ::std::boxed::Box::pin(self::key_login_checks_its_actual_key_deadline_after_identity_wait())
}

struct Capacity {
    blocked: AtomicBool,
    selected_marker: AtomicBool,
    marker: String,
    reached: Notify,
    wake: AtomicWaker,
}

static CAPACITY: LazyLock<Mutex<HashMap<String, Arc<Capacity>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A named test-only capacity boundary around the REAL WebSocket sink. All accepted bytes still
/// traverse the original socket; this is controlled readiness, not a claimed OS-buffer stall.
pub struct CapacitySink<S> {
    inner: S,
    connection: String,
}

impl<S> CapacitySink<S> {
    pub(super) fn into_inner(self) -> S {
        self.inner
    }
}

pub(super) fn capacity_sink<S>(inner: S, connection: &str) -> CapacitySink<S> {
    CapacitySink {
        inner,
        connection: connection.to_owned(),
    }
}

fn capacity(connection: &str) -> Option<Arc<Capacity>> {
    CAPACITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(connection)
        .cloned()
}

pub(super) fn selected_frame(connection: &str, text: &str) {
    if let Some(cut) = capacity(connection)
        && text.contains(&cut.marker)
    {
        cut.selected_marker.store(true, Ordering::Release);
    }
}

impl<S: Sink<Message> + Unpin> Sink<Message> for CapacitySink<S> {
    type Error = S::Error;
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_ready(cx) {
            Poll::Ready(Ok(())) => {
                if let Some(cut) = capacity(&this.connection) {
                    cut.wake.register(cx.waker());
                    if cut.blocked.load(Ordering::Acquire) {
                        cut.reached.notify_one();
                        return Poll::Pending;
                    }
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.get_mut().inner).start_send(item)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

struct Pause {
    name: String,
    cut: Arc<Capacity>,
}

impl Pause {
    fn arm(connection: &server::Connection, marker: &str) -> Self {
        let cut = Arc::new(Capacity {
            blocked: AtomicBool::new(true),
            selected_marker: AtomicBool::new(false),
            marker: marker.to_owned(),
            reached: Notify::new(),
            wake: AtomicWaker::new(),
        });
        let name = connection.name.clone();
        assert!(
            CAPACITY
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(name.clone(), cut.clone())
                .is_none()
        );
        Self { name, cut }
    }
    fn release(&self) {
        self.cut.blocked.store(false, Ordering::Release);
        self.cut.wake.wake();
    }
}

impl Drop for Pause {
    fn drop(&mut self) {
        self.release();
        CAPACITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.name);
    }
}

async fn inbox(
    rig: &fixture::Rig,
    user: orch_types::Id,
    title: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use orch_db::queries::notifications::people::{NewNotification, announce};
    let mut tx = rig.pool.begin().await?;
    announce(
        &mut tx,
        &NewNotification {
            user_id: user,
            kind: "milestone",
            node_id: None,
            thread_id: None,
            approval_id: None,
            chat_id: None,
            title,
            body: "",
            data: &serde_json::json!({}),
            dedupe_key: &format!("socket-capacity:{title}"),
            at: Clock::now(rig.server.app.clock.as_ref()),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn capacity_boundary(expire: bool) -> Proof {
    use fixture::{Client, Heard};
    let rig = fixture::Rig::cramped_with_time_held(|tuning| {
        // Reuse the fixture rendezvous budget so cleanup cannot cancel the writer before the
        // controlled readiness boundary is released; this changes no product threshold.
        tuning.close_grace = fixture::PATIENCE;
    })
    .await;
    let (user, _) = rig.person("capacity-origin-reader").await;
    let token = rig.token_for(user, chrono::Duration::seconds(2)).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;
    inbox(&rig, user, "origin-capacity-positive").await?;
    tokio::time::timeout(fixture::PATIENCE, async {
        loop {
            if client
                .frame()
                .await
                .to_string()
                .contains("origin-capacity-positive")
            {
                break;
            }
        }
    })
    .await?;
    let connection = {
        let registry = rig.server.app.realtime.inner.registry();
        registry
            .connections
            .values()
            .find(|connection| connection.user == user)
            .cloned()
    }
    .ok_or("the actual authenticated socket owner is absent")?;
    assert_eq!(
        connection.credential().browser_session(),
        Some(connection.credential().id())
    );
    const MARKER: &str = "origin-capacity-unaccepted-private-row";
    let pause = Pause::arm(&connection, MARKER);
    inbox(&rig, user, MARKER).await?;
    tokio::time::timeout(fixture::PATIENCE, pause.cut.reached.notified()).await?;
    let detached = connection.outbox.test_in_flight_bytes();
    assert!(
        pause.cut.selected_marker.load(Ordering::Acquire)
            || connection.outbox.test_contains(MARKER),
        "the actual native Inbox projection must be held by this socket, not an unrelated frame"
    );
    if !expire {
        pause.release();
        assert_eq!(
            detached, 0,
            "socket readiness must precede detaching the cancellable projection"
        );
        tokio::time::timeout(fixture::PATIENCE, async {
            loop {
                if client.frame().await.to_string().contains(MARKER) {
                    break;
                }
            }
        })
        .await
        .map_err(|error| {
            format!("same held real socket frame must publish after capacity returns: {error}")
        })?;
    } else {
        if detached > 0 {
            assert!(
                pause.cut.selected_marker.load(Ordering::Acquire),
                "the held frame must be the actual marker"
            );
        }
        rig.let_time_pass(chrono::Duration::seconds(3));
        assert!(
            super::watches::delivery_guard(&rig.server.app.realtime.inner, &connection)
                .await?
                .is_none()
        );
        assert!(
            connection.outbox.closing().is_some(),
            "real raw-session expiry must end this origin"
        );
        pause.release();
        let mut closed = false;
        while let Some(heard) = client.hear_within(fixture::PATIENCE).await {
            match heard {
                Heard::Frame(frame) => assert!(
                    !frame.to_string().contains(MARKER),
                    "unaccepted old-origin bytes reached the real socket"
                ),
                Heard::Closed(_) => {
                    closed = true;
                    break;
                }
            }
        }
        assert!(
            closed,
            "expired socket must close after rejecting held private bytes"
        );
    }
    rig.shutdown().await;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn real_socket_waits_capacity_before_detaching_a_projected_frame() -> Proof {
    capacity_boundary(false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_real_socket_waits_capacity_before_detaching_a_projected_frame()
-> ::futures::future::LocalBoxFuture<'static, ::orch_api_base::realtime::origin_read_tests::Proof> {
    ::std::boxed::Box::pin(self::real_socket_waits_capacity_before_detaching_a_projected_frame())
}

#[cfg(any(test, feature = "test-support"))]
async fn raw_session_expiry_cancels_an_unaccepted_real_socket_frame() -> Proof {
    capacity_boundary(true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_raw_session_expiry_cancels_an_unaccepted_real_socket_frame()
-> ::futures::future::LocalBoxFuture<'static, ::orch_api_base::realtime::origin_read_tests::Proof> {
    ::std::boxed::Box::pin(self::raw_session_expiry_cancels_an_unaccepted_real_socket_frame())
}

struct Publication {
    marker: String,
    reached: Notify,
    replaced: Notify,
    resume: Notify,
    reauth_resume: Notify,
    retired: Notify,
}

static PUBLICATION: LazyLock<Mutex<HashMap<String, Arc<Publication>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn publication(connection: &str) -> Option<Arc<Publication>> {
    PUBLICATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(connection)
        .cloned()
}

pub(super) async fn before_start(connection: &str, text: &str) {
    if let Some(cut) = publication(connection)
        && text.contains(&cut.marker)
    {
        cut.reached.notify_one();
        cut.resume.notified().await;
    }
}

pub(super) async fn reauthenticated(connection: &str) {
    if let Some(cut) = publication(connection) {
        cut.replaced.notify_one();
        cut.reauth_resume.notified().await;
    }
}

pub(super) fn retired(connection: &str) {
    if let Some(cut) = publication(connection) {
        cut.retired.notify_one();
    }
}

struct PublicationPause {
    name: String,
    cut: Arc<Publication>,
}

impl Drop for PublicationPause {
    fn drop(&mut self) {
        self.cut.resume.notify_one();
        self.cut.reauth_resume.notify_one();
        PUBLICATION
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.name);
    }
}

async fn publication_boundary(resource: bool) -> Proof {
    use fixture::Client;
    use orch_realtime_contract::feeds::{Feed, ResourceKind};
    let rig = fixture::Rig::cramped_with_time_held(|tuning| {
        tuning.close_grace = fixture::PATIENCE;
    })
    .await;
    let user = if resource {
        rig.ada
    } else {
        rig.person("publication-origin-reader").await.0
    };
    let token = rig.token_for(user, chrono::Duration::minutes(5)).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    let feed = Feed::Resource {
        kind: ResourceKind::Provider,
        id: rig.provider,
    };
    let feed_name = if resource {
        format!("resource:provider:{}", rig.provider)
    } else {
        "user".to_owned()
    };
    client.sub(1, &feed_name, None).await;
    client.expect("subscribed").await;
    if resource {
        super::ephemeral::deliver(
            &rig.server.app.realtime.inner,
            &feed,
            &serde_json::json!({"what":"changed"}),
            None,
        )
        .await?;
    } else {
        inbox(&rig, user, "publication-positive").await?;
    }
    tokio::time::timeout(fixture::PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if if resource {
                frame["op"] == "event" && frame["data"]["row"].is_object()
            } else {
                frame.to_string().contains("publication-positive")
            } {
                break;
            }
        }
    })
    .await?;
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
        .ok_or("the actual authenticated socket owner is absent")?;
    let fresh = rig.token_for(user, chrono::Duration::minutes(5)).await;
    let before = connection.credential();
    let pause = PublicationPause {
        name: connection.name.clone(),
        cut: Arc::new(Publication {
            marker: "publication-old-decision-marker".to_owned(),
            reached: Notify::new(),
            replaced: Notify::new(),
            resume: Notify::new(),
            reauth_resume: Notify::new(),
            retired: Notify::new(),
        }),
    };
    assert!(
        PUBLICATION
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pause.name.clone(), Arc::clone(&pause.cut))
            .is_none()
    );
    if resource {
        sqlx::query("UPDATE providers SET name=$2 WHERE id=$1")
            .bind(rig.provider)
            .bind(&pause.cut.marker)
            .execute(rig.pool.pool())
            .await?;
        super::ephemeral::deliver(
            &rig.server.app.realtime.inner,
            &feed,
            &serde_json::json!({"what":"changed"}),
            None,
        )
        .await?;
    } else {
        inbox(&rig, user, &pause.cut.marker).await?;
    }
    tokio::time::timeout(fixture::PATIENCE, pause.cut.reached.notified()).await?;
    client
        .send(serde_json::json!({"op":"reauth","token":fresh}))
        .await;
    tokio::time::timeout(fixture::PATIENCE, pause.cut.replaced.notified()).await?;
    assert_ne!(
        connection.credential().authentication,
        before.authentication
    );
    pause.cut.resume.notify_one();
    tokio::time::timeout(fixture::PATIENCE, pause.cut.retired.notified()).await?;
    if resource {
        let sub = connection
            .subscriptions()
            .into_iter()
            .find(|sub| sub.feed() == &feed)
            .ok_or("the actual resource subscription disappeared")?;
        assert!(
            sub.state.lock().await.resource_read.is_none(),
            "a rejected frame was never delivered and must permit its replacement"
        );
    }
    pause.cut.reauth_resume.notify_one();
    let marker = pause.cut.marker.clone();
    drop(pause);
    if resource {
        super::ephemeral::deliver(
            &rig.server.app.realtime.inner,
            &feed,
            &serde_json::json!({"what":"changed"}),
            None,
        )
        .await?;
    }
    tokio::time::timeout(fixture::PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if resource {
                if frame["op"] == "event" && frame.to_string().contains(&marker) {
                    break;
                }
                continue;
            }
            assert!(
                !frame.to_string().contains(&marker),
                "old subscription bytes published before retiring its projection"
            );
            if frame["op"] == "resync" {
                assert_eq!(frame["reload"], true);
                break;
            }
        }
    })
    .await?;
    client.send(serde_json::json!({"op":"ping","n":71})).await;
    assert_eq!(client.expect("pong").await["n"], 71);
    rig.shutdown().await;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn reauthentication_after_subscription_check_retires_the_old_socket_projection() -> Proof {
    publication_boundary(false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_reauthentication_after_subscription_check_retires_the_old_socket_projection()
-> ::futures::future::LocalBoxFuture<'static, ::orch_api_base::realtime::origin_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::reauthentication_after_subscription_check_retires_the_old_socket_projection(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn a_rejected_resource_frame_keeps_its_same_row_replacement_available() -> Proof {
    publication_boundary(true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_rejected_resource_frame_keeps_its_same_row_replacement_available()
-> ::futures::future::LocalBoxFuture<'static, ::orch_api_base::realtime::origin_read_tests::Proof> {
    ::std::boxed::Box::pin(
        self::a_rejected_resource_frame_keeps_its_same_row_replacement_available(),
    )
}
