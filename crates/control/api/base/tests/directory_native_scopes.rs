//! Native directory rows exercise the shared stored/selection adapters with real fixture keys.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

#[path = "directory_native_scopes/creation.rs"]
mod creation;
mod places;
mod support;

use crate::support::FullTagReply as _;
use axum::body::{Body, to_bytes};
use chrono::Duration;
use futures::FutureExt as _;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::MAX_JSON_BYTES;
use orch_api_contract::directory::{
    NodeTagCreate, NodeTagCreateBody, OrgMemberList, OrgMemberListQuery, OrgMemberSuspend,
    OrgServiceAccountCreate, OrgServiceAccountCreateBody, OrgServiceAccountList, TagDelete,
    TagUpdate,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use places::Who;
use serde_json::json;
use support::World;
use tower::ServiceExt;

struct Key {
    id: Id,
    secret: String,
}

fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resource: &str,
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":[resource],
        "allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Directory native scope fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}

fn checked(reply: RawReply, status: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    if reply.raw.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(&reply.raw).unwrap()
    }
}

struct Scene {
    world: World,
    boss: Who,
    org: Id,
    member: Who,
    tag: Id,
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let boss = places::person(&world, "owner").await;
        let org = places::make_org(&world, &boss, "Private directory root", "directory-root")
            .await
            .base
            .id;
        let member = places::person(&world, "zulu").await;
        places::join(&world, org, org, &member, "viewer", &boss).await;
        let tag = world
            .app
            .call(NodeTagCreate, &[&org])
            .token(&boss.token)
            .body(&NodeTagCreateBody {
                name: "Private existing tag".into(),
                color: Some(3),
            })
            .send()
            .map(|reply| reply.full_tag())
            .await
            .ok()
            .id;
        Self {
            world,
            boss,
            org,
            member,
            tag,
        }
    }
    async fn writer(&self, operation: &str, actions: &[&str]) -> Key {
        key(
            &self.world,
            self.boss.id(),
            vec![rule(operation, "org", self.org, "self", "org", actions)],
        )
        .await
    }
    async fn tag(&self, secret: &str, body: Json) -> RawReply {
        self.world
            .app
            .call(TagUpdate, &[&self.tag])
            .token(secret)
            .raw_body(&body)
            .send_bytes()
            .await
    }
    async fn tag_name(&self) -> String {
        sqlx::query_scalar("SELECT name FROM tags WHERE id=$1")
            .bind(self.tag)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn stored_tag_manage_only_keeps_the_effect_without_stored_content() {
    let s = Scene::new().await;
    let writer = key(
        &s.world,
        s.boss.id(),
        vec![
            rule("TagUpdate", "org", s.org, "self", "org", &["manage"]),
            rule("NodeTagList", "org", s.org, "self", "org", &["read"]),
        ],
    )
    .await;
    for body in [json!({"name":"Supplied new tag"}), json!({})] {
        let response = checked(s.tag(&writer.secret, body).await, StatusCode::OK);
        assert_eq!(response, json!({"projection":"receipt","tag_id":s.tag}));
        assert_eq!(s.tag_name().await, "Supplied new tag");
    }
    let full = s.writer("TagUpdate", &["manage", "read"]).await;
    let response = checked(
        s.tag(&full.secret, json!({"color":7})).await,
        StatusCode::OK,
    );
    assert_eq!(response["name"], "Supplied new tag");
    assert_eq!(response["color"], 7);
    assert!(response.get("projection").is_none());
}

#[tokio::test]
async fn stored_tag_uses_its_keeper_and_own_action_without_lending_ordinary_manage() {
    let s = Scene::new().await;
    for rule in [
        rule("TagDelete", "org", s.org, "self", "org", &["manage"]),
        rule("TagUpdate", "org", s.org, "self", "org", &["read"]),
        rule("TagUpdate", "org", new_id(), "self", "org", &["manage"]),
        rule(
            "TagUpdate",
            "account",
            s.boss.id(),
            "self",
            "account",
            &["manage"],
        ),
    ] {
        let wrong = key(&s.world, s.boss.id(), vec![rule]).await;
        checked(
            s.tag(&wrong.secret, json!({"name":"Must stay unwritten"}))
                .await,
            StatusCode::FORBIDDEN,
        );
        assert_eq!(s.tag_name().await, "Private existing tag");
    }
    let viewer = key(
        &s.world,
        s.member.id(),
        vec![rule("TagUpdate", "org", s.org, "self", "org", &["manage"])],
    )
    .await;
    let refusal = s
        .tag(&viewer.secret, json!({"name":"Must stay unwritten"}))
        .await;
    assert_eq!(refusal.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&refusal.raw).contains("Private existing tag"));
    assert_eq!(s.tag_name().await, "Private existing tag");
}

#[tokio::test]
async fn stored_tag_delete_needs_no_read_and_preserves_native_topic_tags() {
    let s = Scene::new().await;
    let project = places::make_project(&s.world, &s.boss, s.org, "Project")
        .await
        .base
        .id;
    let topic = places::make_topic(&s.world, &s.boss, project, "Topic")
        .await
        .base
        .id;
    let tag = s
        .world
        .app
        .call(NodeTagCreate, &[&topic])
        .token(&s.boss.token)
        .body(&NodeTagCreateBody {
            name: "Native topic tag".into(),
            color: None,
        })
        .send()
        .map(|reply| reply.full_tag())
        .await
        .ok()
        .id;
    let writer = key(
        &s.world,
        s.boss.id(),
        vec![rule(
            "TagDelete",
            "topic",
            topic,
            "self",
            "topic",
            &["manage"],
        )],
    )
    .await;
    checked(
        s.world
            .app
            .call(TagDelete, &[&tag])
            .token(&writer.secret)
            .send_bytes()
            .await,
        StatusCode::NO_CONTENT,
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM tags WHERE id=$1")
        .bind(tag)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn member_suspension_manage_only_returns_accepted_ids_and_full_read_stays_optional() {
    let s = Scene::new().await;
    let writer = s.writer("OrgMemberSuspend", &["manage"]).await;
    let receipt = checked(
        s.world
            .app
            .call(OrgMemberSuspend, &[&s.org, &s.member.id()])
            .token(&writer.secret)
            .raw_body(&json!({"reason":"Supplied reason"}))
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(
        receipt,
        json!({"projection":"receipt","org_id":s.org,"user_id":s.member.id()})
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_suspensions WHERE org_id=$1 AND user_id=$2")
            .bind(s.org)
            .bind(s.member.id())
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let collision = s
        .world
        .app
        .call(OrgMemberSuspend, &[&s.org, &s.member.id()])
        .token(&writer.secret)
        .raw_body(&json!({}))
        .send_bytes()
        .await;
    assert_eq!(collision.status, StatusCode::CONFLICT);
    let wire = String::from_utf8_lossy(&collision.raw);
    assert!(!wire.contains("Supplied reason") && !wire.contains("Private directory root"));
    let other = places::person(&s.world, "second").await;
    places::join(&s.world, s.org, s.org, &other, "viewer", &s.boss).await;
    let full = s.writer("OrgMemberSuspend", &["manage", "read"]).await;
    let response = checked(
        s.world
            .app
            .call(OrgMemberSuspend, &[&s.org, &other.id()])
            .token(&full.secret)
            .raw_body(&json!({"reason":"Another supplied reason"}))
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response["user"]["id"], json!(other.id()));
    assert_eq!(response["reason"], "Another supplied reason");
    assert!(response.get("projection").is_none());
}

#[tokio::test]
async fn member_rows_filter_accounts_and_domain_sources_before_the_native_page_window() {
    let s = Scene::new().await;
    let reader = key(
        &s.world,
        s.boss.id(),
        vec![
            rule(
                "OrgMemberList",
                "account",
                s.member.id(),
                "self",
                "account",
                &["read"],
            ),
            rule("OrgMemberList", "org", s.org, "subtree", "org", &["read"]),
        ],
    )
    .await;
    let query = OrgMemberListQuery {
        limit: Some(1),
        ..Default::default()
    };
    let response = checked(
        s.world
            .app
            .call(OrgMemberList, &[&s.org])
            .token(&reader.secret)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response["items"].as_array().unwrap().len(), 1);
    assert_eq!(response["items"][0]["user"]["id"], json!(s.member.id()));
    assert!(response["next_cursor"].is_null());
    let empty = key(
        &s.world,
        s.boss.id(),
        vec![
            rule(
                "OrgMemberList",
                "account",
                new_id(),
                "self",
                "account",
                &["read"],
            ),
            rule("OrgMemberList", "org", s.org, "subtree", "org", &["read"]),
        ],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(OrgMemberList, &[&s.org])
            .token(&empty.secret)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response["items"], json!([]));
    assert!(response["next_cursor"].is_null());
}

#[tokio::test]
async fn service_rows_use_actual_account_ownership_and_their_own_manage_selection() {
    let s = Scene::new().await;
    let mut accounts = Vec::new();
    for username in ["service-one", "service-two"] {
        let account = s
            .world
            .app
            .call(OrgServiceAccountCreate, &[&s.org])
            .token(&s.boss.token)
            .body(&OrgServiceAccountCreateBody {
                username: username.into(),
                display_name: username.into(),
            })
            .send()
            .await
            .ok()
            .into_full()
            .expect("this ordinary control requires the full native creation response");
        accounts.push(account.user.id);
    }
    let reader = key(
        &s.world,
        s.boss.id(),
        vec![rule(
            "OrgServiceAccountList",
            "account",
            accounts[1],
            "self",
            "account",
            &["manage"],
        )],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(OrgServiceAccountList, &[&s.org])
            .token(&reader.secret)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response.as_array().unwrap().len(), 1);
    assert_eq!(response[0]["user"]["id"], json!(accounts[1]));
    let subtree = key(
        &s.world,
        s.boss.id(),
        vec![rule(
            "OrgServiceAccountList",
            "org",
            s.org,
            "subtree",
            "account",
            &["manage"],
        )],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(OrgServiceAccountList, &[&s.org])
            .token(&subtree.secret)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response.as_array().unwrap().len(), 2);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reader.id).bind(json!({"rules":[rule("OrgServiceAccountList", "account", new_id(), "self", "account", &["manage"])]}))
        .execute(&s.world.db.pool).await.unwrap();
    let response = checked(
        s.world
            .app
            .call(OrgServiceAccountList, &[&s.org])
            .token(&reader.secret)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response, json!([]));
}

#[tokio::test]
async fn legacy_directory_writers_keep_the_native_full_reply_under_current_permission_scopes() {
    for scopes in [
        vec!["*".to_owned()],
        vec!["org.manage".to_owned(), "members.manage".to_owned()],
    ] {
        let s = Scene::new().await;
        let secret = new_token(API_TOKEN_PREFIX);
        identity::create_api_token(
            &mut *s.world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: s.boss.id(),
                name: "Legacy directory scope fixture",
                token_hash: &token_hash(&secret),
                scopes: &scopes,
                node_id: Some(s.org),
                at: s.world.now(),
                expires_at: Some(s.world.now() + Duration::hours(1)),
            },
        )
        .await
        .unwrap();
        let tag = checked(s.tag(&secret, json!({"color":6})).await, StatusCode::OK);
        assert_eq!(tag["name"], "Private existing tag");
        assert_eq!(tag["color"], 6);
        assert!(tag.get("projection").is_none());
        let suspension = checked(
            s.world
                .app
                .call(OrgMemberSuspend, &[&s.org, &s.member.id()])
                .token(&secret)
                .raw_body(&json!({"reason":"Supplied legacy reason"}))
                .send_bytes()
                .await,
            StatusCode::OK,
        );
        assert_eq!(suspension["user"]["id"], json!(s.member.id()));
        assert_eq!(suspension["reason"], "Supplied legacy reason");
        assert!(suspension.get("projection").is_none());
    }
}

#[tokio::test]
async fn ordinary_tag_refusal_precedes_a_blocked_native_target() {
    let s = Scene::new().await;
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM tags WHERE id=$1 FOR UPDATE")
        .bind(s.tag)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let wait = std::time::Duration::from_secs(30);
    let update = tokio::time::timeout(
        wait,
        s.tag(&s.member.token, json!({"name":"Must stay unwritten"})),
    )
    .await;
    let delete = tokio::time::timeout(
        wait,
        s.world
            .app
            .call(TagDelete, &[&s.tag])
            .token(&s.member.token)
            .send_bytes(),
    )
    .await;
    blocker.rollback().await.unwrap();
    assert_eq!(
        update
            .expect("ordinary Manage refusal must precede the tag row wait")
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        delete
            .expect("ordinary delete refusal must precede the tag row wait")
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(s.tag_name().await, "Private existing tag");
}

#[tokio::test]
async fn stored_tag_wait_rechecks_the_real_key_deadline_before_its_first_effect() {
    let s = Scene::new().await;
    let writer = s.writer("TagUpdate", &["manage", "read"]).await;
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM tags WHERE id=$1 FOR UPDATE")
        .bind(s.tag)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(s.world.app.app.clone());
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/tags/{}", s.tag))
        .header("authorization", format!("Bearer {}", writer.secret))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"Must not be committed"}).to_string(),
        ))
        .unwrap();
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    // Match the existing database-wait fixture budget; the observed lock is the premise.
    let wait = std::time::Duration::from_secs(30);
    let blocked = tokio::time::timeout(wait, async {
        loop {
            if action.is_finished() { return false; }
            let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if found { return true; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert!(blocked, "the actual native tag row wait must be reached");
    s.world.advance(Duration::hours(2));
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(wait, action).await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("Private existing tag"));
    assert_eq!(s.tag_name().await, "Private existing tag");
}

#[tokio::test]
async fn completed_tag_publication_returns_its_receipt_after_activity_wait_without_replaying() {
    for expire in [false, true] {
        let s = Scene::new().await;
        let writer = s.writer("TagUpdate", &["manage", "read"]).await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let tag = s.tag;
        let request = tokio::spawn(async move {
            let call = app.call(TagUpdate, &[&tag]);
            call.token(&writer.secret)
                .raw_body(&json!({"color":7}))
                .send_bytes()
                .await
        });
        // As in other native-wait controls, the real blocked SQL proves the tested boundary.
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "activity wait was not reached");
                let blocked: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                     AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                     AND query LIKE '%INSERT INTO api_token_activity%')",
                )
                .bind(pid)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
                if blocked {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the committed tag answer must reach its actual activity append");
        let before: (i16, String) = sqlx::query_as("SELECT color,xmin::text FROM tags WHERE id=$1")
            .bind(s.tag)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(before.0, 7);
        if expire {
            s.world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let response = checked(
            tokio::time::timeout(wait, request).await.unwrap().unwrap(),
            StatusCode::OK,
        );
        if expire {
            assert_eq!(response, json!({"projection":"receipt","tag_id":s.tag}));
        } else {
            assert_eq!(response["name"], "Private existing tag");
            assert_eq!(response["color"], 7);
        }
        let after: (i16, String) = sqlx::query_as("SELECT color,xmin::text FROM tags WHERE id=$1")
            .bind(s.tag)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            after, before,
            "publication must not repeat the completed mutation"
        );
    }
}

async fn tag_retry(s: &Scene, secret: &str, act: &str, body: &NodeTagCreateBody) -> RawReply {
    let call = s.world.app.call(NodeTagCreate, &[&s.org]);
    call.token(secret)
        .header("Idempotency-Key", act)
        .body(body)
        .send_bytes()
        .await
}
#[tokio::test]
async fn tag_creation_keys_replay_current_content_with_own_read_and_preserve_one_native_tag() {
    let s = Scene::new().await;
    let body = NodeTagCreateBody {
        name: "One tag retry".into(),
        color: None,
    };
    let act = new_id().to_string();
    let writer = key(
        &s.world,
        s.boss.id(),
        vec![rule(
            "NodeTagCreate",
            "org",
            s.org,
            "self",
            "org",
            &["manage"],
        )],
    )
    .await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM tags WHERE node_id=$1")
        .bind(s.org)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    let first = tag_retry(&s, &writer.secret, &act, &body).await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&first.raw)
    );
    let first: Json = serde_json::from_slice(&first.raw).unwrap();
    assert_eq!(first["projection"], "receipt");
    let id: Id = serde_json::from_value(first["tag_id"].clone()).unwrap();
    let update = s
        .world
        .app
        .call(orch_api_contract::directory::TagUpdate, &[&id]);
    assert!(
        update
            .token(&s.boss.token)
            .body(&orch_api_contract::directory::TagUpdateBody {
                name: Some("Current native tag".into()),
                color: Some(4)
            })
            .send_bytes()
            .await
            .status
            .is_success()
    );
    let reader = key(
        &s.world,
        s.boss.id(),
        vec![rule(
            "NodeTagCreate",
            "org",
            s.org,
            "self",
            "org",
            &["read"],
        )],
    )
    .await;
    let reply = tag_retry(&s, &reader.secret, &act, &body).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let current =
        serde_json::from_slice::<orch_api_contract::directory::TagCreateResponse>(&reply.raw)
            .unwrap()
            .into_full()
            .unwrap();
    assert_eq!(current.id, id);
    assert_eq!(current.name, "Current native tag");
    assert_eq!(current.color, 4);
    let wrong = key(
        &s.world,
        s.boss.id(),
        vec![rule("TagUpdate", "org", s.org, "self", "org", &["read"])],
    )
    .await;
    let reply = tag_retry(&s, &wrong.secret, &act, &body).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(serde_json::from_slice::<Json>(&reply.raw).unwrap(), first);
    assert!(
        tag_retry(&s, &reader.secret, &new_id().to_string(), &body)
            .await
            .status
            .is_client_error()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tags WHERE node_id=$1")
            .bind(s.org)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap(),
        before + 1
    );
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(s.boss.id())
            .bind(&act)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(record["outcome"]["id"], json!(id));
    assert!(!record.to_string().contains("One tag retry"));
    assert!(!record.to_string().contains("Current native tag"));
}
