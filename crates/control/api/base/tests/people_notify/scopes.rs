//! Actual scoped notice creation, native waits and standalone-worker delivery.

use super::*;
use async_trait::async_trait;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::{RawReply, TestApp};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::notifications::{channels, outbox, people, routes};
use orch_db::secrets::MasterKey;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery};
use orch_notifier_contract::descriptor::NotifierDescriptor;
use orch_notifier_contract::notifier::Notifier;
use orch_types::Json;
use orch_worker_core::channels::{Notifiers, Surroundings};
use orch_worker_core::heartbeat::Metrics;
use orch_worker_core::outbox::{Outbox, OutboxParts};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, Semaphore};

const OP: &str = "PersonNotifyCreate";
const KIND: &str = "notice-script";

fn rule(kind: &str, id: Id, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":actions}]})
}

async fn key(desk: &Desk, rules: Vec<Json>) -> (Id, String) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id,
            user_id: desk.ada.id(),
            name: "Notice source fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    (id, secret)
}

async fn notify(app: TestApp, secret: String, document: Id, receiver: Id) -> RawReply {
    let request = app
        .call(PersonNotifyCreate, &[&"documents", &document])
        .token(&secret)
        .body(&PersonNotifyBody {
            user_ids: vec![receiver],
            anchor: None,
            note: Some("Please review this exact document".into()),
        });
    request.send_bytes().await
}

struct Script {
    devices: bool,
    held: bool,
    entered: Notify,
    proceed: Semaphore,
    deliveries: Mutex<Vec<Delivery>>,
}

#[async_trait]
impl Notifier for Script {
    fn describe(&self) -> NotifierDescriptor {
        serde_json::from_value(json!({
            "name":KIND,"title":"Notice script","summary":"Native IO control",
            "targets":["person"],"fields":[],"owners":["user"],"single":false,
            "reaches":if self.devices {"devices"} else {"channel"},"body":"text",
            "templates":{"*":{
                "single":{"subject":"[{{org.name}}] {{title}}","body":"{{title}}\n{{where}}\n{{body}}"},
                "digest":{"subject":"[{{org.name}}] {{count}}","body":"{{#notifications}}{{title}}\n{{where}}\n{{/notifications}}"}
            }}
        })).unwrap()
    }
    async fn check(&self, _: &Config) -> Result<Json, DeliverError> {
        Ok(Json::Null)
    }
    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let first = {
            let mut deliveries = self.deliveries.lock().unwrap();
            let first = deliveries.is_empty();
            deliveries.push(delivery);
            first
        };
        if self.held && first {
            self.entered.notify_one();
            self.proceed.acquire().await.unwrap().forget();
        }
        Ok(())
    }
}

struct Scene {
    desk: Desk,
    reader: places::Who,
    document: Id,
    script: Arc<Script>,
    worker: Outbox,
    pool: sqlx::PgPool,
    route: Id,
    channel: Id,
}

impl Scene {
    async fn new(devices: usize, held: bool) -> Self {
        Self::with_recipient(devices, held, false).await
    }
    async fn with_recipient(devices: usize, held: bool, outside: bool) -> Self {
        let desk = Desk::new().await;
        if outside {
            desk.world.set("sharing.allow_external", json!(true)).await;
        }
        let reader = if outside {
            desk.stranger("native-notice-reader").await
        } else {
            desk.member("native-notice-reader", "viewer").await
        };
        let document = desk.make(&desk.ada, "Exact source guide").await.base.id;
        desk.world.advance(chrono::Duration::nanoseconds(567));
        let script = Arc::new(Script {
            devices: devices > 0,
            held,
            entered: Notify::new(),
            proceed: Semaphore::new(0),
            deliveries: Mutex::new(Vec::new()),
        });
        let mut notifiers = Notifiers::new();
        notifiers.register(script.clone()).unwrap();
        let channel = channels::insert_channel(
            &mut *desk.world.conn().await,
            &channels::NewChannel {
                id: new_id(),
                org_id: None,
                user_id: Some(reader.id()),
                kind: KIND,
                name: "Actual notice route",
                config: &json!({}),
                templates: &json!({}),
                enabled: true,
                created_by: reader.id(),
                at: desk.world.now(),
            },
        )
        .await
        .unwrap();
        let route = routes::put_route(
            &mut *desk.world.conn().await,
            new_id(),
            reader.id(),
            channel.id,
            &routes::RouteChange {
                enabled: Some(true),
                ..routes::RouteChange::default()
            },
            desk.world.now(),
        )
        .await
        .unwrap();
        for index in 0..devices {
            people::put_push_subscription(
                &mut *desk.world.conn().await,
                &people::NewPushSubscription {
                    id: new_id(),
                    user_id: reader.id(),
                    endpoint: &format!("https://push.invalid/{index}"),
                    p256dh: "native-fixture-key",
                    auth: "native-fixture-auth",
                    user_agent: &format!("Browser {index}"),
                    at: desk.world.now(),
                },
            )
            .await
            .unwrap();
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*desk.world.db.pool.connect_options()).clone())
            .await
            .unwrap();
        let worker = Outbox::new(OutboxParts {
            db: orch_db::Db::from_pool(pool.clone()),
            clock: desk.world.app.app.clock.clone(),
            notifiers,
            master: MasterKey::new([7; 32]),
            surroundings: Surroundings {
                system_name: "System fallback".into(),
                address_seed: orch_api_contract::public_addresses::PublicAddresses {
                    public_url: "https://orch.test".into(),
                    alternate_urls: Vec::new(),
                },
            },
            retry: orch_notifier_contract::delivery::DEFAULT_RETRY_SCHEDULE.to_vec(),
            metrics: Arc::new(Metrics::new()),
        });
        Self {
            desk,
            reader,
            document,
            script,
            worker,
            pool,
            route: route.id,
            channel: channel.id,
        }
    }
    async fn create(&self, rules: Vec<Json>) -> (Id, String) {
        let (id, secret) = key(&self.desk, rules).await;
        let reply = notify(
            self.desk.world.app.clone(),
            secret.clone(),
            self.document,
            self.reader.id(),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(reply.raw.is_empty());
        self.desk.world.advance(chrono::Duration::minutes(5));
        (id, secret)
    }
    fn own(&self) -> Vec<Json> {
        vec![rule("document", self.document, OP, &["read", "write"])]
    }
    fn sent(&self) -> Vec<Delivery> {
        self.script.deliveries.lock().unwrap().clone()
    }
    async fn state(&self) -> (String, Option<String>, i32) {
        sqlx::query_as("SELECT state,cancel_reason,attempts FROM outbox WHERE route_id=$1")
            .bind(self.route)
            .fetch_one(&self.desk.world.db.pool)
            .await
            .unwrap()
    }
    async fn original_reference(&self) -> Id {
        sqlx::query_scalar("SELECT (data->>'operation_id')::uuid FROM notifications WHERE user_id=$1 AND kind='person.notify'")
            .bind(self.reader.id()).fetch_one(&self.desk.world.db.pool).await.unwrap()
    }
}

#[tokio::test]
async fn notices_require_their_own_subject_read_and_write_and_keep_a_private_exact_origin() {
    let scene = Scene::new(0, false).await;
    for rules in [
        vec![rule("document", scene.document, OP, &["write"])],
        vec![rule("document", scene.document, OP, &["read"])],
        vec![rule(
            "document",
            scene.document,
            "PersonNotifyContextGet",
            &["read", "write"],
        )],
        vec![rule("document", new_id(), OP, &["read", "write"])],
        vec![
            json!({"target":{"kind":"account","id":scene.desk.ada.id(),"reach":"subtree"},
            "resources":["document"],"allow":[{"operation":OP,"actions":["read","write"]}]}),
        ],
    ] {
        let (_, secret) = key(&scene.desk, rules).await;
        assert!(
            notify(
                scene.desk.world.app.clone(),
                secret,
                scene.document,
                scene.reader.id()
            )
            .await
            .status
            .is_client_error()
        );
        assert_eq!(count(&scene.desk, scene.reader.id()).await, 0);
    }
    let (key_id, secret) = scene.create(scene.own()).await;
    let reference = scene.original_reference().await;
    let private = orch_db::queries::notifications::operations::get(
        &mut *scene.desk.world.conn().await,
        reference,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(private["origin"]["operation"], OP);
    assert!(private.to_string().contains(&key_id.to_string()));
    let public: Json = sqlx::query_scalar(
        "SELECT data FROM notifications WHERE user_id=$1 AND kind='person.notify'",
    )
    .bind(scene.reader.id())
    .fetch_one(&scene.desk.world.db.pool)
    .await
    .unwrap();
    for forbidden in [
        secret.as_str(),
        "credentials",
        "token_ids",
        "operation_scope",
    ] {
        assert!(!public.to_string().contains(forbidden));
    }
    scene.worker.run().await.unwrap();
    let sent = scene.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].body.contains("Exact source guide"));
    assert!(!sent[0].body.contains("Acme") && !sent[0].body.contains("Plans"));
    assert!(
        sent[0].notifications[0].node.is_none(),
        "no parent-node Read was lent by Document Read"
    );
    assert_eq!(scene.state().await.0, "sent");
}

#[tokio::test]
async fn native_place_names_need_the_same_notice_operations_optional_read_sources() {
    for own_names in [false, true] {
        let scene = Scene::new(0, false).await;
        let mut rules = scene.own();
        for (kind, id) in [
            ("org", scene.desk.org),
            ("project", scene.desk.project),
            ("topic", scene.desk.topic),
        ] {
            rules.push(rule(
                kind,
                id,
                if own_names { OP } else { "NodeChildList" },
                &["read"],
            ));
        }
        scene.create(rules).await;
        scene.worker.run().await.unwrap();
        let sent = scene.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].body.contains("Acme/Web/Plans"), own_names);
        assert_eq!(sent[0].notifications[0].node.is_some(), own_names);
    }
}

#[tokio::test]
async fn a_coalesced_notice_keeps_each_originals_name_projection_separate() {
    let scene = Scene::new(0, false).await;
    sqlx::query("INSERT INTO notification_prefs(user_id,pause_until) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET pause_until=$2")
        .bind(scene.reader.id()).bind(scene.desk.world.now()+chrono::Duration::minutes(20))
        .execute(&scene.desk.world.db.pool).await.unwrap();
    scene.create(scene.own()).await;
    let mut names = scene.own();
    for (kind, id) in [
        ("org", scene.desk.org),
        ("project", scene.desk.project),
        ("topic", scene.desk.topic),
    ] {
        names.push(rule(kind, id, OP, &["read"]));
    }
    scene.create(names).await;
    scene.worker.run().await.unwrap();
    assert!(scene.sent().is_empty());
    sqlx::query("UPDATE notification_prefs SET pause_until=NULL WHERE user_id=$1")
        .bind(scene.reader.id())
        .execute(&scene.desk.world.db.pool)
        .await
        .unwrap();
    outbox::rearm_held_of(
        &mut *scene.desk.world.conn().await,
        scene.reader.id(),
        scene.desk.world.now(),
    )
    .await
    .unwrap();
    scene.worker.run().await.unwrap();
    let sent = scene.sent();
    assert_eq!(
        sent.len(),
        1,
        "the actual held native unit is rendered as one digest"
    );
    assert_eq!(sent[0].notifications.len(), 2);
    assert!(
        sent[0].notifications[0].node.is_none(),
        "the other origin cannot lend its same-place label"
    );
    assert_eq!(
        sent[0].notifications[1]
            .node
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Plans")
    );
}

async fn wait_for_lock(
    scene: &Scene,
    pending: &tokio::task::JoinHandle<RawReply>,
    pid: i32,
    query: &str,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!pending.is_finished());
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(query).fetch_one(&scene.desk.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn real_preference_and_inbox_receiver_waits_recheck_the_key_before_the_first_notice_effect() {
    for boundary in ["preferences", "receiver"] {
        for expired in [false, true] {
            let scene = Scene::new(0, false).await;
            let (_, secret) = key(&scene.desk, scene.own()).await;
            sqlx::query(
                "INSERT INTO notification_prefs(user_id) VALUES($1) ON CONFLICT DO NOTHING",
            )
            .bind(scene.reader.id())
            .execute(&scene.desk.world.db.pool)
            .await
            .unwrap();
            let mut holder = scene.desk.world.db.pool.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *holder)
                .await
                .unwrap();
            let query = if boundary == "preferences" {
                sqlx::query_scalar::<_, Id>(
                    "SELECT user_id FROM notification_prefs WHERE user_id=$1 FOR UPDATE",
                )
                .bind(scene.reader.id())
                .fetch_one(&mut *holder)
                .await
                .unwrap();
                "%notification_prefs%"
            } else {
                sqlx::query_scalar::<_, Id>("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
                    .bind(scene.reader.id())
                    .fetch_one(&mut *holder)
                    .await
                    .unwrap();
                "%FOR NO KEY UPDATE%"
            };
            let pending = tokio::spawn(notify(
                scene.desk.world.app.clone(),
                secret,
                scene.document,
                scene.reader.id(),
            ));
            wait_for_lock(&scene, &pending, pid, query).await;
            if expired {
                scene.desk.world.advance(chrono::Duration::hours(2));
            }
            holder.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                reply.status,
                if expired {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::NO_CONTENT
                },
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                count(&scene.desk, scene.reader.id()).await,
                i64::from(!expired)
            );
        }
    }
}

#[tokio::test]
async fn standalone_delivery_restores_the_original_key_and_never_borrows_another_reads_operation() {
    for cut in [
        "allowed",
        "inbox_snapshot",
        "expired",
        "revoked",
        "narrowed",
        "unrelated_read",
        "standing",
        "setup",
        "moved",
        "missing_evidence",
        "malformed_evidence",
        "wrong_origin",
        "other_source",
        "other_kind",
    ] {
        let scene = Scene::new(0, false).await;
        let (id, _) = scene.create(scene.own()).await;
        let reference = scene.original_reference().await;
        match cut {
            "inbox_snapshot" => {
                sqlx::query("UPDATE outbox o SET payload=jsonb_set(jsonb_set(o.payload,'{notifications,0,data}',n.data),'{notifications,0,at}',to_jsonb(n.created_at)) FROM notifications n WHERE o.route_id=$1 AND n.user_id=$2 AND n.kind='person.notify'")
                    .bind(scene.route).bind(scene.reader.id()).execute(&scene.desk.world.db.pool).await.unwrap();
            }
            "expired" => scene.desk.world.advance(chrono::Duration::hours(2)),
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(id)
                    .bind(scene.desk.world.now())
                    .execute(&scene.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            "narrowed" | "unrelated_read" => {
                let mut rules = vec![rule("document", scene.document, OP, &["write"])];
                if cut == "unrelated_read" {
                    rules.push(rule("document", scene.document, "DocumentGet", &["read"]));
                }
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(id)
                    .bind(json!({"rules":rules}))
                    .execute(&scene.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            "standing" => {
                sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
                    .bind(scene.desk.ada.id())
                    .execute(&scene.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            "setup" => {
                scene
                    .desk
                    .world
                    .set("auth.second_step", json!("required"))
                    .await
            }
            "moved" => {
                let destination = scene
                    .desk
                    .another_topic("A different allowed source context")
                    .await;
                sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                    .bind(scene.document)
                    .bind(destination)
                    .execute(&scene.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            "missing_evidence" => {
                sqlx::query("DELETE FROM notification_operations WHERE id=$1")
                    .bind(reference)
                    .execute(&scene.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            "malformed_evidence" => {
                sqlx::query("UPDATE notification_operations SET data='{}'::jsonb WHERE id=$1")
                    .bind(reference)
                    .execute(&scene.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            "wrong_origin" => {
                sqlx::query("UPDATE notification_operations SET data=jsonb_set(data,'{origin,operation}','\"DocumentGet\"'::jsonb) WHERE id=$1")
                    .bind(reference).execute(&scene.desk.world.db.pool).await.unwrap();
            }
            "other_source" => {
                sqlx::query("UPDATE outbox SET payload=jsonb_set(payload,'{notifications,0,title}','\"Another accepted source\"'::jsonb) WHERE route_id=$1")
                .bind(scene.route).execute(&scene.desk.world.db.pool).await.unwrap();
            }
            "other_kind" => {
                sqlx::query("UPDATE outbox SET payload=jsonb_set(payload,'{notifications,0,kind}','\"turn.completed\"'::jsonb) WHERE route_id=$1")
                    .bind(scene.route).execute(&scene.desk.world.db.pool).await.unwrap();
            }
            _ => {}
        }
        scene.worker.run().await.unwrap();
        let allowed = matches!(cut, "allowed" | "inbox_snapshot");
        assert_eq!(scene.sent().len(), usize::from(allowed), "{cut}");
        assert_eq!(
            scene.state().await.0,
            if allowed { "sent" } else { "cancelled" },
            "{cut}"
        );
        assert_eq!(
            count(&scene.desk, scene.reader.id()).await,
            1,
            "a closed producer does not undo its accepted Inbox effect"
        );
    }
}

#[tokio::test]
async fn each_further_device_gets_a_fresh_original_source_cut_without_resending_accepted_io() {
    for expired in [false, true] {
        let scene = Scene::new(3, true).await;
        scene.create(scene.own()).await;
        let worker = scene.worker.clone();
        let pending = tokio::spawn(async move { worker.run().await.unwrap() });
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            scene.script.entered.notified(),
        )
        .await
        .unwrap();
        let one: i32 = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            sqlx::query_scalar("SELECT 1").fetch_one(&scene.pool),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            one, 1,
            "the single worker connection is released throughout actual notifier IO"
        );
        if expired {
            scene.desk.world.advance(chrono::Duration::hours(2));
        }
        scene.script.proceed.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        let sent = scene.sent();
        assert_eq!(sent.len(), if expired { 1 } else { 3 });
        let mut targets = sent
            .iter()
            .map(|delivery| delivery.to.to_string())
            .collect::<Vec<_>>();
        targets.sort();
        targets.dedup();
        assert_eq!(
            targets.len(),
            sent.len(),
            "no accepted native device is resent"
        );
        let health: (Option<String>, Option<chrono::DateTime<chrono::Utc>>) =
            sqlx::query_as("SELECT last_error,last_ok_at FROM notification_channels WHERE id=$1")
                .bind(scene.channel)
                .fetch_one(&scene.desk.world.db.pool)
                .await
                .unwrap();
        assert!(
            health.0.is_none() && health.1.is_some(),
            "source closure is no notifier failure"
        );
        assert_eq!(
            scene.state().await.0,
            if expired { "cancelled" } else { "sent" }
        );
    }
}

async fn grant_chat(scene: &Scene, node: bool) -> Id {
    let chat = if node {
        chat_world::chat_of(&scene.desk.world, &scene.desk.ada, scene.desk.topic).await
    } else {
        let first = scene.desk.member("first-group-peer", "viewer").await;
        let second = scene.desk.member("second-group-peer", "viewer").await;
        scene
            .desk
            .world
            .app
            .call(orch_api_contract::chats::OrgChatCreate, &[&scene.desk.org])
            .token(&scene.desk.ada.token)
            .body(&orch_api_contract::chats::OrgChatCreateBody {
                visibility: None,
                home_id: None,
                user_ids: vec![first.id(), second.id()],
                name: Some("Native scoped group".into()),
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap()
            .base
            .id
    };
    let context = scene
        .desk
        .world
        .app
        .call(
            orch_api_contract::notifications::PersonNotifyContextGet,
            &[&"chats", &chat, &scene.reader.id()],
        )
        .token(&scene.desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(context.can_view, Some(false));
    assert!(
        context.gives.contains(&if node {
            AccessChoice::FullView
        } else {
            AccessChoice::FullInteract
        }),
        "the unchanged native grant must actually be offered to this non-reader"
    );
    chat
}

async fn chat_grant_request(
    app: TestApp,
    secret: String,
    chat: Id,
    receiver: Id,
    node: bool,
    notify: bool,
    replay: bool,
) -> RawReply {
    let mut request = app
        .call(
            orch_api_contract::notifications::ChatPersonGrant,
            &[&chat, &receiver],
        )
        .token(&secret)
        .body(&orch_api_contract::notifications::ChatPersonGrantBody {
            link_origin: None,
            choice: if node {
                AccessChoice::FullView
            } else {
                AccessChoice::FullInteract
            },
            notify,
            anchor: None,
        });
    if replay {
        request = request.header(IDEMPOTENCY_HEADER, &chat.to_string());
    }
    request.send_bytes().await
}

async fn sees_chat(scene: &Scene, chat: Id) -> bool {
    scene
        .desk
        .world
        .app
        .call(orch_api_contract::chats::ChatGet, &[&chat])
        .token(&scene.reader.token)
        .send()
        .await
        .status
        .is_success()
}

#[tokio::test]
async fn chat_person_grants_keep_manage_only_native_grants_and_require_own_read_for_a_copy() {
    for node in [false, true] {
        let scene = Scene::with_recipient(0, false, node).await;
        let chat = grant_chat(&scene, node).await;
        let op = "ChatPersonGrant";
        for rules in [
            vec![rule("chat", chat, "ChatGet", &["manage"])],
            vec![rule("chat", chat, op, &["read"])],
            vec![rule("chat", new_id(), op, &["manage"])],
        ] {
            let (_, secret) = key(&scene.desk, rules).await;
            assert!(
                chat_grant_request(
                    scene.desk.world.app.clone(),
                    secret,
                    chat,
                    scene.reader.id(),
                    node,
                    false,
                    false
                )
                .await
                .status
                .is_client_error()
            );
            assert!(!sees_chat(&scene, chat).await);
        }
        for unrelated_read in [false, true] {
            let mut rules = vec![rule("chat", chat, op, &["manage"])];
            if unrelated_read {
                rules.push(rule("chat", chat, "ChatGet", &["read"]));
            }
            let (_, secret) = key(&scene.desk, rules).await;
            assert!(
                chat_grant_request(
                    scene.desk.world.app.clone(),
                    secret,
                    chat,
                    scene.reader.id(),
                    node,
                    true,
                    false
                )
                .await
                .status
                .is_client_error()
            );
            assert!(!sees_chat(&scene, chat).await);
            assert_eq!(count(&scene.desk, scene.reader.id()).await, 0);
        }
        let (_, secret) = key(&scene.desk, vec![rule("chat", chat, op, &["manage"])]).await;
        let reply = chat_grant_request(
            scene.desk.world.app.clone(),
            secret,
            chat,
            scene.reader.id(),
            node,
            false,
            false,
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(reply.raw.is_empty());
        assert!(sees_chat(&scene, chat).await);
        assert_eq!(
            count(&scene.desk, scene.reader.id()).await,
            0,
            "Manage did not copy existing content"
        );
    }
}

#[tokio::test]
async fn a_completed_notifying_chat_grant_replays_a_receipt_without_lending_its_new_key_to_delivery()
 {
    for node in [false, true] {
        let scene = Scene::with_recipient(0, false, node).await;
        let chat = grant_chat(&scene, node).await;
        let (original, secret) = key(
            &scene.desk,
            vec![rule("chat", chat, "ChatPersonGrant", &["manage", "read"])],
        )
        .await;
        let reply = chat_grant_request(
            scene.desk.world.app.clone(),
            secret,
            chat,
            scene.reader.id(),
            node,
            true,
            true,
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(sees_chat(&scene, chat).await);
        assert_eq!(count(&scene.desk, scene.reader.id()).await, 1);
        let reference = scene.original_reference().await;
        let private = orch_db::queries::notifications::operations::get(
            &mut *scene.desk.world.conn().await,
            reference,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(private["origin"]["operation"], "ChatPersonGrant");
        assert!(private.to_string().contains(&original.to_string()));
        let (_, other) = key(
            &scene.desk,
            vec![rule("chat", chat, "ChatPersonGrant", &["manage"])],
        )
        .await;
        let replay = chat_grant_request(
            scene.desk.world.app.clone(),
            other,
            chat,
            scene.reader.id(),
            node,
            true,
            true,
        )
        .await;
        assert_eq!(
            replay.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&replay.raw)
        );
        assert!(replay.raw.is_empty());
        assert_eq!(count(&scene.desk, scene.reader.id()).await, 1);
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(original)
            .bind(scene.desk.world.now())
            .execute(&scene.desk.world.db.pool)
            .await
            .unwrap();
        scene.desk.world.advance(chrono::Duration::minutes(5));
        scene.worker.run().await.unwrap();
        assert!(
            scene.sent().is_empty(),
            "the replay credential cannot replace the original notifying credential"
        );
        assert!(
            sees_chat(&scene, chat).await,
            "accepted access remains granted"
        );
        assert_eq!(count(&scene.desk, scene.reader.id()).await, 1);
        assert_eq!(scene.state().await.0, "cancelled");
    }
}

#[tokio::test]
async fn chat_person_grants_and_notices_recheck_after_real_page_chat_and_receiver_waits() {
    for boundary in ["page", "group", "receiver"] {
        for expired in [false, true] {
            let node = boundary == "page";
            let scene = Scene::with_recipient(0, false, node).await;
            let chat = grant_chat(&scene, node).await;
            let (_, secret) = key(
                &scene.desk,
                vec![rule("chat", chat, "ChatPersonGrant", &["manage", "read"])],
            )
            .await;
            let mut blocker = scene.desk.world.db.pool.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let query = match boundary {
                "page" => {
                    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                        .bind(scene.desk.topic)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM nodes%FOR UPDATE%"
                }
                "group" => {
                    sqlx::query("SELECT id FROM chats WHERE id=$1 FOR UPDATE")
                        .bind(chat)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM chats%FOR UPDATE%"
                }
                _ => {
                    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
                        .bind(scene.reader.id())
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FOR NO KEY UPDATE%"
                }
            };
            let pending = tokio::spawn(chat_grant_request(
                scene.desk.world.app.clone(),
                secret,
                chat,
                scene.reader.id(),
                node,
                true,
                false,
            ));
            wait_for_lock(&scene, &pending, pid, query).await;
            if expired {
                scene.desk.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                reply.status,
                if expired {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::NO_CONTENT
                },
                "{boundary}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                sees_chat(&scene, chat).await,
                !expired,
                "the compound grant rolls back with its refused notice"
            );
            assert_eq!(
                count(&scene.desk, scene.reader.id()).await,
                i64::from(!expired)
            );
        }
    }
}

#[tokio::test]
async fn deferred_chat_grants_use_their_own_operation_and_actual_native_copy_source() {
    for allowed in [false, true] {
        let scene = Scene::with_recipient(0, false, false).await;
        let chat = grant_chat(&scene, false).await;
        let (id, secret) = key(
            &scene.desk,
            vec![rule("chat", chat, "ChatPersonGrant", &["manage", "read"])],
        )
        .await;
        let reply = chat_grant_request(
            scene.desk.world.app.clone(),
            secret,
            chat,
            scene.reader.id(),
            false,
            true,
            false,
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        if !allowed {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(id)
                .bind(json!({"rules":[rule("chat",chat,"ChatPersonGrant",&["manage"]),rule("chat",chat,"ChatGet",&["read"])]}))
                .execute(&scene.desk.world.db.pool).await.unwrap();
        }
        scene.desk.world.advance(chrono::Duration::minutes(5));
        scene.worker.run().await.unwrap();
        assert_eq!(scene.sent().len(), usize::from(allowed));
        assert!(sees_chat(&scene, chat).await);
        assert_eq!(count(&scene.desk, scene.reader.id()).await, 1);
    }
}

#[tokio::test]
async fn completed_chat_grant_headers_return_only_acknowledgement_without_reopening_source_or_writing_again()
 {
    for node in [false, true] {
        let scene = Scene::with_recipient(0, false, node).await;
        let chat = grant_chat(&scene, node).await;
        let receiver = scene.reader.id();
        let header = new_id().to_string();
        let body = orch_api_contract::notifications::ChatPersonGrantBody {
            link_origin: None,
            choice: if node {
                AccessChoice::FullView
            } else {
                AccessChoice::FullInteract
            },
            notify: false,
            anchor: None,
        };
        let send = |secret: String, body: orch_api_contract::notifications::ChatPersonGrantBody| {
            let app = scene.desk.world.app.clone();
            let header = header.clone();
            async move {
                let request = app
                    .call(
                        orch_api_contract::notifications::ChatPersonGrant,
                        &[&chat, &receiver],
                    )
                    .token(&secret)
                    .header(IDEMPOTENCY_HEADER, &header)
                    .body(&body);
                request.send_bytes().await
            }
        };
        let (_, original) = key(
            &scene.desk,
            vec![rule("chat", chat, "ChatPersonGrant", &["manage"])],
        )
        .await;
        assert_eq!(send(original, body.clone()).await.status, 204);
        assert!(sees_chat(&scene, chat).await);
        assert_eq!(count(&scene.desk, receiver).await, 0);
        let (record_before, xmin_before): (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(scene.desk.ada.id())
        .bind(&header)
        .fetch_one(&scene.desk.world.db.pool)
        .await
        .unwrap();
        assert_eq!(record_before["outcome"]["id"], json!(chat));
        assert_eq!(record_before["outcome"]["data"]["person"], json!(receiver));
        assert!(record_before["outcome"]["data"].get("choice").is_none());
        let (_, other) = key(&scene.desk, vec![rule("chat", chat, "ChatGet", &["read"])]).await;
        let ack = send(other.clone(), body.clone()).await;
        assert_eq!(ack.status, 204, "{}", String::from_utf8_lossy(&ack.raw));
        assert!(ack.raw.is_empty());
        let (record_after, xmin_after): (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(scene.desk.ada.id())
        .bind(&header)
        .fetch_one(&scene.desk.world.db.pool)
        .await
        .unwrap();
        assert_eq!(record_after, record_before);
        assert_eq!(xmin_after, xmin_before);
        let mut changed = body;
        changed.notify = true;
        assert_eq!(
            send(other, changed).await.problem().code,
            orch_api_contract::common::ErrorCode::ConflictIdempotencyMismatch
        );
        assert_eq!(count(&scene.desk, receiver).await, 0);
    }
}
