//! Real announcement scopes bind the native audience and keep accepted outcomes after read loss.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::threads::{ThreadQueueResume, ThreadQueueResumeBody};
use orch_api_contract::topics::{AnnouncementCreateBody, NodeAnnouncementCreate};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use rig::Scene;
use serde_json::json;
use std::time::Duration;

const WAIT_NATIVE: Duration = Duration::from_secs(30);
const TEXT: &str = "Native announcement scope marker";

fn body(wake: bool) -> AnnouncementCreateBody {
    AnnouncementCreateBody {
        title: "A scoped announcement".into(),
        body: TEXT.into(),
        category: None,
        severity: None,
        targets: None,
        delivery: None,
        fallback: None,
        wake_idle: Some(wake),
    }
}
fn rule(kind: &str, id: Id, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}
fn writes(topic: Id, recipients: &[Id]) -> Vec<Json> {
    std::iter::once(rule("topic", topic, "NodeAnnouncementCreate", &["write"]))
        .chain(
            recipients
                .iter()
                .map(|id| rule("thread", *id, "NodeAnnouncementCreate", &["write"])),
        )
        .collect()
}
async fn key(s: &Scene, actor: Id, rules: Vec<Json>) -> (Id, String) {
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(policy.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id,
            user_id: actor,
            name: "Scoped native announcements",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(policy)),
    )
    .await
    .unwrap();
    (id, secret)
}
async fn send(s: &Scene, token: &str, place: Id, body: &AnnouncementCreateBody) -> RawReply {
    static LOGS: std::sync::Once = std::sync::Once::new();
    LOGS.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("error,orch_api::native_operations=debug")
                }),
            )
            .with_test_writer()
            .try_init();
    });
    let request = s
        .rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&place])
        .token(token)
        .body(body);
    request.send_bytes().await
}
fn value(reply: RawReply) -> Json {
    assert_eq!(reply.status, 201, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn count(s: &Scene) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM announcements),(SELECT count(*) FROM items WHERE kind='announcement'),(SELECT count(*) FROM thread_inputs WHERE source='announcement')")
        .fetch_one(&s.rig.world.db.pool).await.unwrap()
}

#[tokio::test]
async fn announcement_scopes_bind_each_actual_topic_and_thread_and_keep_write_only_delivery() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let first = s.thread_by(&s.ada, "First recipient").await.base.id;
    let second = s.thread_by(&s.ada, "Second recipient").await.base.id;
    let before = count(&s).await;
    for rules in [
        vec![rule("topic", topic, "NodeAnnouncementCreate", &["write"])],
        vec![
            rule("thread", first, "NodeAnnouncementCreate", &["write"]),
            rule("thread", second, "NodeAnnouncementCreate", &["write"]),
        ],
        vec![
            rule("topic", topic, "NodeAnnouncementCreate", &["read"]),
            rule("thread", first, "NodeAnnouncementCreate", &["read"]),
            rule("thread", second, "NodeAnnouncementCreate", &["read"]),
        ],
        vec![
            rule("topic", topic, "TopicUpdate", &["write"]),
            rule("thread", first, "ThreadInputCreate", &["write"]),
            rule("thread", second, "ThreadInputCreate", &["write"]),
        ],
    ] {
        let (_, secret) = key(&s, s.ada.id(), rules).await;
        let reply = send(&s, &secret, topic, &body(false)).await;
        assert_eq!(reply.status, 403, "{}", String::from_utf8_lossy(&reply.raw));
        assert_eq!(
            count(&s).await,
            before,
            "no native announcement or delivery is licensed by a missing facet"
        );
    }
    let (_, secret) = key(&s, s.ada.id(), writes(topic, &[first, second])).await;
    let accepted = value(send(&s, &secret, topic, &body(false)).await);
    assert_eq!(accepted["projection"], "receipt");
    assert!(accepted["group_id"].as_str().unwrap().parse::<Id>().is_ok());
    assert_eq!(accepted["announcements"].as_array().unwrap().len(), 1);
    assert_eq!(accepted["announcements"][0]["topic_id"], json!(topic));
    assert_eq!(accepted["announcements"][0]["delivered"]["deferred"], 2);
    assert!(accepted["announcements"][0].get("body").is_none());
    assert!(accepted["announcements"][0].get("from").is_none());
    assert_eq!(count(&s).await, (before.0 + 1, before.1 + 2, before.2));
}

#[tokio::test]
async fn announcement_readback_uses_its_own_read_and_preserves_native_supplied_content() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let recipient = s.thread().await.base.id;
    let mut borrowed = writes(topic, &[recipient]);
    borrowed.push(rule("topic", topic, "NodeAnnouncementList", &["read"]));
    let (_, secret) = key(&s, s.ada.id(), borrowed).await;
    assert_eq!(
        value(send(&s, &secret, topic, &body(false)).await)["projection"],
        "receipt"
    );
    let mut own = writes(topic, &[recipient]);
    own.push(rule("topic", topic, "NodeAnnouncementCreate", &["read"]));
    let (_, secret) = key(&s, s.ada.id(), own).await;
    let full = value(send(&s, &secret, topic, &body(false)).await);
    assert!(full.get("projection").is_none());
    assert_eq!(full["announcements"][0]["body"], TEXT);
    assert_eq!(full["announcements"][0]["title"], "A scoped announcement");
    assert_eq!(full["announcements"][0]["delivered"]["deferred"], 1);
    assert!(full["announcements"][0].get("from").is_some());
}

#[tokio::test]
async fn announcement_project_scopes_keep_empty_topics_and_native_skipped_outcomes() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let recipient = s.thread().await.base.id;
    let empty = places::make_topic(
        &s.rig.world,
        &s.ada,
        s.tree.project.base.id,
        "An empty destination",
    )
    .await
    .base
    .id;
    let done = places::make_topic(
        &s.rig.world,
        &s.ada,
        s.tree.project.base.id,
        "Already complete",
    )
    .await
    .base
    .id;
    sqlx::query("UPDATE topics SET status='done' WHERE node_id=$1")
        .bind(done)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let mut all = writes(topic, &[recipient]);
    all.push(rule("topic", empty, "NodeAnnouncementCreate", &["write"]));
    let (_, secret) = key(&s, s.ada.id(), all).await;
    let result = value(send(&s, &secret, s.tree.project.base.id, &body(false)).await);
    let rows = result["announcements"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row["topic_id"] == json!(empty)));
    assert!(rows.iter().all(|row| row["topic_id"] != json!(done)));
    assert_eq!(result["topics_skipped"], 1);
    sqlx::query("UPDATE topics SET status='done' WHERE node_id=ANY($1)")
        .bind(vec![topic, empty])
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let before = count(&s).await;
    let result = value(send(&s, &secret, s.tree.project.base.id, &body(false)).await);
    assert!(result["announcements"].as_array().unwrap().is_empty());
    assert_eq!(result["topics_skipped"], 3);
    assert_eq!(
        count(&s).await,
        before,
        "a real empty native audience has no fabricated target or effect"
    );
}

#[tokio::test]
async fn announcement_keeps_ordinary_refusal_before_any_native_recipient_contention() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let recipient = s
        .thread_by(&s.ada, "Hidden native recipient marker")
        .await
        .base
        .id;
    let (_, secret) = key(&s, s.cai.id(), writes(topic, &[recipient])).await;
    let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
        .bind(recipient)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let reply = tokio::time::timeout(WAIT_NATIVE, send(&s, &secret, topic, &body(false))).await;
    blocker.rollback().await.unwrap();
    let reply = reply.expect("ordinary denial precedes the actual recipient lock");
    assert_eq!(reply.status, 403);
    assert_eq!(count(&s).await.0, 0);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Hidden native recipient marker"));
}

#[tokio::test]
async fn announcement_receipt_survives_publication_wait_without_replaying_the_committed_batch() {
    for change in ["read", "write", "expiry", "association"] {
        let s = Scene::new().await;
        let topic = s.tree.topic.base.id;
        let recipient = s.thread().await.base.id;
        let mut rules = writes(topic, &[recipient]);
        rules.push(rule("topic", topic, "NodeAnnouncementCreate", &["read"]));
        let (id, secret) = key(&s, s.ada.id(), rules).await;
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let mut action = tokio::spawn(async move {
            let request = app
                .call(NodeAnnouncementCreate, &[&topic])
                .token(&secret)
                .body(&body(false));
            request.send_bytes().await
        });
        let reached = tokio::time::timeout(WAIT_NATIVE, async {
            loop {
                if action.is_finished() { break false; }
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.rig.world.db.pool).await.unwrap();
                if waiting { break true; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap_or(false);
        let committed: Option<(Id, String)> =
            sqlx::query_as("SELECT id,xmin::text FROM announcements WHERE topic_id=$1")
                .bind(topic)
                .fetch_optional(&s.rig.world.db.pool)
                .await
                .unwrap();
        if reached && let Some((announcement, _)) = &committed {
            match change {
                "read" => {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(id)
                        .bind(json!({"rules":writes(topic,&[recipient])}))
                        .execute(&s.rig.world.db.pool)
                        .await
                        .unwrap();
                }
                "write" => {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(id).bind(json!({"rules":[rule("topic",topic,"NodeAnnouncementCreate",&["read"])]})).execute(&s.rig.world.db.pool).await.unwrap();
                }
                "expiry" => s.rig.world.advance(chrono::Duration::hours(2)),
                "association" => {
                    sqlx::query("UPDATE announcements SET body='Replacement must not validate the old result' WHERE id=$1")
                        .bind(*announcement).execute(&s.rig.world.db.pool).await.unwrap();
                }
                _ => unreachable!(),
            }
        }
        blocker.rollback().await.unwrap();
        let completed = tokio::time::timeout(WAIT_NATIVE, &mut action).await;
        if completed.is_err() {
            action.abort();
            let _ = action.await;
        }
        let result = value(
            completed
                .expect("publication drains after releasing activity")
                .unwrap(),
        );
        assert!(
            reached,
            "the real post-commit activity wait must be reached"
        );
        let (announcement, xmin) = committed.expect("the native batch must already be committed");
        assert_eq!(count(&s).await, (1, 1, 0));
        assert_eq!(result["announcements"][0]["id"], json!(announcement));
        if change == "write" {
            assert_eq!(
                result["announcements"][0]["body"], TEXT,
                "completed Write is not a readback prerequisite"
            );
        } else {
            assert_eq!(result["projection"], "receipt");
            assert!(result["announcements"][0].get("body").is_none());
        }
        if change != "association" {
            let after: String =
                sqlx::query_scalar("SELECT xmin::text FROM announcements WHERE id=$1")
                    .bind(announcement)
                    .fetch_one(&s.rig.world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(
                after, xmin,
                "response refresh cannot replay or rewrite the batch"
            );
        }
    }
}

async fn notice_sender(s: &Scene, recipient: Id) {
    use orch_engine::access::{ProviderUse, ResourceAccess};
    s.rig.world.app.call(orch_api_contract::access::PermissionPageUpdate, &[&"nodes", &s.tree.topic.base.id])
        .token(&s.ada.token)
        .raw_body(&json!({"rows":[{"permission":"topic.interact","add":[{"holder":{"kind":"user","id":s.cai.id()}}]}]}))
        .send().await.ok();
    let mut conn = s.rig.world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(s.cai.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    assert!(
        !orch_authz::nodes::check(
            &mut conn,
            &principal,
            recipient,
            orch_access_contract::permissions::Permission::ThreadInteract,
            orch_authz::Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    let access = orch_api_core::thread_access::AuthzAccess::new(s.rig.world.app.app.clock.clone());
    assert!(
        !access
            .provider_use(
                &mut conn,
                &ProviderUse {
                    person: s.cai.id(),
                    node: recipient,
                    provider: s.on.provider,
                    model: Some(rig::MODEL),
                    effort: None,
                    runs_in: None,
                }
            )
            .await
            .unwrap()
            .is_allowed(),
        "the sender has no model use to lend to this native notice"
    );
}

async fn hold_queue(s: &Scene, recipient: Id) {
    sqlx::query(
        "UPDATE threads SET queue_held=true,queue_held_by=$2,queue_held_at=$3 WHERE node_id=$1",
    )
    .bind(recipient)
    .bind(s.ada.id())
    .bind(s.rig.world.now())
    .execute(&s.rig.world.db.pool)
    .await
    .unwrap();
}

async fn resume_queue(s: &Scene, recipient: Id, owner_token: &str) {
    // Settle any owner-access hold before reading the queue revision to resume.
    s.rig.engine.preflight(recipient).await.unwrap();
    let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, recipient)
        .await
        .unwrap()
        .unwrap();
    assert!(row.queue_held);
    let queue = s
        .rig
        .world
        .app
        .call(ThreadQueueResume, &[&recipient])
        .token(owner_token)
        .body(&ThreadQueueResumeBody {
            revision: row.queue_revision,
        })
        .send()
        .await
        .ok();
    assert!(!queue.held);
    assert_eq!(
        queue.held_inputs, 0,
        "Resume releases the actual pending input holds"
    );
}

#[tokio::test]
async fn announcement_wake_keeps_the_sender_source_separate_from_native_owner_model_access() {
    let s = Scene::new().await;
    let recipient = s.thread().await.base.id;
    notice_sender(&s, recipient).await;
    hold_queue(&s, recipient).await;
    let (key_id, secret) = key(&s, s.cai.id(), writes(s.tree.topic.base.id, &[recipient])).await;
    let result = value(send(&s, &secret, s.tree.topic.base.id, &body(true)).await);
    assert_eq!(result["announcements"][0]["delivered"]["queued"], 1);
    let input: Id = sqlx::query_scalar(
        "SELECT id FROM thread_inputs WHERE thread_id=$1 AND source='announcement'",
    )
    .bind(recipient)
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.queue_held,
        "the queued notice inherited the actual input hold"
    );
    assert!(row.author_id.is_none());
    assert!(row.run_as.is_none());
    let provenance = &row.submission_authority.as_ref().unwrap()["announcement_provenance"];
    assert_eq!(provenance["sender_id"], json!(s.cai.id()));
    assert_eq!(provenance["operation"], "NodeAnnouncementCreate");
    assert!(provenance["posted_at"].is_string());
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key_id)
        .bind(s.rig.world.now())
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    resume_queue(&s, recipient, &s.ada.token).await;
    s.rig
        .world
        .app
        .app
        .engine
        .preflight(recipient)
        .await
        .unwrap();
    let start = s.rig.turn_start(recipient, 0).await;
    let public = serde_json::to_string(&start).unwrap();
    assert!(public.contains(TEXT));
    assert!(!public.contains("announcement_provenance"));
    assert!(!public.contains("operation_scope"));
    let after = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert!(!after.queue_held);
    assert!(after.author_id.is_none());
    assert!(
        after.run_as.is_none(),
        "native owner use never rewrites the notice as an authored or approved input"
    );
}

#[tokio::test]
async fn deferred_announcement_is_posted_material_after_its_sender_key_is_revoked() {
    let s = Scene::new().await;
    let recipient = s.thread().await.base.id;
    notice_sender(&s, recipient).await;
    let (id, secret) = key(&s, s.cai.id(), writes(s.tree.topic.base.id, &[recipient])).await;
    let posted = value(send(&s, &secret, s.tree.topic.base.id, &body(false)).await);
    assert_eq!(posted["announcements"][0]["delivered"]["deferred"], 1);
    assert_eq!(
        count(&s).await.2,
        0,
        "defer creates no input or sender model authority"
    );
    let announcement: Id =
        serde_json::from_value(posted["announcements"][0]["id"].clone()).unwrap();
    let (provenance, xmin): (Json, String) =
        sqlx::query_as("SELECT posting_provenance,xmin::text FROM announcements WHERE id=$1")
            .bind(announcement)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    assert_eq!(provenance["operation"], "NodeAnnouncementCreate");
    assert_eq!(provenance["sender_id"], json!(s.cai.id()));
    assert!(provenance["posted_at"].is_string());
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(id)
        .bind(s.rig.world.now())
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM api_tokens WHERE id=$1")
            .bind(id)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    assert!(revoked, "the actual posting key is closed before pickup");
    s.say(
        &s.ada,
        recipient,
        "A later owner's turn",
        orch_api_contract::threads::Delivery::Queue,
        "after-posting",
    )
    .await
    .ok();
    let start = s.rig.turn_start(recipient, 0).await;
    let public = serde_json::to_string(&start).unwrap();
    assert!(
        public.contains(TEXT),
        "the later turn consumes the material already posted to its own thread"
    );
    assert!(!public.contains("posting_provenance"));
    assert!(!public.contains("operation_scope"));
    let unchanged: String = sqlx::query_scalar("SELECT xmin::text FROM announcements WHERE id=$1")
        .bind(announcement)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        unchanged, xmin,
        "pickup neither renews nor rewrites the original posting"
    );
}

#[tokio::test]
async fn announcement_input_waits_for_its_receiving_owner_model_use_without_becoming_sender_authored()
 {
    use orch_engine::access::{ProviderUse, ResourceAccess};
    let s = Scene::new().await;
    let recipient = s
        .thread_by(&s.eve, "An owner whose model use can change")
        .await
        .base
        .id;
    notice_sender(&s, recipient).await;
    hold_queue(&s, recipient).await;
    let (_, secret) = key(&s, s.cai.id(), writes(s.tree.topic.base.id, &[recipient])).await;
    value(send(&s, &secret, s.tree.topic.base.id, &body(true)).await);
    let entry: Id = sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='provider' AND object_id=$1 AND permission='provider.use' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(s.on.provider).bind(s.eve.id()).fetch_one(&s.rig.world.db.pool).await.unwrap();
    s.rig
        .world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"providers", &s.on.provider],
        )
        .token(&s.ada.token)
        .raw_body(&json!({"rows":[{"permission":"provider.use","remove":[entry]}]}))
        .send()
        .await
        .ok();
    let access = orch_api_core::thread_access::AuthzAccess::new(s.rig.world.app.app.clock.clone());
    assert!(
        !access
            .provider_use(
                &mut *s.rig.world.conn().await,
                &ProviderUse {
                    person: s.eve.id(),
                    node: recipient,
                    provider: s.on.provider,
                    model: Some(rig::MODEL),
                    effort: None,
                    runs_in: None,
                }
            )
            .await
            .unwrap()
            .is_allowed(),
        "the real receiving owner has genuinely lost model use"
    );
    resume_queue(&s, recipient, &s.eve.token).await;
    s.rig
        .world
        .app
        .app
        .engine
        .preflight(recipient)
        .await
        .unwrap();
    let input: (String, Option<String>, Option<Id>, Json, bool) = sqlx::query_as("SELECT state,state_detail,author_id,submission_authority,queue_held FROM thread_inputs WHERE thread_id=$1 AND source='announcement'")
        .bind(recipient).fetch_one(&s.rig.world.db.pool).await.unwrap();
    assert_eq!(input.0, "pending");
    assert_eq!(input.1.as_deref(), Some("owner_lacks_access"));
    assert!(
        !input.4,
        "only model use holds the input after queue Resume"
    );
    assert!(input.2.is_none());
    assert_eq!(
        input.3["announcement_provenance"]["sender_id"],
        json!(s.cai.id())
    );
    assert_eq!(
        s.rig.commands.issued_of("turn.start") + s.rig.commands.issued_of("turn.start.fenced"),
        0
    );
    s.rig
        .lets_use("provider", s.on.provider, "provider.use", &s.eve, json!({}))
        .await;
    s.rig
        .world
        .app
        .app
        .engine
        .preflight(recipient)
        .await
        .unwrap();
    let start = s.rig.turn_start(recipient, 0).await;
    assert!(serde_json::to_string(&start).unwrap().contains(TEXT));
    assert_eq!(
        count(&s).await,
        (1, 1, 1),
        "model-use recovery reuses the accepted announcement and input"
    );
}

#[tokio::test]
async fn announcement_active_delivery_records_provenance_for_steer_interrupt_and_queue() {
    use orch_api_contract::topics::AnnouncementDelivery;
    for (delivery, outcome) in [
        (AnnouncementDelivery::Steer, "steered"),
        (AnnouncementDelivery::Interrupt, "interrupted"),
        (AnnouncementDelivery::NextTurn, "queued"),
    ] {
        let s = Scene::new().await;
        let recipient = s.thread().await.base.id;
        s.say(
            &s.ada,
            recipient,
            "Existing work",
            orch_api_contract::threads::Delivery::Queue,
            "existing-work",
        )
        .await
        .ok();
        let start = s.rig.turn_start(recipient, 0).await;
        s.rig.say(recipient, rig::turn_began(start.turn_id)).await;
        s.rig.wait_turn(start.turn_id, "running").await;
        let (_, secret) = key(&s, s.ada.id(), writes(s.tree.topic.base.id, &[recipient])).await;
        let mut request = body(false);
        request.delivery = Some(delivery);
        let result = value(send(&s, &secret, s.tree.topic.base.id, &request).await);
        assert_eq!(result["announcements"][0]["delivered"][outcome], 1);
        let announcement: Id =
            serde_json::from_value(result["announcements"][0]["id"].clone()).unwrap();
        let input: (Option<Id>, Json) = sqlx::query_as("SELECT author_id,submission_authority FROM thread_inputs WHERE thread_id=$1 AND source='announcement'")
            .bind(recipient).fetch_one(&s.rig.world.db.pool).await.unwrap();
        assert!(input.0.is_none());
        let provenance = &input.1["announcement_provenance"];
        assert_eq!(provenance["announcement_id"], json!(announcement));
        assert_eq!(provenance["sender_id"], json!(s.ada.id()));
        assert_eq!(provenance["operation"], "NodeAnnouncementCreate");
        assert!(provenance["posted_at"].is_string());
        assert!(
            input.1.get("origins").is_none(),
            "posting provenance must never be an actor credential"
        );
    }
}
