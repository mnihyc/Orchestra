//! Selected channel namespaces, native management views and independently bounded own routes.

use super::*;
use http::StatusCode;
use orch_api_contract::notifications::MeNotificationRouteLinkCreate;
use orch_api_core::middleware::authentication::token_hash;
use orch_api_core::testing::{RawReply, TestApp};
use orch_types::Json;

const AVAILABLE: &str = "NotificationChannelList.available";
const MANAGE: &str = "NotificationChannelList.manage";

fn rule(operation: &str, target: Json, resource: &str, actions: &[&str]) -> Json {
    json!({"target":target,"resources":[resource],"allow":[{"operation":operation,"actions":actions}]})
}
fn org(o: &Outside, operation: &str, actions: &[&str]) -> Json {
    rule(
        operation,
        json!({"kind":"org","id":o.acme,"reach":"self"}),
        "org",
        actions,
    )
}
fn account(actor: Id, operation: &str) -> Json {
    rule(
        operation,
        json!({"kind":"account","id":actor,"reach":"self"}),
        "account",
        &["read"],
    )
}
fn system(operation: &str, actions: &[&str]) -> Json {
    rule(
        operation,
        json!({"kind":"system","reach":"self"}),
        "system",
        actions,
    )
}
async fn list(app: TestApp, token: String, query: NotificationChannelListQuery) -> RawReply {
    let call = app.call(NotificationChannelList, &[]);
    call.token(&token).query(&query).send_bytes().await
}
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn item(rows: &Json, id: Id) -> &Json {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(id))
        .unwrap()
}
fn no_private_route(row: &Json) {
    assert!(row["route"]["id"].is_null());
    assert!(row["route"]["link"].is_null());
    assert!(row["route"]["needs"].is_null());
    assert!(row["route"]["label"].is_null());
    assert_eq!(row["route"]["waiting_count"], 0);
}
struct Scene {
    o: Outside,
    system: Id,
    org: Id,
    own: Id,
    link: String,
}
impl Scene {
    async fn new() -> Self {
        let o = Outside::new().await;
        let system = make_system(&o, &o.ada, &Outside::bot("private-system-token"))
            .await
            .ok()
            .id;
        let org = make_org(&o, &o.bea, o.acme, &Outside::bot("private-org-token"))
            .await
            .ok()
            .id;
        let far = FarSide::taking().await;
        o.world
            .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
            .await;
        let own = make_own(
            &o,
            &o.bea,
            "Private personal hook",
            &far.url,
            Some("private-signing-value"),
        )
        .await
        .ok()
        .id;
        let call = o.world.app.call(MeNotificationRouteLinkCreate, &[&org]);
        let link = call
            .token(&o.bea.token)
            .send()
            .await
            .ok()
            .into_full()
            .expect("ordinary created notification content")
            .url;
        Self {
            o,
            system,
            org,
            own,
            link,
        }
    }
    fn all_reads(&self) -> Vec<Json> {
        vec![
            system(AVAILABLE, &["read"]),
            org(&self.o, AVAILABLE, &["read"]),
            account(self.o.bea.id(), AVAILABLE),
        ]
    }
    fn managed(&self, actor_read: bool) -> Vec<Json> {
        let mut rules = vec![org(&self.o, MANAGE, &["manage"])];
        if actor_read {
            rules.push(account(self.o.bea.id(), MANAGE));
        }
        rules
    }
    fn org_query(&self) -> NotificationChannelListQuery {
        NotificationChannelListQuery {
            org_id: Some(self.o.acme),
            ..Default::default()
        }
    }
}

#[tokio::test]
async fn channel_lists_filter_actual_namespaces_and_keep_native_fields_with_their_own_route_read() {
    let s = Scene::new().await;
    let native = shown(
        list(
            s.o.world.app.clone(),
            s.o.bea.token.clone(),
            Default::default(),
        )
        .await,
    );
    let full = super::scopes::key(&s.o, s.o.bea.id(), s.all_reads()).await;
    assert_eq!(
        shown(list(s.o.world.app.clone(), full, Default::default()).await),
        native
    );
    let narrow =
        super::scopes::key(&s.o, s.o.bea.id(), vec![org(&s.o, AVAILABLE, &["read"])]).await;
    let selected = shown(list(s.o.world.app.clone(), narrow, Default::default()).await);
    assert_eq!(selected.as_array().unwrap().len(), 1);
    let row = item(&selected, s.org);
    assert_eq!(row["config"], item(&native, s.org)["config"]);
    assert_eq!(row["secrets_set"], item(&native, s.org)["secrets_set"]);
    assert_eq!(
        row["can_edit"], true,
        "ordinary native editor fields remain their original projection"
    );
    no_private_route(row);
    assert!(!selected.to_string().contains(&s.link));
    assert!(!selected.to_string().contains("Private personal hook"));
    assert!(!selected.to_string().contains("private-org-token"));
    let account_only =
        super::scopes::key(&s.o, s.o.bea.id(), vec![account(s.o.bea.id(), AVAILABLE)]).await;
    let mine = shown(list(s.o.world.app.clone(), account_only, Default::default()).await);
    assert_eq!(mine.as_array().unwrap().len(), 1);
    assert_eq!(mine[0]["id"], json!(s.own));
    let other_account =
        super::scopes::key(&s.o, s.o.bea.id(), vec![account(s.o.cai.id(), AVAILABLE)]).await;
    assert_eq!(
        shown(list(s.o.world.app.clone(), other_account, Default::default()).await),
        json!([])
    );
}

#[tokio::test]
async fn channel_management_snapshots_need_actual_manage_and_use_only_their_own_account_read() {
    let s = Scene::new().await;
    let native = shown(list(s.o.world.app.clone(), s.o.bea.token.clone(), s.org_query()).await);
    let managed = super::scopes::key(&s.o, s.o.bea.id(), s.managed(false)).await;
    let body = shown(list(s.o.world.app.clone(), managed, s.org_query()).await);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], json!(s.org));
    assert_eq!(body[0]["config"], native[0]["config"]);
    assert_eq!(
        body[0]["enabled_route_count"],
        native[0]["enabled_route_count"]
    );
    no_private_route(&body[0]);
    let mut unrelated = s.managed(false);
    unrelated.push(account(s.o.bea.id(), AVAILABLE));
    let unrelated = super::scopes::key(&s.o, s.o.bea.id(), unrelated).await;
    no_private_route(&shown(list(s.o.world.app.clone(), unrelated, s.org_query()).await)[0]);
    let full = super::scopes::key(&s.o, s.o.bea.id(), s.managed(true)).await;
    assert_eq!(
        shown(list(s.o.world.app.clone(), full, s.org_query()).await),
        native
    );
    let wrong_variant =
        super::scopes::key(&s.o, s.o.bea.id(), vec![org(&s.o, AVAILABLE, &["read"])]).await;
    assert_eq!(
        list(s.o.world.app.clone(), wrong_variant, s.org_query())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let wrong_action =
        super::scopes::key(&s.o, s.o.bea.id(), vec![org(&s.o, MANAGE, &["read"])]).await;
    assert_eq!(
        list(s.o.world.app.clone(), wrong_action, s.org_query())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn system_channel_management_is_explicit_keeps_native_standing_and_needs_no_private_mode() {
    let s = Scene::new().await;
    let query = NotificationChannelListQuery {
        system: Some(true),
        ..Default::default()
    };
    let token = super::scopes::key(&s.o, s.o.ada.id(), vec![system(MANAGE, &["manage"])]).await;
    let body = shown(list(s.o.world.app.clone(), token.clone(), query.clone()).await);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], json!(s.system));
    assert_eq!(body[0]["can_edit"], true);
    assert!(
        body[0]["secrets_set"]
            .as_array()
            .unwrap()
            .contains(&json!("bot_token"))
    );
    no_private_route(&body[0]);
    assert!(!body.to_string().contains("private-system-token"));
    let unprivileged =
        super::scopes::key(&s.o, s.o.bea.id(), vec![system(MANAGE, &["manage"])]).await;
    assert_eq!(
        list(s.o.world.app.clone(), unprivileged, query.clone())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let not_native_manager =
        super::scopes::key(&s.o, s.o.cai.id(), vec![org(&s.o, MANAGE, &["manage"])]).await;
    let refused = list(s.o.world.app.clone(), not_native_manager, s.org_query()).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&refused.raw).contains("Acme"));
    let unavailable =
        super::scopes::key(&s.o, s.o.fay.id(), vec![org(&s.o, AVAILABLE, &["read"])]).await;
    assert_eq!(
        shown(list(s.o.world.app.clone(), unavailable, Default::default()).await),
        json!([])
    );
    let invalid = NotificationChannelListQuery {
        org_id: Some(s.o.acme),
        system: Some(true),
    };
    assert_eq!(
        list(s.o.world.app.clone(), token, invalid).await.status,
        StatusCode::BAD_REQUEST
    );
}

async fn wait(o: &Outside, pending: &tokio::task::JoinHandle<RawReply>, pid: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!pending.is_finished(), "the actual activity wait must be reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&o.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the produced body reaches its native activity append");
}

#[tokio::test]
async fn channel_publication_cannot_lend_another_current_namespace_or_drop_old_own_route_sources() {
    for actor_loss in [false, true] {
        let s = Scene::new().await;
        let token = super::scopes::key(&s.o, s.o.bea.id(), s.all_reads()).await;
        let control = shown(list(s.o.world.app.clone(), token.clone(), Default::default()).await);
        assert_eq!(item(&control, s.org)["route"]["link"]["url"], s.link);
        let mut blocker = s.o.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pending = tokio::spawn(list(
            s.o.world.app.clone(),
            token.clone(),
            Default::default(),
        ));
        wait(&s.o, &pending, pid).await;
        let rules = if actor_loss {
            vec![
                system(AVAILABLE, &["read"]),
                org(&s.o, AVAILABLE, &["read"]),
            ]
        } else {
            vec![account(s.o.bea.id(), AVAILABLE)]
        };
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
            .bind(token_hash(&token))
            .bind(json!({"rules":rules}))
            .execute(&s.o.world.db.pool)
            .await
            .unwrap();
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains(&s.link));
        let current = shown(list(s.o.world.app.clone(), token, Default::default()).await);
        if actor_loss {
            no_private_route(item(&current, s.org));
        } else {
            assert_eq!(current.as_array().unwrap().len(), 1);
            assert_eq!(current[0]["id"], json!(s.own));
        }
    }
}

#[tokio::test]
async fn channel_publication_keeps_the_actual_native_own_link_expiry_not_just_account_scope() {
    let s = Scene::new().await;
    let token = super::scopes::key(&s.o, s.o.bea.id(), s.managed(true)).await;
    let control = shown(list(s.o.world.app.clone(), token.clone(), s.org_query()).await);
    assert_eq!(control[0]["route"]["link"]["url"], s.link);
    let mut blocker = s.o.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(list(s.o.world.app.clone(), token.clone(), s.org_query()));
    wait(&s.o, &pending, pid).await;
    s.o.world.advance(chrono::Duration::minutes(11));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains(&s.link));
    let current = shown(list(s.o.world.app.clone(), token, s.org_query()).await);
    assert!(current[0]["route"]["link"].is_null());
}

#[tokio::test]
async fn channel_publication_rechecks_the_original_native_editor_cut_after_activity_wait() {
    let s = Scene::new().await;
    let token = super::scopes::key(&s.o, s.o.ada.id(), vec![system(AVAILABLE, &["read"])]).await;
    let control = shown(list(s.o.world.app.clone(), token.clone(), Default::default()).await);
    assert_eq!(item(&control, s.system)["can_edit"], true);
    let mut blocker = s.o.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(list(
        s.o.world.app.clone(),
        token.clone(),
        Default::default(),
    ));
    wait(&s.o, &pending, pid).await;
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(s.o.ada.id())
        .execute(&s.o.world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let current = shown(list(s.o.world.app.clone(), token, Default::default()).await);
    assert_eq!(item(&current, s.system)["can_edit"], false);
    assert_eq!(item(&current, s.system)["config"], json!({}));
}
