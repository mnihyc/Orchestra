//! The existing provider settings and bound-thread readers keep current scope and ordinary visibility.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use crate::support::FullPermissionPageReply;
use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::{ErrorCode, PageQuery, Problem};
use orch_api_contract::resources::{ProviderThreadList, ProviderThreadListReply};
use orch_api_contract::settings::{ProviderSettingList, ResolvedSetting};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::{self, NewApiToken};
use orch_settings::Target;
use orch_types::{Id, Json, Time, new_id};
use places::{make_project, make_topic};
use serde_json::json;
use std::time::Duration;
use support::World;
use tower::ServiceExt;

// Match the established C01/C02 lock rendezvous watchdog: scheduling headroom, never authority by time.
const LOCK_WAIT: Duration = Duration::from_secs(30);
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const IDLE: &str = "runtime.idle_stop_minutes";

struct Key {
    id: Id,
    secret: String,
}
struct Scene {
    world: World,
    company: resources::Company,
    provider: Id,
    other: Id,
    topic: Id,
    threads: Vec<Id>,
}

async fn insert_provider(world: &World, company: &resources::Company, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(company.boss.id()),
        Some(company.acme),
        name,
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id,
        },
        company.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let provider = insert_provider(&world, &company, "Auxiliary provider").await;
        let other = insert_provider(&world, &company, "Another provider").await;
        resources::name_person(
            &world,
            "provider",
            provider,
            "provider.use",
            company.viewer.id(),
        )
        .await;
        let project = make_project(&world, &company.boss, company.acme, "Protected project").await;
        let topic = make_topic(&world, &company.boss, project.base.id, "Protected topic")
            .await
            .base
            .id;
        let mut threads = Vec::new();
        for name in [
            "First bound thread",
            "Second bound thread",
            "Third bound thread",
        ] {
            let id = new_id();
            let mut conn = world.conn().await;
            insert_node(
                &mut conn,
                id,
                "thread",
                Some(topic),
                name,
                company.boss.id(),
            )
            .await
            .unwrap();
            orch_authz::pages::start(
                &mut conn,
                orch_access_contract::pages::StoredKind::Node,
                id,
                company.boss.id(),
                Time::new(world.now()),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE threads SET provider_id=$2,state='running' WHERE node_id=$1")
                .bind(id)
                .bind(provider)
                .execute(&mut *conn)
                .await
                .unwrap();
            threads.push(id);
        }
        threads.sort_unstable();
        world
            .set_at(Target::Node(company.acme), IDLE, json!(60))
            .await;
        Self {
            world,
            company,
            provider,
            other,
            topic,
            threads,
        }
    }
    fn full_rules(&self, operation: &str) -> Vec<Json> {
        let mut rules = vec![rule(operation, "provider", self.provider)];
        if operation == "ProviderThreadList" {
            rules.extend(self.threads.iter().map(|id| rule(operation, "thread", *id)));
        }
        rules.push(rule(operation, "org", self.company.acme));
        rules.push(rule(operation, "topic", self.topic));
        rules
    }
}

fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    issue(world, user, Some(rules), &[]).await
}
async fn issue(world: &World, user: Id, rules: Option<Vec<Json>>, scopes: &[&str]) -> Key {
    let scope = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Provider auxiliary reader control",
            token_hash: &token_hash(&secret),
            scopes: &scopes
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn settings(world: &World, token: &str, provider: Id) -> RawReply {
    world
        .app
        .call(ProviderSettingList, &[&provider])
        .token(token)
        .send_bytes()
        .await
}
async fn threads(world: &World, token: &str, provider: Id, query: PageQuery) -> RawReply {
    world
        .app
        .call(ProviderThreadList, &[&provider])
        .token(token)
        .query(&query)
        .send_bytes()
        .await
}
fn ok<T: serde::de::DeserializeOwned>(reply: RawReply) -> T {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}
fn setting<'a>(rows: &'a [ResolvedSetting], key: &str) -> &'a ResolvedSetting {
    rows.iter()
        .find(|row| row.key == key)
        .expect("the existing runtime key remains present")
}

#[tokio::test]
async fn exact_auxiliary_operations_keep_provider_use_only_browser_and_legacy_read_rights() {
    let s = Scene::new().await;
    for operation in ["ProviderSettingList", "ProviderThreadList"] {
        let actual = key(&s.world, s.company.viewer.id(), s.full_rules(operation)).await;
        let wrong = key(
            &s.world,
            s.company.viewer.id(),
            vec![rule(operation, "provider", s.other)],
        )
        .await;
        let unrelated = key(
            &s.world,
            s.company.viewer.id(),
            vec![rule(
                if operation == "ProviderSettingList" {
                    "ProviderThreadList"
                } else {
                    "ProviderSettingList"
                },
                "provider",
                s.provider,
            )],
        )
        .await;
        let outside = key(&s.world, s.company.stranger.id(), s.full_rules(operation)).await;
        let legacy = issue(
            &s.world,
            s.company.viewer.id(),
            None,
            &[
                "provider.view",
                "provider.use",
                "org.view",
                "project.view",
                "topic.view",
                "thread.view",
            ],
        )
        .await;
        if operation == "ProviderSettingList" {
            let ordinary: Vec<ResolvedSetting> =
                ok(settings(&s.world, &s.company.viewer.token, s.provider).await);
            assert_eq!(
                ok::<Vec<ResolvedSetting>>(settings(&s.world, &actual.secret, s.provider).await),
                ordinary
            );
            assert_eq!(
                ok::<Vec<ResolvedSetting>>(settings(&s.world, &legacy.secret, s.provider).await),
                ordinary
            );
            for token in [&wrong.secret, &unrelated.secret, &outside.secret] {
                denied(settings(&s.world, token, s.provider).await);
            }
        } else {
            let ordinary: ProviderThreadListReply = ok(threads(
                &s.world,
                &s.company.viewer.token,
                s.provider,
                Default::default(),
            )
            .await);
            assert_eq!(
                ordinary.items.len(),
                3,
                "the viewer's ordinary thread access is a real positive premise"
            );
            assert_eq!(
                ok::<ProviderThreadListReply>(
                    threads(&s.world, &actual.secret, s.provider, Default::default()).await
                ),
                ordinary
            );
            assert_eq!(
                ok::<ProviderThreadListReply>(
                    threads(&s.world, &legacy.secret, s.provider, Default::default()).await
                ),
                ordinary
            );
            for token in [&wrong.secret, &unrelated.secret, &outside.secret] {
                denied(threads(&s.world, token, s.provider, Default::default()).await);
            }
        }
        // Legacy labels remain exact: an account's ordinary Use implies View, but this key's
        // provider.use word alone does not carry the existing provider.view endpoint check.
        let use_only_legacy = issue(&s.world, s.company.viewer.id(), None, &["provider.use"]).await;
        let refused = if operation == "ProviderSettingList" {
            settings(&s.world, &use_only_legacy.secret, s.provider).await
        } else {
            threads(
                &s.world,
                &use_only_legacy.secret,
                s.provider,
                Default::default(),
            )
            .await
        };
        let problem: Json = serde_json::from_slice(&refused.raw).unwrap();
        assert_eq!(problem["required"], "provider.view");
        denied(refused);
    }
}

#[tokio::test]
async fn setting_source_names_need_this_operation_read_without_changing_values_or_writable_flags() {
    let s = Scene::new().await;
    let mut narrow_rules = vec![
        rule("ProviderSettingList", "provider", s.provider),
        rule("ProviderThreadList", "org", s.company.acme),
    ];
    let narrow = key(&s.world, s.company.boss.id(), narrow_rules.clone()).await;
    let hidden: Vec<ResolvedSetting> = ok(settings(&s.world, &narrow.secret, s.provider).await);
    let idle = setting(&hidden, IDLE);
    assert_eq!(idle.value, Some(json!(60)));
    assert!(idle.writable);
    let node = idle.source.node.as_ref().unwrap();
    assert_eq!(node.id, s.company.acme);
    assert!(node.redacted && node.name.is_none());
    narrow_rules.push(rule("ProviderSettingList", "org", s.company.acme));
    let full = key(&s.world, s.company.boss.id(), narrow_rules).await;
    let shown: Vec<ResolvedSetting> = ok(settings(&s.world, &full.secret, s.provider).await);
    let ordinary: Vec<ResolvedSetting> =
        ok(settings(&s.world, &s.company.boss.token, s.provider).await);
    assert_eq!(shown, ordinary);
    assert_eq!(
        setting(&shown, IDLE)
            .source
            .node
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Acme")
    );
    for row in &hidden {
        let other = setting(&shown, &row.key);
        assert_eq!(row.value, other.value);
        assert_eq!(row.writable, other.writable);
    }
}

#[tokio::test]
async fn settings_keep_existing_read_only_manager_and_owner_only_command_flags() {
    let s = Scene::new().await;
    let read = key(
        &s.world,
        s.company.viewer.id(),
        vec![rule("ProviderSettingList", "provider", s.provider)],
    )
    .await;
    let shown: Vec<ResolvedSetting> = ok(settings(&s.world, &read.secret, s.provider).await);
    assert!(shown.iter().all(|row| !row.writable));
    resources::name_person(
        &s.world,
        "provider",
        s.provider,
        "provider.manage",
        s.company.viewer.id(),
    )
    .await;
    let manager: Vec<ResolvedSetting> = ok(settings(&s.world, &read.secret, s.provider).await);
    assert!(setting(&manager, "runtime.needs").writable);
    assert!(!setting(&manager, "runtime.after_up").writable);
    assert!(!setting(&manager, "runtime.template_id").writable);
    let owner = key(
        &s.world,
        s.company.boss.id(),
        vec![rule("ProviderSettingList", "provider", s.provider)],
    )
    .await;
    let owner: Vec<ResolvedSetting> = ok(settings(&s.world, &owner.secret, s.provider).await);
    assert!(setting(&owner, "runtime.after_up").writable);
    assert!(setting(&owner, "runtime.template_id").writable);
}

#[tokio::test]
async fn actual_thread_read_filtering_precedes_page_limits_without_counting_key_exclusions_as_hidden()
 {
    let s = Scene::new().await;
    let allowed = [s.threads[1], s.threads[2]];
    let mut rules = vec![rule("ProviderThreadList", "provider", s.provider)];
    rules.extend(
        allowed
            .iter()
            .map(|id| rule("ProviderThreadList", "thread", *id)),
    );
    let token = key(&s.world, s.company.boss.id(), rules).await;
    let mut cursor = None;
    let mut collected = Vec::new();
    let mut pages = 0;
    loop {
        let page: ProviderThreadListReply = ok(threads(
            &s.world,
            &token.secret,
            s.provider,
            PageQuery {
                limit: Some(1),
                cursor: cursor.clone(),
            },
        )
        .await);
        assert_eq!(
            page.hidden, 0,
            "key-scope omissions do not invent ordinary hidden-thread counts"
        );
        assert_eq!(
            page.items.len(),
            1,
            "a scope-excluded earlier row cannot consume the page"
        );
        collected.push(page.items[0].thread.id);
        pages += 1;
        if let Some(next) = page.next_cursor {
            cursor = Some(next);
        } else {
            break;
        }
        assert!(
            pages < 3,
            "the two admitted rows form two pages without a cursor loop"
        );
    }
    assert_eq!(collected, allowed);
    let empty = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("ProviderThreadList", "provider", s.provider),
            rule("ProviderSettingList", "thread", s.threads[0]),
        ],
    )
    .await;
    let page: ProviderThreadListReply =
        ok(threads(&s.world, &empty.secret, s.provider, Default::default()).await);
    assert!(page.items.is_empty() && page.next_cursor.is_none());
    assert_eq!(page.hidden, 0);
}

#[tokio::test]
async fn admitted_thread_rows_keep_public_owners_and_bound_topic_and_org_names_independently() {
    let s = Scene::new().await;
    let primary = vec![
        rule("ProviderThreadList", "provider", s.provider),
        rule("ProviderThreadList", "thread", s.threads[0]),
        rule("ProviderSettingList", "topic", s.topic),
        rule("ProviderSettingList", "org", s.company.acme),
    ];
    let narrow = key(&s.world, s.company.boss.id(), primary.clone()).await;
    let hidden: ProviderThreadListReply =
        ok(threads(&s.world, &narrow.secret, s.provider, Default::default()).await);
    let row = &hidden.items[0];
    assert_eq!(row.thread.id, s.threads[0]);
    assert!(row.thread.name.is_some() && !row.thread.redacted);
    assert!(row.topic.name.is_none() && row.topic.redacted);
    assert!(row.org.name.is_none() && row.org.redacted);
    assert_eq!(row.owner.username, "boss");
    let mut rules = primary;
    rules.push(rule("ProviderThreadList", "topic", s.topic));
    rules.push(rule("ProviderThreadList", "org", s.company.acme));
    let full = key(&s.world, s.company.boss.id(), rules).await;
    let shown: ProviderThreadListReply =
        ok(threads(&s.world, &full.secret, s.provider, Default::default()).await);
    let normal: ProviderThreadListReply = ok(threads(
        &s.world,
        &s.company.boss.token,
        s.provider,
        Default::default(),
    )
    .await);
    let expected = normal
        .items
        .iter()
        .find(|row| row.thread.id == s.threads[0])
        .unwrap();
    assert_eq!(&shown.items[0], expected);
    assert_eq!(shown.items[0].owner, row.owner);
}

#[tokio::test]
async fn thread_scope_never_manufactures_ordinary_view_of_a_private_bound_thread() {
    let s = Scene::new().await;
    s.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &s.threads[1]])
        .token(&s.company.boss.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .full_permission_page()
        .ok();
    let token = key(
        &s.world,
        s.company.viewer.id(),
        s.full_rules("ProviderThreadList"),
    )
    .await;
    let shown: ProviderThreadListReply =
        ok(threads(&s.world, &token.secret, s.provider, Default::default()).await);
    let normal: ProviderThreadListReply = ok(threads(
        &s.world,
        &s.company.viewer.token,
        s.provider,
        Default::default(),
    )
    .await);
    assert_eq!(shown, normal);
    assert_eq!(shown.hidden, 1);
    assert_eq!(shown.items.len(), 2);
    assert!(shown.items.iter().all(|row| row.thread.id != s.threads[1]));
}

#[tokio::test]
async fn both_auxiliary_readers_restore_current_narrowed_and_revoked_key_rows() {
    let s = Scene::new().await;
    for operation in ["ProviderSettingList", "ProviderThreadList"] {
        for revoke in [false, true] {
            let token = key(&s.world, s.company.boss.id(), s.full_rules(operation)).await;
            let before = if operation == "ProviderSettingList" {
                settings(&s.world, &token.secret, s.provider).await
            } else {
                threads(&s.world, &token.secret, s.provider, Default::default()).await
            };
            assert_eq!(before.status, StatusCode::OK);
            if revoke {
                identity::revoke_api_token(
                    &mut *s.world.conn().await,
                    token.id,
                    s.company.boss.id(),
                    s.world.now(),
                )
                .await
                .unwrap();
            } else {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(serde_json::to_value(KeyScope::default()).unwrap())
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            let after = if operation == "ProviderSettingList" {
                settings(&s.world, &token.secret, s.provider).await
            } else {
                threads(&s.world, &token.secret, s.provider, Default::default()).await
            };
            assert_eq!(
                after.status,
                if revoke {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&after.raw).contains("runtime.idle_stop_minutes"));
            assert!(!String::from_utf8_lossy(&after.raw).contains("First bound thread"));
        }
    }
}

async fn checkpoint(operation: &'static str, narrow: bool) {
    let s = Scene::new().await;
    let token = key(&s.world, s.company.boss.id(), s.full_rules(operation)).await;
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_person(&mut blocker, s.company.boss.id())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(token.id)
        .bind(s.company.boss.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(s.world.app.app.clone());
    let path = format!(
        "/providers/{}/{}",
        s.provider,
        if operation == "ProviderSettingList" {
            "settings"
        } else {
            "threads"
        }
    );
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", format!("Bearer {}", token.secret))
        .body(Body::empty())
        .unwrap();
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let observed=tokio::time::timeout(LOCK_WAIT,async {
        loop {
            if action.is_finished() {break false;}
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(blocker_pid).fetch_one(&s.world.db.pool).await.unwrap();
            if waiting {break true;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual reader waits on current identity or finishes");
    if narrow {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token.id)
            .bind(serde_json::to_value(KeyScope::default()).unwrap())
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(LOCK_WAIT, action)
        .await
        .expect("released reader completes")
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    assert_eq!(
        status,
        if narrow {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        },
        "{operation}: actual identity lock observed={observed}; {}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(
        observed,
        "the current read checkpoint must actually wait on the locked identity"
    );
    if narrow {
        let problem: Problem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
        let raw = String::from_utf8_lossy(&bytes);
        assert!(!raw.contains(IDLE) && !raw.contains("First bound thread"));
    }
}

#[tokio::test]
async fn auxiliary_readers_recheck_current_scope_on_their_actual_read_connection() {
    for operation in ["ProviderSettingList", "ProviderThreadList"] {
        checkpoint(operation, false).await;
        checkpoint(operation, true).await;
    }
}
