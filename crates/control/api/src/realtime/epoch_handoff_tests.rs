//! Native metadata/projection and actual subscription/outbox order, without scheduler timing.
//! The existing real HTTP/WebSocket diagnostics retain the end-to-end delivery/quietness oracles.

#[path = "../../tests/realtime_support/mod.rs"]
mod fixture;

use super::server::Connection;
use super::subscriptions::Subscription;
use super::{Realtime, Source, Tuning, deltas::Next, server, subscriptions, sync};
use fixture::Rig;
use orch_api_contract::personal::PersonalStateKey;
use orch_api_contract::resources::{EnrollmentProgress, EnrollmentToken};
use orch_db::queries::{access, identity, notifications::people};
use orch_link_contract::enrollment::EnrollRequest;
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_realtime_contract::feeds::{Feed, ResourceKind, UserEvent};
use orch_realtime_contract::frames::{SubFrame, SubId, UnsubFrame};
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use std::future::Future;
use std::sync::Arc;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::oneshot;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Cut {
    Personal,
    Resource,
    MachineAdmitted,
    MachineReady,
}

struct Boundary {
    cut: Cut,
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}
tokio::task_local! { static BOUNDARY: Mutex<Option<Boundary>>; }

pub(super) async fn after_projection(cut: Cut) {
    let boundary = BOUNDARY
        .try_with(|slot| {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|boundary| boundary.cut == cut) {
                slot.take()
            } else {
                None
            }
        })
        .ok()
        .flatten();
    if let Some(boundary) = boundary {
        let _ = boundary.reached.send(());
        let _ = boundary.resume.await;
    }
}

async fn interleave(
    cut: Cut,
    read: impl Future<Output = Proof>,
    write: impl Future<Output = Proof>,
) -> Proof {
    let (reached, wait) = oneshot::channel();
    let (resume, blocked) = oneshot::channel();
    let reader = BOUNDARY.scope(
        Mutex::new(Some(Boundary {
            cut,
            reached,
            resume: blocked,
        })),
        read,
    );
    let writer = async {
        wait.await?;
        write.await?;
        resume
            .send(())
            .map_err(|()| "the paused native reader ended")?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (read, written) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(reader, writer)
    })
    .await?;
    read?;
    written
}

struct Reader {
    realtime: Realtime,
    connection: Arc<Connection>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacing_a_subscription_discards_only_its_queued_frames() -> Proof {
    let rig = Rig::with_time_held().await;
    let reader = Reader::new(&rig, &rig.token).await?;
    let old = reader.subscribe_id(1, Feed::User).await?;
    let other = reader.subscribe_id(2, Feed::User).await?;
    reader.connection.outbox.push_event(
        old.tag(),
        old.feed(),
        json!({"op":"event","id":1,"data":"old private projection"}).to_string(),
    );
    reader.connection.outbox.push_event(
        other.tag(),
        other.feed(),
        json!({"op":"event","id":2,"data":"other reader state"}).to_string(),
    );
    let frame: SubFrame = serde_json::from_value(json!({"id":1,"feed":"user"}))?;
    subscriptions::subscribe(&reader.realtime.inner, &reader.connection, frame).await?;
    let retained = reader.next().await?;
    assert_eq!(
        retained["id"], 2,
        "only the other subscription's event remains"
    );
    assert_eq!(retained["data"], "other reader state");
    let answer = reader.next().await?;
    assert_eq!(answer["op"], "subscribed");
    assert_eq!(answer["id"], 1);
    reader.empty();
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

impl Drop for Reader {
    fn drop(&mut self) {
        // These direct protocol readers have no socket serve-loop to remove its
        // subscription/connection cycle on a failed assertion. Cancel and detach
        // their actual owners even when the normal joined finish was not reached.
        self.realtime.inner.stop.cancel();
        self.realtime.inner.tasks.close();
        for sub in self.connection.subscriptions() {
            let _ = self.connection.take(sub.tag().id);
            self.realtime.inner.unregister(&sub);
        }
    }
}

impl Reader {
    async fn new(rig: &Rig, token: &str) -> Proof<Self> {
        // Only this manually driven reader has no epoch/fanout timers. Its database,
        // session login, native GET projection and subscription implementation are real.
        let realtime = Realtime::tuned(
            &rig.pool,
            &rig.clock,
            Tuning::from_config(&rig.server.app.config),
        );
        realtime.attach_app(&rig.server.app);
        assert!(realtime.inner.engine.set(rig.server.engine.clone()).is_ok());
        let connection = server::epoch_test_connection(&rig.server.app, &realtime.inner, token)
            .await
            .map_err(|(code, _)| format!("native session login refused: {code}"))?;
        Ok(Self {
            realtime,
            connection,
        })
    }

    async fn peer(&self, rig: &Rig, token: &str) -> Proof<Self> {
        let connection =
            server::epoch_test_connection(&rig.server.app, &self.realtime.inner, token)
                .await
                .map_err(|(code, _)| format!("native peer login refused: {code}"))?;
        Ok(Self {
            realtime: self.realtime.clone(),
            connection,
        })
    }

    async fn subscribe(&self, feed: Feed) -> Proof<Arc<Subscription>> {
        self.subscribe_id(1, feed).await
    }

    async fn subscribe_id(&self, id: SubId, feed: Feed) -> Proof<Arc<Subscription>> {
        let frame: SubFrame = serde_json::from_value(json!({"id":id,"feed":feed.to_string()}))?;
        subscriptions::subscribe(&self.realtime.inner, &self.connection, frame).await?;
        let ack = self.next().await?;
        assert_eq!(ack["op"], "subscribed");
        // Join the actual setup poke before creating the tested commit; there
        // are no listener/sweep/epoch loops on this manually driven process.
        self.realtime.inner.tasks.close();
        tokio::time::timeout(Duration::from_secs(10), self.realtime.inner.tasks.wait()).await?;
        let sub = self
            .connection
            .subscriptions()
            .into_iter()
            .find(|sub| sub.tag().id == id)
            .ok_or("the native subscription was not retained")?;
        assert!(sub.is_live(), "setup must finish before the tested commit");
        Ok(sub)
    }

    async fn next(&self) -> Proof<Value> {
        let next =
            tokio::time::timeout(Duration::from_secs(10), self.connection.outbox.next()).await?;
        let Next::Frame(text) = next else {
            return Err("the native reader closed instead of delivering a frame".into());
        };
        self.connection.outbox.done();
        Ok(serde_json::from_str(&text)?)
    }

    async fn epoch(&self) -> Proof<i64> {
        Ok(sync::read_epoch(&self.realtime.inner).await?)
    }

    async fn reset(&self, sub: &Arc<Subscription>) -> Proof {
        let epoch = self.epoch().await?;
        let mut state = sub.state.lock().await;
        assert!(
            !self.realtime.inner.recheck(sub, &mut state, epoch).await?,
            "Inbox rows must stop at their old cut"
        );
        assert!(!sub.is_live());
        Ok(())
    }

    async fn project_resource(&self, sub: &Arc<Subscription>) -> Proof {
        let mut state = sub.state.lock().await;
        self.realtime
            .inner
            .ephemeral
            .current(&self.realtime.inner, sub, &mut state)
            .await?;
        Ok(())
    }

    fn empty(&self) {
        assert_eq!(
            self.connection.outbox.waiting_bytes(),
            0,
            "no stale/reset/duplicate frame may remain"
        );
    }

    /// On failure, name only static changed field paths; native IDs and values stay opaque.
    async fn resource_quiet(
        &self,
        sub: &Arc<Subscription>,
        phase: &'static str,
        expected: &Value,
    ) -> Proof {
        let bytes = self.connection.outbox.waiting_bytes();
        if bytes == 0 {
            return Ok(());
        }
        let unexpected = self.next().await?;
        let frame_kind = match unexpected["op"].as_str() {
            Some("event") => "event",
            Some("resync") => "resync",
            Some("closed") => "closed",
            Some("sync") => "sync",
            _ => "other",
        };
        let same_subscription = unexpected["id"] == sub.tag().id;
        let mut changed = Vec::new();
        differences(
            Some(expected),
            unexpected["data"]["row"].get("value"),
            "",
            &mut changed,
        );
        let recorded_fingerprint_matches_frame = sub.state.lock().await.resource_read
            == Some(orch_types::Hash::of(
                unexpected["data"].to_string().as_bytes(),
            ));
        panic!(
            "{phase}: quiet resource queued bytes={bytes}, frame_kind={frame_kind}, same_subscription={same_subscription}, recorded_fingerprint_matches_frame={recorded_fingerprint_matches_frame}, changed_paths={changed:?}"
        );
    }

    async fn finish(self) {
        for sub in self.connection.subscriptions() {
            subscriptions::unsubscribe(
                &self.realtime.inner,
                &self.connection,
                UnsubFrame { id: sub.tag().id },
            )
            .await;
        }
        self.realtime.stop().await;
    }
}

async fn bump(rig: &Rig) -> Proof {
    let mut conn = rig.pool.acquire().await?;
    access::bump_epoch(&mut conn).await?;
    Ok(())
}

async fn inbox(rig: &Rig, key: &str) -> Proof<people::Announced> {
    let mut conn = rig.pool.acquire().await?;
    Ok(people::announce(
        &mut conn,
        &people::NewNotification {
            user_id: rig.ada,
            kind: "milestone",
            node_id: None,
            thread_id: None,
            approval_id: None,
            chat_id: None,
            title: "Private synthetic Inbox row",
            body: "",
            data: &json!({}),
            dedupe_key: key,
            at: rig.clock.now(),
        },
    )
    .await?
    .ok_or("the unique native Inbox row was not created")?)
}

async fn enroll(rig: &Rig, node: bool) -> Proof<(String, Id)> {
    let (kind, body) = if node {
        ("hosts", json!({"home_id":rig.root,"access":"only_me"}))
    } else {
        ("machines", json!({"home_id":rig.root,"roles":["agent"]}))
    };
    let reply = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/{kind}/enrollment-tokens",
            rig.server.addr
        ))
        .bearer_auth(&rig.token)
        .json(&body)
        .send()
        .await?;
    assert_eq!(reply.status(), reqwest::StatusCode::OK);
    let issued: EnrollmentToken = reply.json().await?;
    let receipt = issued
        .receipt
        .ok_or("native token lacks its issuer receipt")?;
    let certificate = orch_pki::enrollment::PendingEnrollment::start()?;
    let enrolled = rig
        .server
        .app
        .link
        .certificates()
        .enroll(
            &EnrollRequest {
                machine_id: None,
                token: issued.token,
                csr_pem: certificate.csr_pem().to_owned(),
                name: "epoch-handoff-machine".to_owned(),
                platform: Platform {
                    os: Os::Linux,
                    arch: Arch::X86_64,
                },
                connector_version: "1.0.0".to_owned(),
                protocol_versions: vec![1],
                roles: vec![if node { Role::Node } else { Role::Agent }],
            },
            &orch_db::audit::Origin::default(),
        )
        .await
        .map_err(|_| "native enrollment refused")?;
    Ok((receipt, enrolled.machine_id))
}

async fn enrollment_order(authority_first: bool) -> Proof {
    for node in [false, true] {
        let rig = Rig::with_time_held().await;
        let (peer_id, peer_token) = rig.person("epoch-handoff-peer").await;
        let reader = Reader::new(&rig, &rig.token).await?;
        let peer = reader.peer(&rig, &peer_token).await?;
        let sub = reader.subscribe(Feed::User).await?;
        peer.subscribe(Feed::User).await?;
        let old = inbox(&rig, "epoch-old-row").await?;
        {
            let mut state = sub.state.lock().await;
            reader
                .realtime
                .inner
                .deliver(&sub, &mut state, old.inbox_seq, &mut sync::Cache::default())
                .await?;
            assert_eq!(state.read, old.inbox_seq);
            assert!(
                sub.is_live(),
                "the pre-commit queue must contain a native Inbox projection, not an epoch reset"
            );
        }
        assert!(
            reader.connection.outbox.waiting_bytes() > 0,
            "the actual old Inbox projection must be queued"
        );
        let before = reader.epoch().await?;
        let (receipt, machine) = enroll(&rig, node).await?;
        assert!(
            reader.epoch().await? > before,
            "the actual enrollment commit must advance authority"
        );
        {
            let digest = hex::decode(&receipt)?;
            let mut conn = rig.pool.acquire().await?;
            let owned =
                orch_db::queries::resources::enrollment_progress(&mut conn, rig.ada, &digest)
                    .await?
                    .ok_or("the actual native receipt does not belong to the issuer")?;
            assert_eq!(owned.1, Some(machine));
            assert!(
                orch_db::queries::resources::enrollment_progress(&mut conn, peer_id, &digest)
                    .await?
                    .is_none()
            );
        }
        if authority_first {
            reader.reset(&sub).await?;
            // Another real commit before the writer takes the reset must preserve
            // exactly one reset, not purge its only row-repair instruction.
            bump(&rig).await?;
            reader.reset(&sub).await?;
        }
        // Reconstruct only the listener dispatch checkpoint, after the actual native
        // enrollment/owner/token commit. The original HTTP+WS tests cover pg_notify routing.
        reader
            .realtime
            .inner
            .tell_connections(&[rig.ada], &json!({"what":"enrollment","receipt":receipt}));
        tokio::time::timeout(Duration::from_secs(10), reader.realtime.inner.tasks.wait()).await?;
        if !authority_first {
            reader.reset(&sub).await?;
        }
        let reset = reader.next().await?;
        assert_eq!(reset["op"], "resync");
        assert_eq!(reset["feed"], "user");
        assert_eq!(
            reset["reload"], true,
            "authority changes require a fresh projection, not a sequence catch-up"
        );
        let event = reader.next().await?;
        assert_eq!(event["op"], "event");
        assert_eq!(event["data"]["what"], "enrollment");
        assert!(
            event["data"]["receipt"].as_str() == Some(receipt.as_str()),
            "the notice must match the exact issuer receipt"
        );
        let progress: EnrollmentProgress = serde_json::from_value(event["data"]["status"].clone())?;
        assert!(
            progress.receipt == receipt,
            "projected status must retain the same receipt"
        );
        if node {
            let host = progress
                .host
                .ok_or("node enrollment omitted its native Host")?;
            assert!(progress.machine.is_none());
            assert_ne!(host.base.id, machine);
            assert_eq!(host.machine.id, machine);
            assert!(
                host.access
                    .permissions
                    .iter()
                    .any(|permission| permission == "host.view")
            );
        } else {
            assert!(progress.host.is_none());
            assert_eq!(
                progress
                    .machine
                    .ok_or("connector enrollment omitted its native Machine")?
                    .base
                    .id,
                machine
            );
        }
        reader.empty();
        peer.empty();
        // A later real Inbox row remains paused; personal delivery did not revive
        // or rewind the sequenced row lane or replay the purged private row.
        let later = inbox(&rig, "epoch-later-row").await?;
        {
            let mut state = sub.state.lock().await;
            reader
                .realtime
                .inner
                .deliver(
                    &sub,
                    &mut state,
                    later.inbox_seq,
                    &mut sync::Cache::default(),
                )
                .await?;
        }
        reader.empty();
        assert!(!sub.is_live());
        assert_eq!(
            reader
                .realtime
                .inner
                .subscribers_of(Source::Inbox(rig.ada))
                .len(),
            1
        );
        reader.finish().await;
        peer.finish().await;
        rig.server.stop().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notice_first_preserves_the_native_enrollment_status_and_purges_old_inbox_rows() -> Proof {
    enrollment_order(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authority_recheck_first_preserves_the_same_native_issuer_notice() -> Proof {
    enrollment_order(true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_personal_state_stays_current_after_repeated_inbox_resets_and_never_revives_a_closed_reader()
-> Proof {
    let rig = Rig::with_time_held().await;
    let reader = Reader::new(&rig, &rig.token).await?;
    let sub = reader.subscribe(Feed::User).await?;
    bump(&rig).await?;
    reader.reset(&sub).await?;
    let reset = reader.next().await?;
    assert_eq!(reset["op"], "resync");
    assert_eq!(
        reset["reload"], true,
        "authority changes require a fresh projection, not a sequence catch-up"
    );
    for revision in [1, 2] {
        let expected = json!({"synthetic_revision":revision});
        {
            let mut conn = rig.pool.acquire().await?;
            identity::put_interface_state(
                &mut conn,
                rig.ada,
                "sidebar",
                &expected,
                rig.clock.now(),
            )
            .await?;
        }
        reader.connection.outbox.push_event(
            sub.tag(),
            sub.feed(),
            "old-personal-projection-canary".to_owned(),
        );
        bump(&rig).await?;
        reader.reset(&sub).await?;
        reader.empty();
        reader
            .realtime
            .inner
            .personal_event(
                &sub,
                UserEvent::State {
                    key: PersonalStateKey::Sidebar,
                    value: Some(json!({"stale":true})),
                },
            )
            .await?;
        let event = reader.next().await?;
        assert_eq!(event["op"], "event");
        assert_eq!(event["data"]["what"], "state");
        assert_eq!(
            event["data"]["value"], expected,
            "the notice must be reprojected, not replay its supplied value"
        );
        reader.empty();
    }
    let replacement = reader.subscribe(Feed::User).await?;
    reader
        .realtime
        .inner
        .personal_event(&sub, UserEvent::Favorites)
        .await?;
    reader.empty();
    reader
        .realtime
        .inner
        .personal_event(&replacement, UserEvent::Favorites)
        .await?;
    assert_eq!(reader.next().await?["data"]["what"], "favorites");
    subscriptions::unsubscribe(
        &reader.realtime.inner,
        &reader.connection,
        UnsubFrame { id: 1 },
    )
    .await;
    assert_eq!(reader.next().await?["op"], "closed");
    reader
        .realtime
        .inner
        .personal_event(&sub, UserEvent::Favorites)
        .await?;
    reader
        .realtime
        .inner
        .personal_event(&replacement, UserEvent::Favorites)
        .await?;
    reader.empty();
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_exact_session_cannot_receive_personal_news_while_its_other_session_still_can()
-> Proof {
    let rig = Rig::with_time_held().await;
    let other_token = rig.token_for(rig.ada, chrono::Duration::days(1)).await;
    let reader = Reader::new(&rig, &rig.token).await?;
    let other = Reader::new(&rig, &other_token).await?;
    let sub = reader.subscribe(Feed::User).await?;
    let other_sub = other.subscribe(Feed::User).await?;
    let session = reader.connection.session();
    {
        let mut conn = rig.pool.acquire().await?;
        assert!(identity::revoke_session(&mut conn, session, rig.ada, rig.clock.now()).await?);
    }
    reader
        .realtime
        .inner
        .personal_event(&sub, UserEvent::Favorites)
        .await?;
    assert!(
        reader.connection.outbox.closing().is_some(),
        "the current native credential must close, not emit personal data"
    );
    other
        .realtime
        .inner
        .personal_event(&other_sub, UserEvent::Favorites)
        .await?;
    let event = other.next().await?;
    assert_eq!(event["op"], "event");
    assert_eq!(event["data"]["what"], "favorites");
    other.empty();
    reader.finish().await;
    other.finish().await;
    rig.server.stop().await;
    Ok(())
}

async fn grant(rig: &Rig, user: Id, permission: &str) -> Proof<Id> {
    let mut conn = rig.pool.acquire().await?;
    let id = new_id();
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id,
            page: &access::PageKey::new("provider", rig.provider, "live"),
            profile: "any",
            permission,
            holder_kind: "user",
            holder_id: Some(user),
            holder_role: None,
            options: &json!({}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(rig.ada),
            at: rig.clock.now(),
        },
    )
    .await?;
    access::bump_epoch(&mut conn).await?;
    Ok(id)
}

#[derive(Debug)]
struct ChangedPath {
    path: String,
    expected_kind: &'static str,
    actual_kind: &'static str,
}

fn value_kind(value: Option<&Value>) -> &'static str {
    match value {
        None => "missing",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

// Only static contract field names may appear. Config/map keys, native IDs,
// account labels and all values remain opaque, including nested object keys.
fn public_field(key: &str) -> &str {
    match key {
        "access"
        | "admin_only"
        | "admin_access"
        | "admin_place"
        | "participating"
        | "permissions"
        | "administers"
        | "through"
        | "holder"
        | "by"
        | "user"
        | "id"
        | "name"
        | "kind"
        | "address"
        | "display_name"
        | "username"
        | "avatar"
        | "text"
        | "color"
        | "credential_keeper"
        | "runtime_support"
        | "runtime_changes"
        | "owner"
        | "owners"
        | "org_id"
        | "home"
        | "agent"
        | "arrangements"
        | "held_by"
        | "runtime_config"
        | "upstreams"
        | "secrets_set"
        | "config"
        | "subscription"
        | "protocols"
        | "key_set"
        | "key_machine"
        | "machine"
        | "status"
        | "status_detail"
        | "status_at"
        | "facts"
        | "facts_state"
        | "sources"
        | "facts_overridden"
        | "facts_at"
        | "facts_requested_at"
        | "last_resets"
        | "models"
        | "runtime"
        | "harnesses"
        | "revision"
        | "created_at"
        | "updated_at"
        | "placement"
        | "harness_kind"
        | "redacted"
        | "plan"
        | "windows"
        | "limit_reached"
        | "identity"
        | "credential"
        | "available_resets"
        | "extra_usage"
        | "balances"
        | "spend"
        | "ordinary_usage_allowed"
        | "credential_verified"
        | "field_times"
        | "model_name"
        | "parameters"
        | "enabled"
        | "descriptor"
        | "described_at"
        | "last_error" => key,
        _ => "*",
    }
}

fn differences(
    expected: Option<&Value>,
    actual: Option<&Value>,
    path: &str,
    out: &mut Vec<ChangedPath>,
) {
    if expected == actual || out.len() >= 32 {
        return;
    }
    match (expected, actual) {
        (Some(Value::Object(a)), Some(Value::Object(b))) => {
            let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
            for key in keys {
                differences(
                    a.get(key),
                    b.get(key),
                    &format!("{path}/{}", public_field(key)),
                    out,
                );
            }
        }
        (Some(Value::Array(a)), Some(Value::Array(b))) => {
            for index in 0..a.len().max(b.len()) {
                differences(a.get(index), b.get(index), &format!("{path}/{index}"), out);
            }
        }
        _ => out.push(ChangedPath {
            path: path.to_owned(),
            expected_kind: value_kind(expected),
            actual_kind: value_kind(actual),
        }),
    }
}

fn same_native_row(phase: &str, expected: &Value, actual: &Value) {
    if expected == actual {
        return;
    }
    let mut changed = Vec::new();
    differences(Some(expected), Some(actual), "", &mut changed);
    // No additional database read: isolate typed wire round-trip loss from a
    // GET/projection context difference using the already received HTTP row.
    let normalized =
        serde_json::from_value::<orch_api_contract::resources::Provider>(expected.clone())
            .and_then(serde_json::to_value);
    let (decoded, normalized_matches_wire, normalization) = match normalized {
        Ok(normalized) => {
            let mut paths = Vec::new();
            differences(Some(expected), Some(&normalized), "", &mut paths);
            (true, Some(normalized == *actual), paths)
        }
        Err(_) => (false, None, Vec::new()),
    };
    assert!(
        expected == actual,
        "{phase}: native GET and wire row differ; changed_paths={changed:?}; typed_roundtrip_decoded={decoded}; normalized_matches_wire={normalized_matches_wire:?}; normalization_paths={normalization:?}"
    );
}

fn phase<T>(label: &'static str, result: Proof<T>) -> Proof<T> {
    result.map_err(|error| {
        if error.is::<tokio::time::error::Elapsed>() {
            format!("epoch proof phase={label}: the original bounded wait elapsed").into()
        } else {
            error
        }
    })
}

// The direct reader's projection and enqueue have completed on return. If no
// visible replacement was queued, the previously delivered row must still be
// exactly the native GET; a stable representation must never redraw itself.
async fn current_native_row(
    reader: &Reader,
    sub: &Arc<Subscription>,
    label: &'static str,
    native: &Value,
    previous: &Value,
) -> Proof<Value> {
    phase(label, reader.project_resource(sub).await)?;
    let queued = reader.connection.outbox.waiting_bytes() > 0;
    let row = if queued {
        let frame = phase(label, reader.next().await)?;
        assert_eq!(frame["op"], "event", "phase={label}");
        assert_eq!(frame["id"], sub.tag().id, "phase={label}");
        assert!(
            matches!(
                serde_json::from_value::<orch_realtime_contract::feeds::ResourceEvent>(
                    frame["data"].clone()
                )?,
                orch_realtime_contract::feeds::ResourceEvent::Changed {
                    row: Some(_),
                    view_revoked: false
                }
            ),
            "phase={label}"
        );
        frame["data"]["row"]["value"].clone()
    } else {
        previous.clone()
    };
    same_native_row(label, native, &row);
    assert_eq!(
        queued,
        native != previous,
        "phase={label}: unchanged native rows are quiet; a changed row needs one complete replacement"
    );
    reader.empty();
    Ok(row)
}

#[test]
fn native_row_failure_diagnostics_keep_dynamic_keys_and_values_opaque() {
    let a = json!({"access":{"permissions":["sentinel-a"]},"config":{"dynamic-key-a":"sentinel-b"},
        "sources":{"models":{"descriptor":null,"described_at":null,"last_error":null}}});
    let b = json!({"access":{"permissions":["sentinel-c"]},"config":{"dynamic-key-b":"sentinel-d"},
        "sources":{"models":{"descriptor":{"dynamic-descriptor-key":"sentinel-e"},"described_at":"sentinel-f","last_error":{"message":"sentinel-g"}}}});
    let mut paths = Vec::new();
    differences(Some(&a), Some(&b), "", &mut paths);
    let report = format!("{paths:?}");
    assert!(paths.iter().any(|path| path.path == "/access/permissions/0"
        && path.expected_kind == "string"
        && path.actual_kind == "string"));
    assert!(paths.iter().any(|path| path.path == "/config/*"));
    for field in ["descriptor", "described_at", "last_error"] {
        assert!(
            paths
                .iter()
                .any(|path| path.path == format!("/sources/models/{field}"))
        );
    }
    for hidden in [
        "dynamic-key-a",
        "dynamic-key-b",
        "dynamic-descriptor-key",
        "sentinel-a",
        "sentinel-b",
        "sentinel-c",
        "sentinel-d",
        "sentinel-e",
        "sentinel-f",
        "sentinel-g",
    ] {
        assert!(!report.contains(hidden));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_an_environment_during_initial_projection_closes_only_that_acknowledged_reader()
-> Proof {
    for (cut, label) in [
        (
            Cut::MachineAdmitted,
            "environment deletion after native admission",
        ),
        (
            Cut::MachineReady,
            "environment deletion after native projection",
        ),
    ] {
        let rig = Rig::with_time_held().await;
        let reader = Reader::new(&rig, &rig.token).await?;
        // The real socket installs both app and link. This manually driven
        // reader has no socket serve-loop; without its link, reachability is
        // always offline and the native admission cut cannot be reached.
        reader.realtime.attach_link(rig.server.app.link.clone());
        {
            let mut conn = rig.pool.acquire().await?;
            let binding = super::environment_watches::reachable(
                &reader.realtime.inner,
                &mut conn,
                rig.environment,
            )
            .await?;
            assert!(
                matches!(binding, Some((machine, _, 1)) if machine == rig.machine_id),
                "phase={label}: the actual native environment must be online before installing its deletion cut"
            );
        }
        assert_eq!(
            super::watches::decide(
                &reader.realtime.inner,
                rig.ada,
                rig.environment,
                orch_access_contract::permissions::Permission::VncView
            )
            .await?,
            subscriptions::Access::Allowed,
            "phase={label}: current native display View must admit the real projection"
        );
        let sibling = reader.subscribe_id(2, Feed::User).await?;
        let feed = Feed::EnvDisplays(rig.environment);
        let epoch = reader.epoch().await?;
        phase(
            label,
            interleave(
                cut,
                async {
                    subscriptions::subscribe(
                        &reader.realtime.inner,
                        &reader.connection,
                        serde_json::from_value(json!({"id":1,"feed":feed.to_string()}))?,
                    )
                    .await?;
                    Ok(())
                },
                async {
                    // The real ACK has left the outbox before the deletion commits.
                    let ack = phase(
                        "deleted environment: actual acknowledged reader",
                        reader.next().await,
                    )?;
                    assert_eq!(ack["op"], "subscribed");
                    assert_eq!(ack["id"], 1);
                    let sub = reader
                        .connection
                        .subscriptions()
                        .into_iter()
                        .find(|sub| sub.tag().id == 1)
                        .ok_or("the acknowledged machine reader was not retained")?;
                    reader.connection.outbox.push_event(
                        sub.tag(),
                        sub.feed(),
                        "old-display-canary".to_owned(),
                    );
                    let mut conn = rig.pool.acquire().await?;
                    sqlx::query("UPDATE environments SET deleted_at=$2 WHERE id=$1")
                        .bind(rig.environment)
                        .bind(rig.clock.now())
                        .execute(&mut *conn)
                        .await?;
                    drop(conn);
                    assert_eq!(
                        reader.epoch().await?,
                        epoch,
                        "the deletion proof must not borrow an authority announcement"
                    );
                    Ok(())
                },
            )
            .await,
        )
        .map_err(|error| {
            if error.is::<oneshot::error::RecvError>() {
                format!(
                    "phase={label}: native read ended before reaching the named deletion boundary"
                )
                .into()
            } else {
                error
            }
        })?;
        let closed = phase(
            "deleted environment: source-local closed frame",
            reader.next().await,
        )?;
        assert_eq!(closed["op"], "closed");
        assert_eq!(closed["id"], 1);
        assert_eq!(closed["reason"], "deleted");
        assert!(
            reader
                .connection
                .subscriptions()
                .iter()
                .all(|sub| sub.tag().id != 1)
        );
        assert!(reader.connection.outbox.closing().is_none());
        assert!(sibling.is_live());
        reader
            .realtime
            .inner
            .personal_event(&sibling, UserEvent::Favorites)
            .await?;
        let current = phase(
            "deleted environment: same-socket sibling publication",
            reader.next().await,
        )?;
        assert_eq!(current["op"], "event");
        assert_eq!(current["id"], 2);
        assert_eq!(current["data"]["what"], "favorites");
        reader.empty();
        reader.finish().await;
        rig.shutdown().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invalid_projection_on_a_live_environment_remains_an_error_not_a_deleted_reader() -> Proof
{
    let rig = Rig::with_time_held().await;
    let reader = Reader::new(&rig, &rig.token).await?;
    assert_eq!(
        super::watches::decide(
            &reader.realtime.inner,
            rig.ada,
            rig.environment,
            orch_access_contract::permissions::Permission::TerminalManage
        )
        .await?,
        subscriptions::Access::Allowed
    );
    let result = super::machine_views::project(
        &reader.realtime.inner,
        &reader.connection,
        &Feed::EnvTerminals(rig.environment),
        &json!({"terminals":"not a native snapshot"}),
    )
    .await;
    assert!(
        result.is_err(),
        "a live source's malformed projection must retain its actual error"
    );
    reader.finish().await;
    rig.shutdown().await;
    Ok(())
}

fn provider_source_executions(rig: &Rig) -> Vec<orch_testkit::doubles::machine::Execution> {
    rig.machine
        .handler
        .executions()
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                orch_link_contract::commands::CommandKind::ProviderRefresh
                    | orch_link_contract::commands::CommandKind::ProviderDiscoverModels
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_resource_manage_withdrawal_purges_private_progress_then_last_right_withdrawal_closes_the_reader()
-> Proof {
    let rig = Rig::with_time_held_and_joined_provider_sources().await;
    let source_reads = || provider_source_executions(&rig);
    let initial_source_reads = source_reads();
    let (user, token) = rig.person("epoch-resource-reader").await;
    let view = grant(&rig, user, "provider.view").await?;
    let manage = grant(&rig, user, "provider.manage").await?;
    // This independent right can admit the feed without granting the native content View.
    let page_manage = grant(&rig, user, "access.manage").await?;
    let http = reqwest::Client::new();
    let url = format!(
        "http://{}/api/v1/providers/{}",
        rig.server.addr, rig.provider
    );
    let reader = Reader::new(&rig, &token).await?;
    let sub = phase(
        "resource subscription",
        reader
            .subscribe(Feed::Resource {
                kind: ResourceKind::Provider,
                id: rig.provider,
            })
            .await,
    )?;
    let initial = phase("initial native row", reader.next().await)?;
    let mut delivered = initial["data"]["row"]["value"].clone();
    let permissions = initial["data"]["row"]["value"]["access"]["permissions"]
        .as_array()
        .ok_or("native access permissions missing")?;
    assert!(
        permissions
            .iter()
            .any(|permission| permission == "provider.manage")
    );
    {
        let mut tx = rig.pool.begin().await?;
        assert!(access::remove_entry(&mut tx, view, rig.ada, rig.clock.now()).await?);
        access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
    }
    // Removing an explicit lower-level entry does not subtract View supplied by Manage.
    let native = http.get(&url).bearer_auth(&token).send().await?;
    assert_eq!(native.status(), reqwest::StatusCode::OK);
    let native: Value = native.json().await?;
    delivered = current_native_row(
        &reader,
        &sub,
        "explicit View removed, Manage remains",
        &native,
        &delivered,
    )
    .await?;
    let permissions = native["access"]["permissions"]
        .as_array()
        .ok_or("native implied access permissions missing")?;
    assert!(
        permissions
            .iter()
            .any(|permission| permission == "provider.view")
    );
    assert!(
        permissions
            .iter()
            .any(|permission| permission == "provider.manage")
    );
    assert!(
        sub.is_live(),
        "its remaining native Manage right still admits the resource feed"
    );
    reader.project_resource(&sub).await?;
    reader.empty();
    let view = grant(&rig, user, "provider.view").await?;
    let native = http.get(&url).bearer_auth(&token).send().await?;
    assert_eq!(native.status(), reqwest::StatusCode::OK);
    let native: Value = native.json().await?;
    delivered = current_native_row(
        &reader,
        &sub,
        "explicit View restored, Manage remains",
        &native,
        &delivered,
    )
    .await?;
    reader.connection.outbox.push_event(
        sub.tag(),
        sub.feed(),
        "private-progress-canary".to_owned(),
    );
    {
        let mut tx = rig.pool.begin().await?;
        assert!(access::remove_entry(&mut tx, manage, rig.ada, rig.clock.now()).await?);
        access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
    }
    {
        let epoch = reader.epoch().await?;
        let mut state = sub.state.lock().await;
        assert!(
            reader
                .realtime
                .inner
                .recheck(&sub, &mut state, epoch)
                .await?
        );
    }
    let native = http.get(&url).bearer_auth(&token).send().await?;
    assert_eq!(native.status(), reqwest::StatusCode::OK);
    let native: Value = native.json().await?;
    delivered = current_native_row(
        &reader,
        &sub,
        "Manage withdrawn, private progress purged",
        &native,
        &delivered,
    )
    .await?;
    let permissions = delivered["access"]["permissions"]
        .as_array()
        .ok_or("current native access permissions missing")?;
    assert!(
        permissions
            .iter()
            .any(|permission| permission == "provider.view")
    );
    assert!(
        !permissions
            .iter()
            .any(|permission| permission == "provider.manage")
    );
    reader.empty();
    {
        let mut tx = rig.pool.begin().await?;
        assert!(access::remove_entry(&mut tx, view, rig.ada, rig.clock.now()).await?);
        access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
    }
    {
        let epoch = reader.epoch().await?;
        let mut state = sub.state.lock().await;
        assert!(
            reader
                .realtime
                .inner
                .recheck(&sub, &mut state, epoch)
                .await?,
            "page management must keep only this resource feed admitted"
        );
    }
    {
        let mut conn = rig.pool.acquire().await?;
        let principal = subscriptions::principal_for_connection(
            &reader.realtime.inner,
            &mut conn,
            &reader.connection,
        )
        .await?
        .ok_or("the current native credential was lost")?;
        let effective = orch_authz::resources::effective(
            &mut conn,
            &principal,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Provider,
                id: rig.provider,
            },
            orch_authz::Policy::default(),
        )
        .await?;
        let held: Vec<_> = effective.held.iter().map(|held| held.permission).collect();
        assert_eq!(
            held,
            [orch_access_contract::permissions::Permission::AccessManage],
            "the retained right must not imply Provider View"
        );
    }
    let native = http.get(&url).bearer_auth(&token).send().await?;
    assert_eq!(native.status(), reqwest::StatusCode::FORBIDDEN);
    let refusal: Value = native.json().await?;
    assert_eq!(refusal["code"], "authz.denied");
    reader.project_resource(&sub).await?;
    let revoked = phase(
        "View withdrawn, only Access Manage remains",
        reader.next().await,
    )?;
    assert_eq!(revoked["op"], "event");
    assert_eq!(revoked["data"]["what"], "changed");
    assert_eq!(revoked["data"]["view_revoked"], true);
    assert!(
        revoked["data"].get("row").is_none(),
        "losing native View must remove the entire old rich row"
    );
    assert!(matches!(
        serde_json::from_value::<orch_realtime_contract::feeds::ResourceEvent>(
            revoked["data"].clone()
        )?,
        orch_realtime_contract::feeds::ResourceEvent::Changed {
            row: None,
            view_revoked: true
        }
    ));
    assert!(
        sub.is_live(),
        "its independent Access Manage right still admits the resource feed"
    );
    reader.project_resource(&sub).await?;
    reader.empty();
    {
        let mut tx = rig.pool.begin().await?;
        assert!(access::remove_entry(&mut tx, page_manage, rig.ada, rig.clock.now()).await?);
        access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
    }
    {
        let epoch = reader.epoch().await?;
        let mut state = sub.state.lock().await;
        assert!(
            !reader
                .realtime
                .inner
                .recheck(&sub, &mut state, epoch)
                .await?
        );
    }
    let closed = phase(
        "last right withdrawn: subscription revoked",
        reader.next().await,
    )?;
    assert_eq!(closed["op"], "closed");
    assert_eq!(closed["reason"], "revoked");
    assert!(!sub.state.lock().await.open);
    reader.empty();
    assert!(
        source_reads() == initial_source_reads,
        "this permission-only full-row oracle must not race an unjoined later native source observation at the held clock"
    );
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_purged_unsent_native_resource_replacement_is_reprojected_but_private_progress_does_not_break_quietness()
-> Proof {
    let rig = Rig::with_time_held_and_joined_provider_sources().await;
    let initial_source_reads = provider_source_executions(&rig);
    let reader = Reader::new(&rig, &rig.token).await?;
    let sub = reader
        .subscribe(Feed::Resource {
            kind: ResourceKind::Provider,
            id: rig.provider,
        })
        .await?;
    let initial = reader.next().await?;
    assert_eq!(initial["data"]["what"], "changed");
    {
        let mut conn = rig.pool.acquire().await?;
        sqlx::query("UPDATE providers SET name='Queued replacement' WHERE id=$1")
            .bind(rig.provider)
            .execute(&mut *conn)
            .await?;
    }
    reader.project_resource(&sub).await?;
    assert!(reader.connection.outbox.waiting_bytes() > 0);
    bump(&rig).await?;
    {
        let epoch = reader.epoch().await?;
        let mut state = sub.state.lock().await;
        assert!(
            reader
                .realtime
                .inner
                .recheck(&sub, &mut state, epoch)
                .await?
        );
    }
    reader
        .resource_quiet(
            &sub,
            "purged unsent replacement",
            &initial["data"]["row"]["value"],
        )
        .await?;
    reader.empty();
    reader.project_resource(&sub).await?;
    let replacement = reader.next().await?;
    assert_eq!(replacement["op"], "event");
    assert_eq!(
        replacement["data"]["row"]["value"]["name"],
        "Queued replacement"
    );
    reader
        .resource_quiet(
            &sub,
            "delivered replacement",
            &replacement["data"]["row"]["value"],
        )
        .await?;
    reader.empty();
    // This explicitly queued private event is an outbox canary, not a claim
    // that the native provider emitted a new login step.
    reader.connection.outbox.push_event(
        sub.tag(),
        sub.feed(),
        "private-progress-canary".to_owned(),
    );
    bump(&rig).await?;
    {
        let epoch = reader.epoch().await?;
        let mut state = sub.state.lock().await;
        assert!(
            reader
                .realtime
                .inner
                .recheck(&sub, &mut state, epoch)
                .await?
        );
    }
    reader
        .resource_quiet(
            &sub,
            "purged private progress",
            &replacement["data"]["row"]["value"],
        )
        .await?;
    reader.empty();
    reader.project_resource(&sub).await?;
    reader
        .resource_quiet(
            &sub,
            "unchanged native projection",
            &replacement["data"]["row"]["value"],
        )
        .await?;
    reader.empty();
    assert!(
        provider_source_executions(&rig) == initial_source_reads,
        "an unchanged held source cut must not race an unjoined native observation during the quietness proof"
    );
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_personal_projection_cut_reprojects_the_notice_before_publication() -> Proof {
    let rig = Rig::with_time_held().await;
    let reader = Reader::new(&rig, &rig.token).await?;
    let sub = reader.subscribe(Feed::User).await?;
    {
        let mut conn = rig.pool.acquire().await?;
        identity::put_interface_state(
            &mut conn,
            rig.ada,
            "sidebar",
            &json!({"revision":1}),
            rig.clock.now(),
        )
        .await?;
    }
    bump(&rig).await?;
    reader.reset(&sub).await?;
    let reset = reader.next().await?;
    assert_eq!(reset["op"], "resync");
    assert_eq!(
        reset["reload"], true,
        "authority changes require a fresh projection, not a sequence catch-up"
    );
    interleave(
        Cut::Personal,
        async {
            reader
                .realtime
                .inner
                .personal_event(
                    &sub,
                    UserEvent::State {
                        key: PersonalStateKey::Sidebar,
                        value: None,
                    },
                )
                .await?;
            Ok(())
        },
        async {
            let mut tx = rig.pool.begin().await?;
            identity::put_interface_state(
                &mut tx,
                rig.ada,
                "sidebar",
                &json!({"revision":2}),
                rig.clock.now(),
            )
            .await?;
            access::bump_epoch(&mut tx).await?;
            tx.commit().await?;
            Ok(())
        },
    )
    .await?;
    let event = reader.next().await?;
    assert_eq!(event["op"], "event");
    assert_eq!(event["data"]["what"], "state");
    assert_eq!(event["data"]["value"], json!({"revision":2}));
    reader.empty();
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_credential_deadline_crossed_during_paused_inbox_projection_prevents_publication() -> Proof
{
    let rig = Rig::with_time_held().await;
    let short = rig.token_for(rig.ada, chrono::Duration::seconds(1)).await;
    let reader = Reader::new(&rig, &short).await?;
    let sub = reader.subscribe(Feed::User).await?;
    bump(&rig).await?;
    reader.reset(&sub).await?;
    let reset = reader.next().await?;
    assert_eq!(reset["op"], "resync");
    assert_eq!(
        reset["reload"], true,
        "authority changes require a fresh projection, not a sequence catch-up"
    );
    interleave(
        Cut::Personal,
        async {
            reader
                .realtime
                .inner
                .personal_event(&sub, UserEvent::Favorites)
                .await?;
            Ok(())
        },
        async {
            // Advance the fixture's held API clock at the exact projection boundary;
            // no real sleep or longer native/client patience is involved.
            rig.let_time_pass(chrono::Duration::seconds(1));
            Ok(())
        },
    )
    .await?;
    assert!(reader.connection.outbox.closing().is_some());
    assert_eq!(reader.connection.outbox.waiting_bytes(), 0);
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manage_withdrawal_after_native_resource_projection_cannot_publish_the_old_rich_access()
-> Proof {
    let rig = Rig::with_time_held().await;
    let (user, token) = rig.person("epoch-cut-reader").await;
    grant(&rig, user, "provider.view").await?;
    let manage = grant(&rig, user, "provider.manage").await?;
    let reader = Reader::new(&rig, &token).await?;
    let sub = reader
        .subscribe(Feed::Resource {
            kind: ResourceKind::Provider,
            id: rig.provider,
        })
        .await?;
    reader.next().await?;
    interleave(Cut::Resource, reader.project_resource(&sub), async {
        let mut tx = rig.pool.begin().await?;
        assert!(access::remove_entry(&mut tx, manage, rig.ada, rig.clock.now()).await?);
        access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    })
    .await?;
    let event = reader.next().await?;
    assert_eq!(event["op"], "event");
    assert_eq!(event["data"]["what"], "changed");
    let permissions = event["data"]["row"]["value"]["access"]["permissions"]
        .as_array()
        .ok_or("current native access permissions missing")?;
    assert!(
        !permissions
            .iter()
            .any(|permission| permission == "provider.manage")
    );
    assert!(
        permissions
            .iter()
            .any(|permission| permission == "provider.view")
    );
    reader.empty();
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn purging_one_resource_generation_keeps_the_other_views_queued_replacement_and_fingerprint()
-> Proof {
    let rig = Rig::with_time_held_and_joined_provider_sources().await;
    let initial_source_reads = provider_source_executions(&rig);
    let reader = Reader::new(&rig, &rig.token).await?;
    let feed = Feed::Resource {
        kind: ResourceKind::Provider,
        id: rig.provider,
    };
    let first = reader.subscribe_id(1, feed.clone()).await?;
    reader.next().await?;
    let second = reader.subscribe_id(2, feed).await?;
    reader.next().await?;
    {
        let mut conn = rig.pool.acquire().await?;
        sqlx::query("UPDATE providers SET name='Two waiting views' WHERE id=$1")
            .bind(rig.provider)
            .execute(&mut *conn)
            .await?;
    }
    reader.project_resource(&first).await?;
    reader.project_resource(&second).await?;
    let second_fingerprint = second.state.lock().await.resource_read;
    assert!(second_fingerprint.is_some());
    bump(&rig).await?;
    {
        let epoch = reader.epoch().await?;
        let mut state = first.state.lock().await;
        assert!(
            reader
                .realtime
                .inner
                .recheck(&first, &mut state, epoch)
                .await?
        );
        assert!(state.resource_read.is_none());
    }
    assert_eq!(second.state.lock().await.resource_read, second_fingerprint);
    let retained = reader.next().await?;
    assert_eq!(retained["id"], 2);
    assert_eq!(
        retained["data"]["row"]["value"]["name"],
        "Two waiting views"
    );
    reader
        .resource_quiet(
            &second,
            "other generation retained",
            &retained["data"]["row"]["value"],
        )
        .await?;
    reader.empty();
    reader.project_resource(&first).await?;
    assert_eq!(reader.next().await?["id"], 1);
    reader.project_resource(&second).await?;
    reader
        .resource_quiet(
            &second,
            "second generation unchanged",
            &retained["data"]["row"]["value"],
        )
        .await?;
    reader.empty();
    assert!(
        provider_source_executions(&rig) == initial_source_reads,
        "an unchanged held source cut must not race an unjoined native observation during the quietness proof"
    );
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_hints_use_existing_personal_readers_and_keep_exact_session_fences() -> Proof {
    let rig = Rig::with_time_held().await;
    let (_, peer_token) = rig.person("metadata-peer").await;
    let revoked_token = rig.token_for(rig.ada, chrono::Duration::days(1)).await;
    let reader = Reader::new(&rig, &rig.token).await?;
    let peer = reader.peer(&rig, &peer_token).await?;
    let revoked = reader.peer(&rig, &revoked_token).await?;
    let sub = reader.subscribe(Feed::User).await?;
    peer.subscribe(Feed::User).await?;
    revoked.subscribe(Feed::User).await?;
    {
        let mut conn = rig.pool.acquire().await?;
        assert!(
            identity::revoke_session(
                &mut conn,
                revoked.connection.session(),
                rig.ada,
                rig.clock.now()
            )
            .await?
        );
    }
    reader
        .realtime
        .inner
        .tell_meta_changed(Some("committed-request".to_owned()));
    tokio::time::timeout(Duration::from_secs(10), reader.realtime.inner.tasks.wait()).await?;
    for event in [reader.next().await?, peer.next().await?] {
        assert_eq!(event["op"], "event");
        assert_eq!(
            event["data"],
            json!({"what":"meta_changed","request_id":"committed-request"})
        );
    }
    reader.empty();
    peer.empty();
    assert!(
        revoked.connection.outbox.closing().is_some(),
        "a revoked browser session receives no metadata hint"
    );
    reader
        .realtime
        .inner
        .personal_event(&sub, UserEvent::OrgCreation)
        .await?;
    assert_eq!(reader.next().await?["data"], json!({"what":"org_creation"}));
    peer.empty();
    reader.empty();
    revoked.finish().await;
    peer.finish().await;
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nodes_machine_feed_keeps_host_view_until_the_physical_machine_is_deleted() -> Proof {
    let rig = Rig::with_time_held().await;
    let (_, machine) = enroll(&rig, true).await?;
    let (viewer, viewer_token) = rig.person("node-machine-viewer").await;
    let (_, stranger_token) = rig.person("node-machine-stranger").await;
    let host = {
        let mut conn = rig.pool.acquire().await?;
        let host = orch_db::queries::resources::hosting::host_of_machine(&mut conn, machine)
            .await?
            .ok_or("node enrollment did not create its Host facade")?;
        access::insert_entry(
            &mut conn,
            &access::NewEntry {
                id: new_id(),
                page: &access::PageKey::new("host", host, "live"),
                profile: "any",
                permission: "host.view",
                holder_kind: "user",
                holder_id: Some(viewer),
                holder_role: None,
                options: &json!({}),
                expires_at: None,
                note: "",
                source: "page",
                created_by: Some(rig.ada),
                at: rig.clock.now(),
            },
        )
        .await?;
        access::bump_epoch(&mut conn).await?;
        host
    };
    let reader = Reader::new(&rig, &viewer_token).await?;
    let denied = reader.peer(&rig, &stranger_token).await?;
    let feed = Feed::Resource {
        kind: ResourceKind::Machine,
        id: machine,
    };
    subscriptions::subscribe(
        &denied.realtime.inner,
        &denied.connection,
        serde_json::from_value(json!({"id":1,"feed":feed.to_string()}))?,
    )
    .await?;
    let refusal = denied.next().await?;
    assert_eq!(refusal["op"], "error");
    assert_eq!(refusal["code"], "forbidden");
    assert!(denied.connection.subscriptions().is_empty());

    let sub = reader.subscribe(feed).await?;
    let first = reader.next().await?;
    let row = &first["data"]["row"]["value"];
    assert_eq!(row["id"], machine.to_string());
    assert_eq!(row["host"]["id"], host.to_string());
    let permissions = row["access"]["permissions"]
        .as_array()
        .ok_or("native permissions missing")?;
    assert!(permissions.iter().any(|value| value == "host.view"));
    assert!(!permissions.iter().any(|value| value == "machine.view"));
    reader
        .realtime
        .inner
        .ephemeral
        .close_deleted(&reader.realtime.inner)
        .await?;
    assert!(
        sub.is_live(),
        "an active node is not a deleted physical machine"
    );
    reader.empty();

    {
        let mut conn = rig.pool.acquire().await?;
        assert!(
            orch_db::queries::resources::revoke_machine(&mut conn, machine, rig.clock.now())
                .await?
        );
    }
    reader
        .realtime
        .inner
        .ephemeral
        .close_deleted(&reader.realtime.inner)
        .await?;
    assert!(
        sub.is_live(),
        "revocation keeps the page from which its owner enrolls it again"
    );
    reader.project_resource(&sub).await?;
    let revoked = reader.next().await?;
    assert_eq!(revoked["data"]["row"]["value"]["status"], "revoked");
    assert_eq!(revoked["data"]["row"]["value"]["id"], machine.to_string());
    reader.empty();

    reader.connection.outbox.push_event(
        sub.tag(),
        sub.feed(),
        "must-be-purged-before-delete".to_owned(),
    );
    {
        let mut conn = rig.pool.acquire().await?;
        sqlx::query("UPDATE machines SET deleted_at=$2 WHERE id=$1")
            .bind(machine)
            .bind(rig.clock.now())
            .execute(&mut *conn)
            .await?;
    }
    reader
        .realtime
        .inner
        .ephemeral
        .close_deleted(&reader.realtime.inner)
        .await?;
    assert!(!sub.is_live());
    let closed = reader.next().await?;
    assert_eq!(closed["op"], "closed");
    assert_eq!(closed["reason"], "deleted");
    reader.empty();
    denied.finish().await;
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}
