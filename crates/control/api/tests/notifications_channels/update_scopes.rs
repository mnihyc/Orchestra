//! Channel updates preserve one validation, actual stored custody and completed-effect receipts.

use super::*;
use http::StatusCode;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::{RawReply, TestApp};
use orch_types::{Json, new_id};

const OP: &str = "NotificationChannelUpdate";
#[derive(Clone, Copy)]
enum Owner {
    Person,
    Org,
    System,
}
impl Owner {
    const ALL: [Self; 3] = [Self::Person, Self::Org, Self::System];
    fn actor(self, o: &Outside) -> &Who {
        if matches!(self, Self::System) {
            &o.ada
        } else {
            &o.bea
        }
    }
    fn rule(self, o: &Outside, operation: &str, actions: &[&str]) -> Json {
        let (target, resource) = match self {
            Self::Person => (json!({"kind":"account","id":o.bea.id()}), "account"),
            Self::Org => (json!({"kind":"org","id":o.acme,"reach":"self"}), "org"),
            Self::System => (json!({"kind":"system"}), "system"),
        };
        json!({"target":target,"resources":[resource],"allow":[{"operation":operation,"actions":actions}]})
    }
    async fn make(self, o: &Outside) -> Id {
        match self {
            Self::Person => {
                let far = FarSide::taking().await;
                o.world
                    .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
                    .await;
                make_own(
                    o,
                    &o.bea,
                    "Private old hook",
                    &far.url,
                    Some("private-signing-value"),
                )
                .await
                .ok()
                .id
            }
            Self::Org => {
                make_org(o, &o.bea, o.acme, &Outside::bot("private-org-token"))
                    .await
                    .ok()
                    .id
            }
            Self::System => {
                make_system(o, &o.ada, &Outside::bot("private-system-token"))
                    .await
                    .ok()
                    .id
            }
        }
    }
}
async fn update(
    app: TestApp,
    token: String,
    id: Id,
    body: NotificationChannelUpdateBody,
) -> RawReply {
    let call = app.call(NotificationChannelUpdate, &[&id]);
    call.token(&token).body(&body).send_bytes().await
}
fn rename() -> NotificationChannelUpdateBody {
    NotificationChannelUpdateBody {
        name: Some("Supplied new name".into()),
        ..Default::default()
    }
}
fn value(reply: RawReply, expected: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        expected,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn receipt(id: Id) -> Json {
    json!({"projection":"receipt","channel_id":id})
}
async fn name(o: &Outside, id: Id) -> String {
    sqlx::query_scalar("SELECT name FROM notification_channels WHERE id=$1")
        .bind(id)
        .fetch_one(&o.world.db.pool)
        .await
        .unwrap()
}
async fn wait(o: &Outside, request: &tokio::task::JoinHandle<RawReply>, pid: i32, query: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the real native wait must be reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(query).fetch_one(&o.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the request reaches its actual PostgreSQL wait");
}

#[tokio::test]
async fn channel_update_binds_real_owner_manage_keeps_effects_and_never_lends_read() {
    for owner in Owner::ALL {
        let o = Outside::new().await;
        let id = owner.make(&o).await;
        let token = super::scopes::key(
            &o,
            owner.actor(&o).id(),
            vec![owner.rule(&o, OP, &["manage"])],
        )
        .await;
        assert_eq!(
            value(
                update(o.world.app.clone(), token, id, rename()).await,
                StatusCode::OK
            ),
            receipt(id)
        );
        assert_eq!(name(&o, id).await, "Supplied new name");
        let unrelated = super::scopes::key(
            &o,
            owner.actor(&o).id(),
            vec![
                owner.rule(&o, OP, &["manage"]),
                owner.rule(&o, "NotificationChannelList.available", &["read"]),
            ],
        )
        .await;
        assert_eq!(
            value(
                update(
                    o.world.app.clone(),
                    unrelated,
                    id,
                    NotificationChannelUpdateBody {
                        enabled: Some(false),
                        ..Default::default()
                    }
                )
                .await,
                StatusCode::OK
            ),
            receipt(id)
        );
        for rules in [
            vec![owner.rule(&o, OP, &["read"])],
            vec![owner.rule(&o, "NotificationChannelDelete", &["manage"])],
        ] {
            let token = super::scopes::key(&o, owner.actor(&o).id(), rules).await;
            assert_eq!(
                update(
                    o.world.app.clone(),
                    token,
                    id,
                    NotificationChannelUpdateBody {
                        name: Some("Forbidden".into()),
                        ..Default::default()
                    }
                )
                .await
                .status,
                StatusCode::FORBIDDEN
            );
            assert_eq!(name(&o, id).await, "Supplied new name");
        }
        let unprivileged = super::scopes::key(
            &o,
            o.cai.id(),
            vec![owner.rule(&o, OP, &["manage", "read"])],
        )
        .await;
        assert!(matches!(
            update(o.world.app.clone(), unprivileged, id, rename())
                .await
                .status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
    }
}

#[tokio::test]
async fn channel_update_readback_keeps_native_content_and_independent_actual_account_routes() {
    let o = Outside::new().await;
    let id = Owner::Org.make(&o).await;
    let call = o.world.app.call(
        orch_api_contract::notifications::MeNotificationRouteLinkCreate,
        &[&id],
    );
    let link = call.token(&o.bea.token).send().await.ok();
    let without_account = super::scopes::key(
        &o,
        o.bea.id(),
        vec![Owner::Org.rule(&o, OP, &["manage", "read"])],
    )
    .await;
    let body = value(
        update(o.world.app.clone(), without_account, id, rename()).await,
        StatusCode::OK,
    );
    assert_eq!(body["id"], json!(id));
    assert_eq!(body["name"], "Supplied new name");
    assert!(body["route"]["id"].is_null());
    assert!(body["route"]["link"].is_null());
    let token = super::scopes::key(
        &o,
        o.bea.id(),
        vec![
            Owner::Org.rule(&o, OP, &["manage", "read"]),
            Owner::Person.rule(&o, OP, &["read"]),
        ],
    )
    .await;
    let body = value(
        update(
            o.world.app.clone(),
            token,
            id,
            NotificationChannelUpdateBody::default(),
        )
        .await,
        StatusCode::OK,
    );
    let native = o
        .channels(&o.bea)
        .await
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(body, json!(native));
    assert_eq!(body["route"]["link"]["url"], link.url);
    assert!(!body.to_string().contains("private-org-token"));
}

#[tokio::test]
async fn channel_update_rechecks_key_expiry_after_its_actual_channel_row_wait() {
    for expire in [false, true] {
        let o = Outside::new().await;
        let id = Owner::Org.make(&o).await;
        let old = name(&o, id).await;
        let token =
            super::scopes::key(&o, o.bea.id(), vec![Owner::Org.rule(&o, OP, &["manage"])]).await;
        let mut blocker = o.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM notification_channels WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pending = tokio::spawn(update(o.world.app.clone(), token, id, rename()));
        wait(
            &o,
            &pending,
            pid,
            "SELECT %FROM notification_channels%FOR NO KEY UPDATE%",
        )
        .await;
        assert_eq!(name(&o, id).await, old);
        if expire {
            o.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert!(matches!(
                reply.status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ));
            assert_eq!(name(&o, id).await, old);
        } else {
            assert_eq!(value(reply, StatusCode::OK), receipt(id));
            assert_eq!(name(&o, id).await, "Supplied new name");
        }
    }
}

#[tokio::test]
async fn notifier_validation_runs_once_outside_the_connection_and_never_outlives_its_origin() {
    for close in [0, 1, 2] {
        let o = Outside::new().await;
        let host = format!("held-{}.example", new_id());
        let id = make_system(&o, &o.ada, &Outside::mail_server(&host))
            .await
            .ok()
            .id;
        let gate = outside::hold_check(host);
        let token = super::scopes::key(
            &o,
            o.ada.id(),
            vec![Owner::System.rule(&o, OP, &["manage"])],
        )
        .await;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*o.world.db.pool.connect_options()).clone())
            .await
            .unwrap();
        let mut registry = orch_api_core::registry::Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let caller = TestApp::new(
            orch_db::Db::from_pool(pool.clone()),
            o.world.app.clock.clone(),
            registry,
        );
        let body = NotificationChannelUpdateBody {
            secrets: Some(BTreeMap::from([("password".into(), "new-password".into())])),
            ..Default::default()
        };
        let pending = tokio::spawn(update(caller, token.clone(), id, body));
        tokio::time::timeout(std::time::Duration::from_secs(30), gate.entered())
            .await
            .unwrap();
        assert_eq!(gate.calls(), 1);
        let _: i32 = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            sqlx::query_scalar("SELECT 1::integer")
                .fetch_one(&pool)
                .await
                .unwrap()
        })
        .await
        .expect("the native initial supplier does not keep its snapshot connection");
        if close == 1 {
            o.world.advance(chrono::Duration::hours(2));
        }
        if close == 2 {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                .bind(token_hash(&token))
                .bind(json!({"rules":[Owner::System.rule(&o, OP, &["read"])]}))
                .execute(&o.world.db.pool)
                .await
                .unwrap();
        }
        gate.release();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if close == 0 {
            assert_eq!(value(reply, StatusCode::OK), receipt(id));
        } else {
            assert!(matches!(
                reply.status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ));
            assert_eq!(o.count("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='channel.updated'", id).await, 0);
        }
        assert_eq!(
            gate.calls(),
            1,
            "no guarded reprepare/publication may rerun the native notifier check"
        );
    }
}

#[tokio::test]
async fn a_changed_native_validation_input_is_not_overwritten_or_rechecked_by_a_retry() {
    let o = Outside::new().await;
    let host = format!("held-{}.example", new_id());
    let id = make_system(&o, &o.ada, &Outside::mail_server(&host))
        .await
        .ok()
        .id;
    let gate = outside::hold_check(host);
    let token = super::scopes::key(
        &o,
        o.ada.id(),
        vec![Owner::System.rule(&o, OP, &["manage"])],
    )
    .await;
    let pending = tokio::spawn(update(
        o.world.app.clone(),
        token,
        id,
        NotificationChannelUpdateBody {
            secrets: Some(BTreeMap::from([("password".into(), "new-password".into())])),
            ..Default::default()
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(30), gate.entered())
        .await
        .unwrap();
    sqlx::query("UPDATE notification_channels SET config=jsonb_set(config,'{host}','\"changed.example\"'::jsonb) WHERE id=$1")
        .bind(id).execute(&o.world.db.pool).await.unwrap();
    gate.release();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::CONFLICT);
    let host: String =
        sqlx::query_scalar("SELECT config->>'host' FROM notification_channels WHERE id=$1")
            .bind(id)
            .fetch_one(&o.world.db.pool)
            .await
            .unwrap();
    assert_eq!(host, "changed.example");
    assert_eq!(gate.calls(), 1);
    assert_eq!(
        o.count(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='channel.updated'",
            id
        )
        .await,
        0
    );
}

#[tokio::test]
async fn channel_update_rolls_back_if_its_real_secret_write_wait_outlives_the_http_key() {
    let o = Outside::new().await;
    let id = make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok()
        .id;
    let token = super::scopes::key(
        &o,
        o.ada.id(),
        vec![Owner::System.rule(&o, OP, &["manage"])],
    )
    .await;
    let mut blocker = o.world.db.pool.begin().await.unwrap();
    let old: Vec<u8> = sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_id=$1 AND name='password' FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut *blocker)
    .await
    .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(update(
        o.world.app.clone(),
        token,
        id,
        NotificationChannelUpdateBody {
            name: Some("Never committed".into()),
            secrets: Some(BTreeMap::from([("password".into(), "new-password".into())])),
            ..Default::default()
        },
    ));
    wait(&o, &pending, pid, "%INSERT INTO secrets%").await;
    o.world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        reply.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(name(&o, id).await, "Mail");
    let current: Vec<u8> =
        sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_id=$1 AND name='password'")
            .bind(id)
            .fetch_one(&o.world.db.pool)
            .await
            .unwrap();
    assert_eq!(current, old);
}

#[tokio::test]
async fn channel_update_publication_keeps_the_committed_receipt_and_does_not_readmit_manage() {
    for cut in [0, 1, 2] {
        let o = Outside::new().await;
        let id = Owner::System.make(&o).await;
        let token = super::scopes::key(
            &o,
            o.ada.id(),
            vec![Owner::System.rule(&o, OP, &["manage", "read"])],
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
        let pending = tokio::spawn(update(o.world.app.clone(), token.clone(), id, rename()));
        wait(&o, &pending, pid, "%INSERT INTO api_token_activity%").await;
        assert_eq!(name(&o, id).await, "Supplied new name");
        let version: String =
            sqlx::query_scalar("SELECT xmin::text FROM notification_channels WHERE id=$1")
                .bind(id)
                .fetch_one(&o.world.db.pool)
                .await
                .unwrap();
        if cut == 1 {
            o.world.advance(chrono::Duration::hours(2));
        }
        if cut == 2 {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                .bind(token_hash(&token))
                .bind(json!({"rules":[Owner::System.rule(&o, OP, &["read"])]}))
                .execute(&o.world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let body = value(
            tokio::time::timeout(std::time::Duration::from_secs(30), pending)
                .await
                .unwrap()
                .unwrap(),
            StatusCode::OK,
        );
        if cut == 1 {
            assert_eq!(body, receipt(id));
        } else {
            assert_eq!(body["id"], json!(id));
            assert_eq!(body["name"], "Supplied new name");
        }
        let after: String =
            sqlx::query_scalar("SELECT xmin::text FROM notification_channels WHERE id=$1")
                .bind(id)
                .fetch_one(&o.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            after, version,
            "publication never replays the completed writer"
        );
        assert_eq!(
            o.count(
                "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='channel.updated'",
                id
            )
            .await,
            1
        );
    }
}

#[tokio::test]
async fn channel_update_preserves_the_native_legacy_settings_management_response() {
    let o = Outside::new().await;
    let id = Owner::Org.make(&o).await;
    let token = new_token(API_TOKEN_PREFIX);
    orch_db::queries::identity::create_api_token(
        &mut *o.world.conn().await,
        &orch_db::queries::identity::NewApiToken {
            id: new_id(),
            user_id: o.bea.id(),
            name: "Legacy channel management fixture",
            token_hash: &token_hash(&token),
            scopes: &["settings.manage".into()],
            node_id: Some(o.acme),
            at: o.world.now(),
            expires_at: Some(o.world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let body = value(
        update(o.world.app.clone(), token, id, rename()).await,
        StatusCode::OK,
    );
    let native = o
        .channels(&o.bea)
        .await
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(body, json!(native));
}
