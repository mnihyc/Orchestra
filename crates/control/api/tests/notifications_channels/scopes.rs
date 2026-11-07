//! Stored namespaces, ordinary management and the channel/route first-effect waits.

use super::*;
use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::notifications::{MeNotificationRouteSet, MeNotificationRouteSetBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::{RawReply, TestApp};
use orch_db::queries::identity;
use orch_types::{Json, new_id};

#[derive(Clone, Copy, Debug)]
enum Namespace {
    Person,
    Org,
    System,
}
impl Namespace {
    const ALL: [Self; 3] = [Self::Person, Self::Org, Self::System];

    fn actor(self, o: &Outside) -> &Who {
        match self {
            Self::Person | Self::Org => &o.bea,
            Self::System => &o.ada,
        }
    }
    fn rule(self, o: &Outside, operation: &str, actions: &[&str]) -> Json {
        let (target, resource) = match self {
            Self::Person => (json!({"kind":"account","id":o.bea.id()}), "account"),
            Self::Org => (json!({"kind":"org","id":o.acme,"reach":"self"}), "org"),
            Self::System => (json!({"kind":"system"}), "system"),
        };
        json!({"target":target,"resources":[resource],
            "allow":[{"operation":operation,"actions":actions}]})
    }
    async fn make(self, o: &Outside) -> NotificationChannel {
        match self {
            Self::Person | Self::Org => {
                let far = FarSide::taking().await;
                o.world
                    .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
                    .await;
                if matches!(self, Self::Person) {
                    make_own(
                        o,
                        &o.bea,
                        "Private own hook",
                        &far.url,
                        Some("private-signing-value"),
                    )
                    .await
                    .ok()
                } else {
                    make_org(
                        o,
                        &o.bea,
                        o.acme,
                        &Outside::webhook(
                            "Private org hook",
                            &far.url,
                            Some("private-signing-value"),
                        ),
                    )
                    .await
                    .ok()
                }
            }
            Self::System => make_system(o, &o.ada, &Outside::bot("private-system-token"))
                .await
                .ok(),
        }
    }
}

pub(super) async fn key(o: &Outside, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *o.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Stored channel scope fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: o.world.now(),
            expires_at: Some(o.world.now() + Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}

async fn delete(app: TestApp, secret: String, id: Id) -> RawReply {
    let call = app.call(NotificationChannelDelete, &[&id]);
    call.token(&secret).send_bytes().await
}

async fn exists(o: &Outside, id: Id) -> bool {
    o.count("SELECT count(*) FROM notification_channels WHERE id=$1", id)
        .await
        == 1
}

async fn route(o: &Outside, channel: Id) -> Id {
    let call = o.world.app.call(MeNotificationRouteSet, &[&channel]);
    call.token(&o.cai.token)
        .body(&MeNotificationRouteSetBody {
            enabled: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .id
        .unwrap()
}

async fn wait_for_blocker(
    o: &Outside,
    request: &tokio::task::JoinHandle<RawReply>,
    pid: i32,
    query: &str,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(
                !request.is_finished(),
                "the request must reach its native wait"
            );
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)",
            )
            .bind(pid)
            .bind(query)
            .fetch_one(&o.world.db.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the request reaches the native PostgreSQL lock");
}

fn empty(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(
        reply.raw.is_empty(),
        "removal has no existing-content readback"
    );
}

#[tokio::test]
async fn channel_removal_binds_its_stored_namespace_and_needs_only_its_own_manage() {
    for namespace in Namespace::ALL {
        let o = Outside::new().await;
        let made = namespace.make(&o).await;
        let actor = namespace.actor(&o).id();
        for (operation, actions) in [
            ("NotificationChannelDelete", vec!["read"]),
            ("NotificationChannelUpdate", vec!["manage"]),
        ] {
            let secret = key(&o, actor, vec![namespace.rule(&o, operation, &actions)]).await;
            let reply = delete(o.world.app.clone(), secret, made.id).await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{namespace:?}");
            assert!(exists(&o, made.id).await);
            assert!(!String::from_utf8_lossy(&reply.raw).contains("private-"));
        }
        if !matches!(namespace, Namespace::System) {
            let mut wrong = namespace.rule(&o, "NotificationChannelDelete", &["manage"]);
            wrong["target"]["id"] = json!(new_id());
            let secret = key(&o, actor, vec![wrong]).await;
            assert_eq!(
                delete(o.world.app.clone(), secret, made.id).await.status,
                StatusCode::FORBIDDEN
            );
            assert!(exists(&o, made.id).await);
        }
        let secret = key(
            &o,
            actor,
            vec![namespace.rule(&o, "NotificationChannelDelete", &["manage"])],
        )
        .await;
        empty(delete(o.world.app.clone(), secret, made.id).await);
        assert!(!exists(&o, made.id).await);
        assert_eq!(
            o.count("SELECT count(*) FROM secrets WHERE owner_id=$1", made.id)
                .await,
            0
        );
    }
}

#[tokio::test]
async fn channel_removal_scope_does_not_grant_ordinary_settings_or_system_administration() {
    for namespace in Namespace::ALL {
        let o = Outside::new().await;
        let made = namespace.make(&o).await;
        let secret = key(
            &o,
            o.cai.id(),
            vec![namespace.rule(&o, "NotificationChannelDelete", &["manage"])],
        )
        .await;
        let reply = delete(o.world.app.clone(), secret, made.id).await;
        assert!(matches!(
            reply.status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
        assert!(exists(&o, made.id).await);
        let body = String::from_utf8_lossy(&reply.raw);
        assert!(!body.contains("Private own hook"));
        assert!(!body.contains("private-"));
        assert!(!body.contains("Acme"));
    }
}

#[tokio::test]
async fn channel_removal_rechecks_after_both_channel_and_route_lock_waits_before_mutation() {
    for lock_route in [false, true] {
        for expire in [false, true] {
            let o = Outside::new().await;
            let made = Namespace::Org.make(&o).await;
            let route = route(&o, made.id).await;
            let pending = new_id();
            orch_db::queries::notifications::outbox::insert(
                &mut *o.world.conn().await,
                &orch_db::queries::notifications::outbox::NewRow {
                    id: pending,
                    channel: "webhook",
                    route_id: Some(route),
                    destination_id: None,
                    channel_id: Some(made.id),
                    payload: &json!({}),
                    at: o.world.now(),
                },
            )
            .await
            .unwrap();
            let secret = key(
                &o,
                o.bea.id(),
                vec![Namespace::Org.rule(&o, "NotificationChannelDelete", &["manage"])],
            )
            .await;
            let mut blocker = o.world.db.pool.begin().await.unwrap();
            let (sql, query, target) = if lock_route {
                (
                    "SELECT id FROM notification_routes WHERE id=$1 FOR UPDATE",
                    "SELECT id FROM notification_routes%FOR UPDATE%",
                    route,
                )
            } else {
                (
                    "SELECT id FROM notification_channels WHERE id=$1 FOR UPDATE",
                    "SELECT id FROM notification_channels%FOR UPDATE%",
                    made.id,
                )
            };
            sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
                .bind(target)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let request = tokio::spawn(delete(o.world.app.clone(), secret, made.id));
            wait_for_blocker(&o, &request, pid, query).await;
            assert!(exists(&o, made.id).await);
            if expire {
                o.world.advance(Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap();
            let kept: (Option<Id>, bool, String, Json) = sqlx::query_as(
                "SELECT channel_id,enabled,health,channel_snapshot FROM notification_routes WHERE id=$1",
            )
            .bind(route)
            .fetch_one(&o.world.db.pool)
            .await
            .unwrap();
            let delivery: (String, Option<String>) =
                sqlx::query_as("SELECT state,cancel_reason FROM outbox WHERE id=$1")
                    .bind(pending)
                    .fetch_one(&o.world.db.pool)
                    .await
                    .unwrap();
            if expire {
                assert!(matches!(
                    reply.status,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ));
                assert!(exists(&o, made.id).await);
                assert_eq!(kept.0, Some(made.id));
                assert!(kept.1);
                assert_ne!(kept.2, "channel_removed");
                assert_eq!(delivery, ("pending".into(), None));
                assert_eq!(
                    o.count("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='channel.deleted'", made.id).await,
                    0
                );
            } else {
                empty(reply);
                assert!(!exists(&o, made.id).await);
                assert_eq!(kept.0, None);
                assert!(!kept.1);
                assert_eq!(kept.2, "channel_removed");
                assert_eq!(kept.3["name"], json!(made.name));
                assert_eq!(
                    delivery,
                    ("cancelled".into(), Some("channel_removed".into()))
                );
                assert_eq!(
                    o.count("SELECT count(*) FROM notifications WHERE kind='notification.route_attention' AND data->>'route_id'=($1::uuid)::text", route).await,
                    1
                );
                assert_eq!(
                    o.count("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='channel.deleted'", made.id).await,
                    1
                );
            }
        }
    }
}

#[tokio::test]
async fn completed_channel_removal_keeps_its_empty_ack_after_activity_wait_expiry() {
    for expire in [false, true] {
        let o = Outside::new().await;
        let made = Namespace::Org.make(&o).await;
        let secret = key(
            &o,
            o.bea.id(),
            vec![Namespace::Org.rule(&o, "NotificationChannelDelete", &["manage"])],
        )
        .await;
        let mut blocker = o.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = tokio::spawn(delete(o.world.app.clone(), secret, made.id));
        wait_for_blocker(&o, &request, pid, "%INSERT INTO api_token_activity%").await;
        assert!(!exists(&o, made.id).await);
        if expire {
            o.world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        empty(
            tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(
            o.count(
                "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='channel.deleted'",
                made.id
            )
            .await,
            1,
            "post-commit bookkeeping cannot replay the removal"
        );
    }
}
