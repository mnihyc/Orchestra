//! Native sample scopes, real before-IO waits and exact accepted-result reconciliation.

use super::*;
use chrono::Duration;
use http::StatusCode;
use orch_api_contract::notifications::{MeNotificationRouteSet, MeNotificationRouteSetBody};
use orch_api_core::middleware::authentication::token_hash;
use orch_api_core::testing::{RawReply, TestApp};
use orch_types::{Json, new_id};

const OP: &str = "NotificationChannelTest";
fn rule(kind: &str, id: Option<Id>, op: &str, actions: &[&str]) -> Json {
    let target = match id {
        Some(id) => json!({"kind":kind,"id":id,"reach":"self"}),
        None => json!({"kind":kind,"reach":"self"}),
    };
    json!({"target":target,"resources":[kind],"allow":[{"operation":op,"actions":actions}]})
}
fn system() -> Json {
    rule("system", None, OP, &["manage"])
}
async fn sample(
    app: TestApp,
    token: String,
    id: Id,
    body: NotificationChannelTestBody,
) -> RawReply {
    let call = app.call(NotificationChannelTest, &[&id]);
    call.token(&token).body(&body).send_bytes().await
}
fn body(reply: RawReply, status: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn smtp(o: &Outside) -> (Id, String, String) {
    let host = format!("sample-{}.example", new_id());
    let mut given = Outside::mail_server(&host);
    given.name = format!("Sample {}", new_id());
    let id = make_system(o, &o.ada, &given).await.ok().id;
    (id, host, given.name)
}
fn mail() -> NotificationChannelTestBody {
    NotificationChannelTestBody {
        to: Some("sample-recipient@example.org".into()),
        device_id: None,
    }
}
async fn wait(o: &Outside, pending: &tokio::task::JoinHandle<RawReply>, pid: i32, query: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!pending.is_finished(), "the real source wait must be observed");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(query).fetch_one(&o.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("the actual PostgreSQL source wait");
}
async fn joined(pending: tokio::task::JoinHandle<RawReply>) -> RawReply {
    tokio::time::timeout(std::time::Duration::from_secs(30), pending)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn sample_binds_each_real_owner_manage_without_lending_other_operations_or_account_ancestry()
{
    for owner in ["account", "org", "system"] {
        let o = Outside::new().await;
        let far = FarSide::taking().await;
        o.world
            .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
            .await;
        let (id, actor, target, input) = match owner {
            "account" => (
                make_own(&o, &o.bea, "Own sample", &far.url, None)
                    .await
                    .ok()
                    .id,
                &o.bea,
                Some(o.bea.id()),
                NotificationChannelTestBody::default(),
            ),
            "org" => (
                make_org(
                    &o,
                    &o.bea,
                    o.acme,
                    &Outside::mail_server("org-sample.example"),
                )
                .await
                .ok()
                .id,
                &o.bea,
                Some(o.acme),
                mail(),
            ),
            _ => (
                make_system(&o, &o.ada, &Outside::mail_server("system-sample.example"))
                    .await
                    .ok()
                    .id,
                &o.ada,
                None,
                mail(),
            ),
        };
        for selected in [
            rule(owner, target, "NotificationChannelDelete", &["manage"]),
            rule(owner, target, OP, &["write"]),
            if owner == "system" {
                rule("account", Some(actor.id()), OP, &["manage"])
            } else {
                rule(owner, Some(new_id()), OP, &["manage"])
            },
        ] {
            let token = super::scopes::key(&o, actor.id(), vec![selected]).await;
            assert!(
                sample(o.world.app.clone(), token, id, input.clone())
                    .await
                    .status
                    .is_client_error()
            );
        }
        if owner == "account" {
            let org = json!({"target":{"kind":"org","id":o.acme,"reach":"subtree"},"resources":["account"],"allow":[{"operation":OP,"actions":["manage"]}]});
            let token = super::scopes::key(&o, actor.id(), vec![org]).await;
            assert!(
                sample(o.world.app.clone(), token, id, input.clone())
                    .await
                    .status
                    .is_client_error()
            );
        }
        let token =
            super::scopes::key(&o, actor.id(), vec![rule(owner, target, OP, &["manage"])]).await;
        assert_eq!(
            body(
                sample(o.world.app.clone(), token, id, input).await,
                StatusCode::OK
            ),
            json!({"ok":true,"error":null})
        );
        if owner == "account" {
            assert_eq!(far.count(), 1);
        }
    }
}

#[tokio::test]
async fn scoped_samples_keep_the_native_linked_route_alternative_and_editor_only_alternate_address()
{
    let o = Outside::new().await;
    let far = FarSide::taking().await;
    let id = make_system(
        &o,
        &o.ada,
        &Outside::webhook("Linked sample", &far.url, None),
    )
    .await
    .ok()
    .id;
    let token = super::scopes::key(&o, o.dee.id(), vec![system()]).await;
    assert!(
        sample(
            o.world.app.clone(),
            token.clone(),
            id,
            NotificationChannelTestBody::default()
        )
        .await
        .status
        .is_client_error()
    );
    let call = o.world.app.call(MeNotificationRouteSet, &[&id]);
    call.token(&o.dee.token)
        .body(&MeNotificationRouteSetBody::default())
        .send()
        .await
        .ok();
    assert_eq!(
        body(
            sample(
                o.world.app.clone(),
                token.clone(),
                id,
                NotificationChannelTestBody::default()
            )
            .await,
            StatusCode::OK
        )["ok"],
        true
    );
    let denied = body(
        sample(o.world.app.clone(), token, id, mail()).await,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(denied["code"], "request.invalid");
    assert_eq!(far.count(), 1);
    let other = super::scopes::key(&o, o.dee.id(), vec![system()]).await;
    sqlx::query("DELETE FROM notification_routes WHERE channel_id=$1 AND user_id=$2")
        .bind(id)
        .bind(o.dee.id())
        .execute(&o.world.db.pool)
        .await
        .unwrap();
    assert!(
        sample(
            o.world.app.clone(),
            other,
            id,
            NotificationChannelTestBody::default()
        )
        .await
        .status
        .is_client_error()
    );
}

#[tokio::test]
async fn sample_rechecks_expiry_after_the_actual_channel_wait_before_any_notifier_attempt() {
    for expire in [false, true] {
        let o = Outside::new().await;
        let (id, host, name) = smtp(&o).await;
        let gate = outside::hold_delivery(host);
        gate.release();
        let token = super::scopes::key(&o, o.ada.id(), vec![system()]).await;
        let mut held = o.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM notification_channels WHERE id=$1 FOR UPDATE")
            .bind(id)
            .execute(&mut *held)
            .await
            .unwrap();
        let pending = tokio::spawn(sample(o.world.app.clone(), token, id, mail()));
        wait(&o, &pending, pid, "%notification_channels%FOR SHARE%").await;
        assert!(!sent().iter().any(|entry| entry.body.contains(&name)));
        if expire {
            o.world.app.clock.advance(Duration::hours(2));
        }
        held.commit().await.unwrap();
        let reply = joined(pending).await;
        if expire {
            assert!(reply.status.is_client_error());
        } else {
            assert_eq!(body(reply, StatusCode::OK)["ok"], true);
        }
        assert_eq!(gate.calls(), usize::from(!expire));
    }
}

#[tokio::test]
async fn accepted_sample_keeps_its_result_and_owned_health_after_expiry_without_holding_a_connection()
 {
    let o = Outside::new().await;
    let (id, host, _) = smtp(&o).await;
    sqlx::query("UPDATE notification_channels SET last_error='old failure' WHERE id=$1")
        .bind(id)
        .execute(&o.world.db.pool)
        .await
        .unwrap();
    let gate = outside::hold_delivery(host);
    let token = super::scopes::key(&o, o.ada.id(), vec![system()]).await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*o.world.db.pool.connect_options()).clone())
        .await
        .unwrap();
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::new(
        orch_db::Db::from_pool(pool.clone()),
        o.world.app.clock.clone(),
        registry,
    );
    let pending = tokio::spawn(sample(app, token, id, mail()));
    tokio::time::timeout(std::time::Duration::from_secs(30), gate.entered())
        .await
        .unwrap();
    let one: i32 = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        sqlx::query_scalar("SELECT 1").fetch_one(&pool),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(one, 1);
    o.world.app.clock.advance(Duration::hours(2));
    gate.release();
    assert_eq!(
        body(joined(pending).await, StatusCode::OK),
        json!({"ok":true,"error":null})
    );
    let health: (Option<String>, bool) = sqlx::query_as(
        "SELECT last_error,last_ok_at IS NOT NULL FROM notification_channels WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&o.world.db.pool)
    .await
    .unwrap();
    assert_eq!(health, (None, true));
    assert_eq!(gate.calls(), 1);
    pool.close().await;
}

#[tokio::test]
async fn each_push_attempt_rechecks_original_key_and_actual_remaining_device_without_replaying_the_first()
 {
    for change in ["none", "expire", "scope", "device", "channel", "standing"] {
        let o = Outside::new().await;
        let name = format!("Push {}", new_id());
        let id = make_system(
            &o,
            &o.ada,
            &NotificationChannelCreateBody {
                kind: "push".into(),
                name: name.clone(),
                config: json!({}),
                secrets: None,
                templates: None,
                enabled: None,
            },
        )
        .await
        .ok()
        .id;
        let mut devices = Vec::new();
        for index in 0..3 {
            let device = new_id();
            let endpoint = format!("https://push-{}.example/{}", new_id(), index);
            let mut conn = o.world.conn().await;
            orch_db::queries::notifications::people::put_push_subscription(
                &mut conn,
                &orch_db::queries::notifications::people::NewPushSubscription {
                    id: device,
                    user_id: o.ada.id(),
                    endpoint: &endpoint,
                    p256dh: "test-public-key",
                    auth: "test-auth",
                    user_agent: "fixture",
                    at: o.world.now() - Duration::seconds(3 - index),
                },
            )
            .await
            .unwrap();
            devices.push((device, endpoint));
        }
        let gate = outside::hold_delivery(devices[0].1.clone());
        let token = super::scopes::key(&o, o.ada.id(), vec![system()]).await;
        let pending = tokio::spawn(sample(
            o.world.app.clone(),
            token.clone(),
            id,
            NotificationChannelTestBody::default(),
        ));
        tokio::time::timeout(std::time::Duration::from_secs(30), gate.entered())
            .await
            .unwrap();
        match change {
            "expire" => {
                o.world.app.clock.advance(Duration::hours(2));
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1").bind(token_hash(&token))
                .bind(json!({"rules":[rule("system",None,"NotificationChannelDelete",&["manage"])]})).execute(&o.world.db.pool).await.unwrap();
            }
            "device" => {
                sqlx::query("UPDATE push_subscriptions SET endpoint=$2 WHERE id=$1")
                    .bind(devices[1].0)
                    .bind(format!("https://replaced-{}.example/new", new_id()))
                    .execute(&o.world.db.pool)
                    .await
                    .unwrap();
            }
            "channel" => {
                let call = o.world.app.call(NotificationChannelUpdate, &[&id]);
                call.token(&o.ada.token)
                    .body(&NotificationChannelUpdateBody {
                        name: Some(format!("Changed {}", new_id())),
                        ..Default::default()
                    })
                    .send()
                    .await
                    .ok();
            }
            "standing" => {
                sqlx::query("DELETE FROM notification_routes WHERE channel_id=$1 AND user_id=$2")
                    .bind(id)
                    .bind(o.ada.id())
                    .execute(&o.world.db.pool)
                    .await
                    .unwrap();
                sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
                    .bind(o.ada.id())
                    .execute(&o.world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        gate.release();
        let result = body(joined(pending).await, StatusCode::OK);
        assert_eq!(result["ok"], change == "none");
        let attempts = sent()
            .into_iter()
            .filter(|entry| entry.body.contains(&name))
            .collect::<Vec<_>>();
        assert_eq!(attempts.len(), if change == "none" { 3 } else { 1 });
        assert_eq!(attempts[0].to["endpoint"], devices[0].1);
        assert_eq!(gate.calls(), 1);
        let health: Option<String> =
            sqlx::query_scalar("SELECT last_error FROM notification_channels WHERE id=$1")
                .bind(id)
                .fetch_one(&o.world.db.pool)
                .await
                .unwrap();
        assert!(
            health.is_none(),
            "a source interruption is no channel failure"
        );
    }
}

#[tokio::test]
async fn sample_publication_preserves_one_accepted_result_after_its_actual_activity_wait() {
    for expire in [false, true] {
        let o = Outside::new().await;
        let (id, host, _) = smtp(&o).await;
        let gate = outside::hold_delivery(host);
        gate.release();
        let token = super::scopes::key(&o, o.ada.id(), vec![system()]).await;
        let mut held = o.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *held)
            .await
            .unwrap();
        let pending = tokio::spawn(sample(o.world.app.clone(), token, id, mail()));
        wait(&o, &pending, pid, "%INSERT INTO api_token_activity%").await;
        assert_eq!(gate.calls(), 1);
        if expire {
            o.world.app.clock.advance(Duration::hours(2));
        }
        held.commit().await.unwrap();
        assert_eq!(
            body(joined(pending).await, StatusCode::OK),
            json!({"ok":true,"error":null})
        );
        assert_eq!(gate.calls(), 1);
    }
}
