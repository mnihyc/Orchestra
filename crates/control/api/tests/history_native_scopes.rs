//! History binds actual page/row sources before paging and retains its own live-name bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access::{
    AccessChoice, HistoryList, HistoryListQuery, HolderInput, PageChange, PageChangeName,
    PermissionPageUpdate,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::{hierarchy, identity, resources};
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use serde_json::json;
use support::{FullPermissionPageReply, World};

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":["read"]}]})
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
            name: "Native history source fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
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

struct Scene {
    world: World,
    owner: Who,
    project: Id,
    environment: Id,
    threads: [Id; 2],
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let org = places::make_org(&world, &owner, "History root", "history-root")
            .await
            .base
            .id;
        let project = places::make_project(&world, &owner, org, "Current protected target")
            .await
            .base
            .id;
        let topic = places::make_topic(&world, &owner, project, "History conversations")
            .await
            .base
            .id;
        let mut conn = world.conn().await;
        let environment = resources::access_fixtures::insert_environment(
            &mut conn,
            owner.id(),
            Some(project),
            "History environment",
            "managed",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
            owner.id(),
            NewAccess::Defaults,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        let threads = [new_id(), new_id()];
        for (id, name) in [
            (threads[0], "Allowed current thread"),
            (threads[1], "Other current thread"),
        ] {
            hierarchy::testing::insert_node(&mut conn, id, "thread", Some(topic), name, owner.id())
                .await
                .unwrap();
            let live: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads t JOIN thread_continuations c ON c.thread_id=t.node_id AND c.id=t.active_continuation_id WHERE t.node_id=$1)")
                .bind(id).fetch_one(&mut *conn).await.unwrap();
            assert!(
                live,
                "the fixture must contain the actual active native continuation"
            );
        }
        drop(conn);
        Self {
            world,
            owner,
            project,
            environment,
            threads,
        }
    }
    async fn row(&self, thread: Option<Id>, summary: &str) -> Id {
        let mut entry = Entry::new(
            Actor::User(self.owner.id()),
            "workspace.file_written",
            "environment",
            Some(self.environment),
            summary,
        );
        entry.history = true;
        entry.environment_id = Some(self.environment);
        entry.node_id = Some(thread.unwrap_or(self.project));
        entry.details = thread.map_or_else(
            || json!({"path":"native.txt"}),
            |id| json!({"thread_id":id,"path":"native.txt"}),
        );
        let id = audit::append(
            &mut *self.world.conn().await,
            &entry,
            &Origin::default(),
            self.world.now(),
        )
        .await
        .unwrap();
        // Native chronological paging is tested independently of randomly generated UUID order.
        self.world.advance(chrono::Duration::seconds(1));
        id
    }
    async fn read(&self, token: &str, query: &HistoryListQuery) -> RawReply {
        self.world
            .app
            .call(HistoryList, &[&"environments", &self.environment])
            .token(token)
            .query(query)
            .send_bytes()
            .await
    }
    fn rules(&self, threads: &[Id]) -> Vec<Json> {
        let mut rules = vec![rule("HistoryList", "environment", self.environment)];
        rules.extend(threads.iter().map(|id| rule("HistoryList", "thread", *id)));
        rules
    }
}
fn files(limit: u32) -> HistoryListQuery {
    HistoryListQuery {
        kinds: Some("files".into()),
        limit: Some(limit),
        ..Default::default()
    }
}

#[tokio::test]
async fn history_filters_actual_thread_sources_before_the_native_page_window() {
    let s = Scene::new().await;
    let older = s.row(Some(s.threads[0]), "Stored allowed line").await;
    let newer = s.row(Some(s.threads[1]), "Stored excluded line").await;
    let selected = key(&s.world, s.owner.id(), s.rules(&[s.threads[0]])).await;
    let body = shown(s.read(&selected.secret, &files(1)).await);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], json!(older));
    assert!(body["next_cursor"].is_null());
    assert!(!body.to_string().contains("Stored excluded line"));
    let root_only = key(&s.world, s.owner.id(), s.rules(&[])).await;
    assert_eq!(
        shown(s.read(&root_only.secret, &files(1)).await)["items"],
        json!([])
    );
    let mut full_rules = s.rules(&s.threads);
    full_rules.push(rule("HistoryList", "project", s.project));
    let full = key(&s.world, s.owner.id(), full_rules).await;
    let ordinary = shown(s.read(&s.owner.token, &files(1)).await);
    assert_eq!(ordinary["items"][0]["id"], json!(newer));
    assert!(ordinary["next_cursor"].as_str().is_some());
    assert_eq!(shown(s.read(&full.secret, &files(1)).await), ordinary);
    let next = HistoryListQuery {
        cursor: ordinary["next_cursor"].as_str().map(str::to_owned),
        ..files(1)
    };
    assert_eq!(
        shown(s.read(&full.secret, &next).await)["items"][0]["id"],
        json!(older)
    );
}

#[tokio::test]
async fn history_keeps_stored_wording_but_live_target_names_need_its_own_read() {
    let s = Scene::new().await;
    s.row(None, "Copied historical wording").await;
    let mut narrow = s.rules(&[]);
    narrow.push(rule("NodeTagList", "project", s.project));
    let limited = key(&s.world, s.owner.id(), narrow).await;
    let body = shown(s.read(&limited.secret, &files(10)).await);
    assert_eq!(body["items"][0]["text"], "Copied historical wording");
    assert_eq!(
        body["items"][0]["wording"]["target"]["id"],
        json!(s.project)
    );
    assert!(body["items"][0]["wording"]["target"]["name"].is_null());
    assert_eq!(body["items"][0]["wording"]["target"]["redacted"], true);
    assert!(!body.to_string().contains("Current protected target"));
    let mut rules = s.rules(&[]);
    rules.push(rule("HistoryList", "project", s.project));
    let full = key(&s.world, s.owner.id(), rules).await;
    let ordinary = shown(s.read(&s.owner.token, &files(10)).await);
    assert!(ordinary.to_string().contains("Current protected target"));
    assert_eq!(shown(s.read(&full.secret, &files(10)).await), ordinary);
}

#[tokio::test]
async fn history_scope_never_supplies_the_native_page_or_thread_read_gate() {
    let s = Scene::new().await;
    s.row(Some(s.threads[0]), "A private thread line").await;
    let visitor = places::person(&s.world, "visitor").await;
    let selected = key(&s.world, visitor.id(), s.rules(&s.threads)).await;
    let denied = s.read(&selected.secret, &files(10)).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    s.world
        .app
        .call(PermissionPageUpdate, &[&"environments", &s.environment])
        .token(&s.owner.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: visitor.id() }],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .full_permission_page()
        .ok();
    let ordinary = shown(s.read(&visitor.token, &files(10)).await);
    assert_eq!(ordinary["items"], json!([]));
    let body = shown(s.read(&selected.secret, &files(10)).await);
    assert_eq!(body, ordinary);
    assert!(!body.to_string().contains("A private thread line"));
}

#[tokio::test]
async fn history_publication_does_not_lend_a_new_page_to_its_old_thread_sources() {
    let s = Scene::new().await;
    s.row(Some(s.threads[0]), "Current allowed older line")
        .await;
    s.row(Some(s.threads[1]), "Held prior thread line").await;
    let token = key(&s.world, s.owner.id(), s.rules(&s.threads)).await;
    let control = shown(s.read(&token.secret, &files(1)).await);
    assert_eq!(control["items"][0]["text"], "Held prior thread line");
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
    let environment = s.environment;
    let secret = token.secret.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(HistoryList, &[&"environments", &environment])
            .token(&secret)
            .query(&files(1));
        call.send_bytes().await
    });
    // The existing database-wait watchdog; observed activity SQL is the actual held cut.
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the activity wait was not reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the produced history body must reach its actual activity append");
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token.id)
        .bind(json!({"rules":s.rules(&[s.threads[0]])}))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Held prior thread line"));
    let current = shown(s.read(&token.secret, &files(1)).await);
    assert_eq!(current["items"][0]["text"], "Current allowed older line");
}
